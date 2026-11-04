use crate::audio::{encode_sound_toggle_command, AudioBuffer};
use crate::clipboard::{
    decode_clipboard_packet, encode_clipboard_packet, get_linux_system_clipboard,
    set_linux_system_clipboard, ClipboardState,
};
use crate::file_transfer::{encode_chat_message, FileTransferEntry, FileTransferManager};
use crate::input::{keyboard, mouse_button, mouse_move, InputEvent};
use crate::protocol::{
    build_connection_request_command, build_login_with_password, build_registration_command,
    connection_request_password_value, decode_byte_array_fields, decode_target_route,
    decode_transport_body, encode_relay_frame, encode_transport_frame, encrypt_registration_bytes,
    extract_command3_identity, http_service_preface, md5_hex_lower, version_equal_or_greater,
    Command3Identity, TargetRoute, REGISTRATION_PASSWORD,
};
use crate::screen::{desktop_size_from_login, RGB555Screen, ZstdDictionaryDecoder};
use native_tls::{TlsConnector, TlsStream};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_REGISTRATION_HOST: &str = "functions3.ultraviewer.net";
pub const TLS_TARGET_HOST: &str = "ultraviewer";
pub const DEFAULT_REGISTRATION_JSON: &str = include_str!("../linux-controller-private.json");
pub const DEFAULT_PARTNER_JSON: &str = include_str!("../default-partner.json");
pub const DEFAULT_ZSTD_DICT: &[u8] = include_bytes!("../8bit.dat");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationProfile {
    pub terms: Vec<String>,
    #[serde(default = "default_ansi_codec")]
    pub ansi_codec: String,
    #[serde(default)]
    pub experimental_linux_metadata_policy: bool,
}

fn default_ansi_codec() -> String {
    "cp1252".to_string()
}

impl Default for RegistrationProfile {
    fn default() -> Self {
        serde_json::from_str(DEFAULT_REGISTRATION_JSON).unwrap_or(Self {
            terms: Vec::new(),
            ansi_codec: "cp1252".to_string(),
            experimental_linux_metadata_policy: true,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartnerProfile {
    pub request_terms: Vec<String>,
    pub password: String,
    #[serde(default)]
    pub password_type: i32,
}

impl Default for PartnerProfile {
    fn default() -> Self {
        serde_json::from_str(DEFAULT_PARTNER_JSON).unwrap_or(Self {
            request_terms: Vec::new(),
            password: String::new(),
            password_type: 0,
        })
    }
}

pub fn detect_local_ip() -> String {
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = sock.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

pub fn looks_like_tls_record(prefix: &[u8]) -> bool {
    if prefix.len() < 3 {
        return false;
    }
    matches!(prefix[0], 20 | 21 | 22 | 23) && prefix[1] == 3 && prefix[2] <= 4
}

pub fn now_epoch_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn random_digits(count: usize) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0x517c_c1b7_2722_0a95);
    let mut state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
        ^ COUNTER.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed);
    let mut out = String::with_capacity(count);
    for _ in 0..count {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let digit = (state.wrapping_mul(0x2545_f491_4f6c_dd1d) % 10) as u8;
        out.push((b'0' + digit) as char);
    }
    out
}

fn local_hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.trim().is_empty() {
            return h.trim().to_string();
        }
    }
    if let Ok(h) = fs::read_to_string("/etc/hostname") {
        let trimmed = h.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "linux-host".to_string()
}

pub enum TransportChannel {
    Plain(TcpStream),
    Tls(TlsStream<TcpStream>),
}

impl TransportChannel {
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        match self {
            Self::Plain(s) => {
                let _ = s.set_read_timeout(timeout);
                let _ = s.set_write_timeout(timeout);
            }
            Self::Tls(s) => {
                let _ = s.get_ref().set_read_timeout(timeout);
                let _ = s.get_ref().set_write_timeout(timeout);
            }
        }
    }
}

impl Read for TransportChannel {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Tls(s) => s.read(buf),
        }
    }
}

impl Write for TransportChannel {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
        }
    }
}

pub struct RegistrationTransport {
    pub timeout: Duration,
    pub max_frame: usize,
    pub original_certificate_policy: bool,
    pub channel: Option<TransportChannel>,
    pub buffer: Vec<u8>,
    pub response_header: Vec<u8>,
}

impl RegistrationTransport {
    pub fn new(
        timeout_secs: f64,
        max_frame: usize,
        original_certificate_policy: bool,
    ) -> Result<Self, String> {
        if timeout_secs <= 0.0 || max_frame == 0 {
            return Err("positive limits required".to_string());
        }
        Ok(Self {
            timeout: Duration::from_secs_f64(timeout_secs),
            max_frame,
            original_certificate_policy,
            channel: None,
            buffer: Vec::new(),
            response_header: Vec::new(),
        })
    }

    pub fn connect(&mut self, host: &str, port: u16) -> Result<Vec<u8>, String> {
        if self.channel.is_some() {
            return Err("already connected".to_string());
        }
        let addr = resolve_addr(host, port)?;
        let raw = TcpStream::connect_timeout(&addr, self.timeout)
            .map_err(|e| format!("connect failed to {}:{}: {}", host, port, e))?;
        let _ = raw.set_read_timeout(Some(self.timeout));
        let _ = raw.set_write_timeout(Some(self.timeout));

        let connector = build_tls_connector(self.original_certificate_policy)?;
        let tls = connector
            .connect(TLS_TARGET_HOST, raw)
            .map_err(|e| format!("TLS handshake failed: {}", e))?;
        self.channel = Some(TransportChannel::Tls(tls));

        let preface = http_service_preface(host, true);
        if let Some(ref mut ch) = self.channel {
            ch.write_all(&preface)
                .map_err(|e| format!("failed sending preface: {}", e))?;
        }
        let deadline = Instant::now() + self.timeout;
        while find_crlf_crlf(&self.buffer).is_none() {
            if self.buffer.len() >= 16384 {
                self.close();
                return Err("HTTP header exceeds limit".to_string());
            }
            if let Err(e) = self.receive_chunk(deadline) {
                self.close();
                return Err(e);
            }
        }
        let end = find_crlf_crlf(&self.buffer).unwrap() + 4;
        self.response_header = self.buffer[..end].to_vec();
        self.buffer.drain(..end);
        let first_line = self
            .response_header
            .split(|&b| b == b'\n')
            .next()
            .unwrap_or(&[]);
        let first_line = first_line.strip_suffix(b"\r").unwrap_or(first_line);
        if first_line != b"HTTP/1.1 200 OK" {
            self.close();
            return Err("registration tunnel did not return HTTP 200".to_string());
        }
        Ok(self.response_header.clone())
    }

    pub(crate) fn receive_chunk(&mut self, deadline: Instant) -> Result<(), String> {
        let now = Instant::now();
        if now >= deadline {
            return Err("TimeoutError: transport deadline".to_string());
        }
        let remaining = deadline - now;
        let ch = self
            .channel
            .as_mut()
            .ok_or_else(|| "not connected".to_string())?;
        ch.set_timeout(Some(remaining));
        let mut temp = [0u8; 4096];
        let n = ch.read(&mut temp).map_err(|e| {
            if matches!(
                e.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) {
                "TimeoutError: transport deadline".to_string()
            } else {
                format!("read error: {}", e)
            }
        })?;
        if n == 0 {
            return Err("EOFError: server closed transport".to_string());
        }
        self.buffer.extend_from_slice(&temp[..n]);
        Ok(())
    }

    pub fn send_command(&mut self, plaintext: &[u8]) -> Result<(), String> {
        let ch = self
            .channel
            .as_mut()
            .ok_or_else(|| "not connected".to_string())?;
        let encrypted = encrypt_registration_bytes(plaintext, REGISTRATION_PASSWORD);
        let mut body = Vec::with_capacity(2 + encrypted.len());
        body.push(1u8);
        body.push(0u8);
        body.extend_from_slice(&encrypted);
        if body.len() > self.max_frame {
            return Err("outbound frame exceeds limit".to_string());
        }
        let frame = encode_transport_frame(&body);
        ch.write_all(&frame)
            .map_err(|e| format!("send_command write failed: {}", e))
    }

    pub fn receive_body(&mut self) -> Result<Vec<u8>, String> {
        if self.channel.is_none() {
            return Err("not connected".to_string());
        }
        let deadline = Instant::now() + self.timeout;
        while self.buffer.len() < 4 {
            self.receive_chunk(deadline)?;
        }
        let size = u32::from_le_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as usize;
        if size == 0 || size > self.max_frame {
            return Err("invalid inbound frame length".to_string());
        }
        while self.buffer.len() < 4 + size {
            self.receive_chunk(deadline)?;
        }
        let body = self.buffer[4..4 + size].to_vec();
        self.buffer.drain(..4 + size);
        Ok(body)
    }

    pub fn receive_mode1_payload(&mut self) -> Result<Vec<u8>, String> {
        let body = self.receive_body()?;
        if body.is_empty() || body[0] != 1 {
            return Err("expected mode1 frame".to_string());
        }
        Ok(body[1..].to_vec())
    }

    pub fn receive_identity(&mut self) -> Result<Command3Identity, String> {
        let payload = self.receive_mode1_payload()?;
        extract_command3_identity(&payload)
    }

    pub fn close(&mut self) {
        self.channel = None;
        self.buffer.clear();
    }
}

pub struct SessionTransport {
    pub inner: RegistrationTransport,
}

impl SessionTransport {
    pub fn new(
        timeout_secs: f64,
        max_frame: usize,
        original_certificate_policy: bool,
    ) -> Result<Self, String> {
        Ok(Self {
            inner: RegistrationTransport::new(
                timeout_secs,
                max_frame,
                original_certificate_policy,
            )?,
        })
    }

    pub fn set_timeout(&mut self, timeout_secs: f64) {
        self.inner.timeout = Duration::from_secs_f64(timeout_secs.max(0.01));
    }

    pub fn connect(
        &mut self,
        host: &str,
        port: u16,
        http_hostname: &str,
        use_ssl: bool,
    ) -> Result<Vec<u8>, String> {
        if self.inner.channel.is_some() {
            return Err("already connected".to_string());
        }
        let addr = resolve_addr(host, port)?;
        let raw = TcpStream::connect_timeout(&addr, self.inner.timeout)
            .map_err(|e| format!("session connect failed to {}:{}: {}", host, port, e))?;
        let _ = raw.set_read_timeout(Some(self.inner.timeout));
        let _ = raw.set_write_timeout(Some(self.inner.timeout));

        self.inner.channel = if use_ssl {
            let connector = build_tls_connector(self.inner.original_certificate_policy)?;
            let tls = connector
                .connect(TLS_TARGET_HOST, raw)
                .map_err(|e| format!("session TLS handshake failed: {}", e))?;
            Some(TransportChannel::Tls(tls))
        } else {
            Some(TransportChannel::Plain(raw))
        };

        if !http_hostname.is_empty() {
            let preface = http_service_preface(http_hostname, true);
            if let Some(ref mut ch) = self.inner.channel {
                ch.write_all(&preface)
                    .map_err(|e| format!("failed sending session preface: {}", e))?;
            }
            let deadline = Instant::now() + self.inner.timeout;
            while find_crlf_crlf(&self.inner.buffer).is_none() {
                if self.inner.buffer.len() >= 16384 {
                    self.close();
                    return Err("HTTP header exceeds limit".to_string());
                }
                if let Err(e) = self.inner.receive_chunk(deadline) {
                    self.close();
                    return Err(e);
                }
            }
            let end = find_crlf_crlf(&self.inner.buffer).unwrap() + 4;
            self.inner.response_header = self.inner.buffer[..end].to_vec();
            self.inner.buffer.drain(..end);
            if !self.inner.response_header.starts_with(b"HTTP/1.1 200 ") {
                self.close();
                return Err("session tunnel did not return HTTP 200".to_string());
            }
        }
        Ok(self.inner.response_header.clone())
    }

    pub fn send_relay(
        &mut self,
        payload: &[u8],
        target_ip: &str,
        target_port: u16,
        unicode_bytes: bool,
    ) -> Result<(), String> {
        let ch = self
            .inner
            .channel
            .as_mut()
            .ok_or_else(|| "not connected".to_string())?;
        let frame = encode_relay_frame(
            payload,
            target_ip,
            target_port,
            if unicode_bytes { 1 } else { 0 },
        )?;
        if frame.len().saturating_sub(4) > self.inner.max_frame {
            return Err("outbound frame exceeds limit".to_string());
        }
        ch.write_all(&frame)
            .map_err(|e| format!("send_relay write failed: {}", e))
    }

    pub fn receive_body(&mut self) -> Result<Vec<u8>, String> {
        self.inner.receive_body()
    }

    pub fn close(&mut self) {
        self.inner.close();
    }
}

fn resolve_addr(host: &str, port: u16) -> Result<SocketAddr, String> {
    let target = format!("{}:{}", host, port);
    target
        .to_socket_addrs()
        .map_err(|e| format!("DNS resolution failed for {}: {}", target, e))?
        .next()
        .ok_or_else(|| format!("no address resolved for {}", target))
}

fn build_tls_connector(original_certificate_policy: bool) -> Result<TlsConnector, String> {
    let mut builder = TlsConnector::builder();
    if original_certificate_policy {
        builder.danger_accept_invalid_certs(true);
        builder.danger_accept_invalid_hostnames(true);
    }
    builder
        .build()
        .map_err(|e| format!("TLS connector build failed: {}", e))
}

fn find_crlf_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub state: String,
    pub status: String,
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    pub local_id: String,
    pub partner_id: String,
    pub route_mode: String,
    pub remote_endpoint: String,
    pub screen_width: u32,
    pub screen_height: u32,
    pub frames_received: usize,
    pub rectangles_applied: usize,
    pub audio_enabled: bool,
    pub audio_chunks_received: u64,
    pub audio_bytes_decoded: u64,
    pub clipboard_local: String,
    pub clipboard_remote: String,
    pub clipboard_auto_sync: bool,
    pub clipboard_updates_sent: u64,
    pub clipboard_updates_received: u64,
    pub file_transfers: Vec<FileTransferEntry>,
    pub chat_messages: Vec<String>,
    pub last_message: String,
    pub error: Option<String>,
    pub updated_at: f64,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            state: "disconnected".to_string(),
            status: "disconnected".to_string(),
            frames: 0,
            width: 1024,
            height: 768,
            local_id: String::new(),
            partner_id: String::new(),
            route_mode: String::new(),
            remote_endpoint: String::new(),
            screen_width: 1024,
            screen_height: 768,
            frames_received: 0,
            rectangles_applied: 0,
            audio_enabled: true,
            audio_chunks_received: 0,
            audio_bytes_decoded: 0,
            clipboard_local: String::new(),
            clipboard_remote: String::new(),
            clipboard_auto_sync: true,
            clipboard_updates_sent: 0,
            clipboard_updates_received: 0,
            file_transfers: Vec::new(),
            chat_messages: Vec::new(),
            last_message: "Ready to connect".to_string(),
            error: None,
            updated_at: now_epoch_secs(),
        }
    }
}

pub struct ControlSessionInner {
    pub state: SessionState,
    pub registration_profile: RegistrationProfile,
    pub default_partner: PartnerProfile,
    pub dictionary: Vec<u8>,
    pub screen: RGB555Screen,
    pub frames_applied: usize,
    pub rectangles_applied: usize,
    pub latest_png: Vec<u8>,
    pub audio: AudioBuffer,
    pub clipboard: ClipboardState,
    pub files: FileTransferManager,
    pub registration: Option<RegistrationTransport>,
    pub relay: Option<SessionTransport>,
    pub route: Option<TargetRoute>,
    pub login_acknowledged: bool,
    pub outbound_queue: Vec<(Vec<u8>, Option<InputEvent>)>,
}

#[derive(Clone)]
pub struct ControlSession {
    pub inner: Arc<Mutex<ControlSessionInner>>,
    pub stop_flag: Arc<AtomicBool>,
}

impl Default for ControlSession {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlSession {
    pub fn new() -> Self {
        Self::with_profiles(
            RegistrationProfile::default(),
            PartnerProfile::default(),
            DEFAULT_ZSTD_DICT.to_vec(),
        )
    }

    pub fn with_profiles(
        registration_profile: RegistrationProfile,
        default_partner: PartnerProfile,
        dictionary: Vec<u8>,
    ) -> Self {
        let decoder = ZstdDictionaryDecoder::new(dictionary.clone(), 64 * 1024 * 1024);
        let screen = RGB555Screen::new((1024, 768), decoder, 1000, 32);
        Self {
            inner: Arc::new(Mutex::new(ControlSessionInner {
                state: SessionState::default(),
                registration_profile,
                default_partner,
                dictionary,
                screen,
                frames_applied: 0,
                rectangles_applied: 0,
                latest_png: Vec::new(),
                audio: AudioBuffer::default(),
                clipboard: ClipboardState::default(),
                files: FileTransferManager::default(),
                registration: None,
                relay: None,
                route: None,
                login_acknowledged: false,
                outbound_queue: Vec::new(),
            })),
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn snapshot(&self) -> SessionState {
        let mut guard = self.inner.lock().unwrap();
        Self::sync_state_fields(&mut guard);
        guard.state.clone()
    }

    pub fn png_frame(&self) -> Result<Vec<u8>, String> {
        let guard = self.inner.lock().unwrap();
        if !guard.latest_png.is_empty() {
            return Ok(guard.latest_png.clone());
        }
        guard.screen.to_png()
    }

    pub fn audio_wav_since(&self, after_seq: u64) -> (u64, Vec<u8>) {
        let guard = self.inner.lock().unwrap();
        guard.audio.wav_since(after_seq)
    }

    pub fn downloaded_file_bytes(&self, file_id: &str) -> Option<(String, Vec<u8>)> {
        let guard = self.inner.lock().unwrap();
        let entry = guard.files.transfers.get(file_id)?;
        Some((entry.file_name.clone(), entry.buffer.clone()))
    }

    fn sync_state_fields(inner: &mut ControlSessionInner) {
        inner.state.status = inner.state.state.clone();
        inner.state.width = inner.screen.canvas.width;
        inner.state.height = inner.screen.canvas.height;
        inner.state.screen_width = inner.screen.canvas.width;
        inner.state.screen_height = inner.screen.canvas.height;
        inner.state.frames = inner.frames_applied;
        inner.state.frames_received = inner.frames_applied;
        inner.state.rectangles_applied = inner.rectangles_applied;
        inner.state.audio_enabled = inner.audio.enabled;
        inner.state.audio_chunks_received = inner.audio.chunks_received;
        inner.state.audio_bytes_decoded = inner.audio.bytes_decoded;
        inner.state.clipboard_local = inner.clipboard.local_text.clone();
        inner.state.clipboard_remote = inner.clipboard.remote_text.clone();
        inner.state.clipboard_auto_sync = inner.clipboard.auto_sync_system;
        inner.state.clipboard_updates_sent = inner.clipboard.updates_sent;
        inner.state.clipboard_updates_received = inner.clipboard.updates_received;
        inner.state.file_transfers = inner.files.transfers.values().cloned().collect();
        inner.state.chat_messages = inner.files.chat_messages.clone();
        inner.state.updated_at = now_epoch_secs();
    }

    fn dispatch_payload(
        inner: &mut ControlSessionInner,
        payload: &[u8],
        event: Option<InputEvent>,
    ) -> Result<(), String> {
        if let (Some(ref mut relay), Some(ref route)) = (&mut inner.relay, &inner.route) {
            relay.send_relay(payload, &route.public_ip, route.tcp_port, false)
        } else {
            inner.outbound_queue.push((payload.to_vec(), event));
            Ok(())
        }
    }

    pub fn handle_session_payload(&self, payload: &[u8]) {
        if payload.is_empty() {
            return;
        }
        let mut guard = self.inner.lock().unwrap();

        if let Ok(fields) = decode_byte_array_fields(payload) {
            if !fields.is_empty() && !fields[0].is_empty() {
                let kind = fields[0][0];

                // Kind 0: Login response with desktop dimensions
                if kind == 0 && fields[0].len() == 1 {
                    if let Ok((w, h)) = desktop_size_from_login(&fields) {
                        let decoder =
                            ZstdDictionaryDecoder::new(guard.dictionary.clone(), 64 * 1024 * 1024);
                        guard.screen = RGB555Screen::new((w, h), decoder, 1000, 32);
                        guard.login_acknowledged = true;
                        let _ = Self::dispatch_payload(&mut guard, b"17", None);
                        guard.state.state = "connected".to_string();
                        guard.state.last_message = format!("Remote desktop size {}x{}", w, h);
                        Self::sync_state_fields(&mut guard);
                        return;
                    }
                }

                // Kind 1: 11-field screen update packet
                if kind == 1 && fields[0].len() == 1 && fields.len() == 11 {
                    if let Ok(applied) = guard.screen.apply(&fields) {
                        guard.frames_applied += 1;
                        guard.rectangles_applied += applied;
                        if let Ok(png) = guard.screen.to_png() {
                            guard.latest_png = png;
                        }
                        let _ = Self::dispatch_payload(&mut guard, b"18", None);
                        guard.state.last_message = format!(
                            "Frame #{} ({} rects)",
                            guard.frames_applied, applied
                        );
                        Self::sync_state_fields(&mut guard);
                        return;
                    }
                }

                // Kind 3: Keepalive ping -> reply with |||4<int32>
                if kind == 3 && fields[0].len() == 1 && fields.len() > 1 && fields[1].len() == 4 {
                    let val = i32::from_le_bytes([
                        fields[1][0],
                        fields[1][1],
                        fields[1][2],
                        fields[1][3],
                    ]);
                    let reply = format!("|||4{}", val).into_bytes();
                    let _ = Self::dispatch_payload(&mut guard, &reply, None);
                    return;
                }
            }
        }

        // Audio packet (kinds 42, 43, 50, 51)
        if let Ok(Some(audio_pkt)) = guard.audio.ingest_packet(payload) {
            guard.state.last_message = format!(
                "Audio packet kind={} ({} PCM bytes)",
                audio_pkt.kind,
                audio_pkt.pcm_s16le.len()
            );
            Self::sync_state_fields(&mut guard);
            return;
        }

        // Clipboard packet (kind 33 / |||33|||)
        if let Some(clip_msg) = decode_clipboard_packet(payload) {
            guard.clipboard.remote_text = clip_msg.text.clone();
            guard.clipboard.updates_received += 1;
            if guard.clipboard.auto_sync_system {
                let _ = set_linux_system_clipboard(&clip_msg.text);
            }
            guard.state.last_message =
                format!("Clipboard synced from remote ({} chars)", clip_msg.text.len());
            Self::sync_state_fields(&mut guard);
            return;
        }

        // File Transfer & Chat packets (kinds 12, 13, 14, 15, 16, 21)
        if let Ok(Some((ft_pkt, replies))) = guard.files.handle_packet(payload) {
            guard.state.last_message = format!("File/chat event: {:?}", ft_pkt);
            for reply in replies {
                let _ = Self::dispatch_payload(&mut guard, &reply, None);
            }
            Self::sync_state_fields(&mut guard);
            return;
        }

        Self::sync_state_fields(&mut guard);
    }

    pub fn send_input(&self, event: InputEvent) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap();
        let payload = event.to_wire()?;
        Self::dispatch_payload(&mut guard, &payload, Some(event))?;
        Ok(payload)
    }

    pub fn toggle_remote_sound(&self, enable: bool) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap();
        guard.audio.enabled = enable;
        let payload = encode_sound_toggle_command(enable);
        Self::dispatch_payload(&mut guard, &payload, None)?;
        guard.state.last_message = if enable {
            "Requested remote computer sound ON (|||31|||)".to_string()
        } else {
            "Requested remote computer sound OFF (|||32|||)".to_string()
        };
        Self::sync_state_fields(&mut guard);
        Ok(payload)
    }

    pub fn send_clipboard(&self, text: &str, sync_system: bool) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap();
        guard.clipboard.local_text = text.to_string();
        guard.clipboard.updates_sent += 1;
        if sync_system {
            let _ = set_linux_system_clipboard(text);
        }
        let payload = encode_clipboard_packet(0, text)?;
        Self::dispatch_payload(&mut guard, &payload, None)?;
        guard.state.last_message = format!("Sent clipboard to remote ({} chars)", text.len());
        Self::sync_state_fields(&mut guard);
        Ok(payload)
    }

    pub fn sync_from_system_clipboard(&self) -> Result<String, String> {
        let text = get_linux_system_clipboard().unwrap_or_default();
        self.send_clipboard(&text, false)?;
        Ok(text)
    }

    pub fn offer_file_bytes(&self, file_name: &str, data: Vec<u8>) -> Result<String, String> {
        let mut guard = self.inner.lock().unwrap();
        let (file_id, pkt) = guard.files.offer_file_bytes(file_name, data)?;
        Self::dispatch_payload(&mut guard, &pkt, None)?;
        guard.state.last_message = format!("Offered file {} ({})", file_name, file_id);
        Self::sync_state_fields(&mut guard);
        Ok(file_id)
    }

    pub fn offer_file_from_path(&self, path: &Path) -> Result<String, String> {
        let mut guard = self.inner.lock().unwrap();
        let (file_id, pkt) = guard.files.offer_file_from_path(path)?;
        Self::dispatch_payload(&mut guard, &pkt, None)?;
        guard.state.last_message = format!("Offered file {} ({})", path.display(), file_id);
        Self::sync_state_fields(&mut guard);
        Ok(file_id)
    }

    pub fn accept_file(&self, file_id: &str) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap();
        let pkt = guard.files.accept_incoming_offer(file_id)?;
        Self::dispatch_payload(&mut guard, &pkt, None)?;
        guard.state.last_message = format!("Accepted incoming file {}", file_id);
        Self::sync_state_fields(&mut guard);
        Ok(pkt)
    }

    pub fn cancel_file(&self, file_id: &str) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap();
        let pkt = guard.files.cancel_transfer(file_id)?;
        Self::dispatch_payload(&mut guard, &pkt, None)?;
        guard.state.last_message = format!("Cancelled file transfer {}", file_id);
        Self::sync_state_fields(&mut guard);
        Ok(pkt)
    }

    pub fn send_chat(&self, text: &str) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap();
        let modern = guard.files.modern_protocol;
        let pkt = encode_chat_message(text, modern)?;
        guard.files.chat_messages.push(format!("Me: {}", text));
        Self::dispatch_payload(&mut guard, &pkt, None)?;
        guard.state.last_message = format!("Sent chat message ({} chars)", text.len());
        Self::sync_state_fields(&mut guard);
        Ok(pkt)
    }

    pub fn start_connect(&self, partner_id: &str, password: &str) -> Result<SessionState, String> {
        self.disconnect();
        self.stop_flag.store(false, Ordering::SeqCst);

        let (reg_profile, mut partner_profile) = {
            let mut guard = self.inner.lock().unwrap();
            let mut p = guard.default_partner.clone();
            if !partner_id.trim().is_empty() {
                let cleaned: String = partner_id.chars().filter(|c| !c.is_whitespace()).collect();
                if p.request_terms.len() > 1 {
                    p.request_terms[1] = cleaned.clone();
                }
                guard.state.partner_id = cleaned;
            } else if p.request_terms.len() > 1 {
                guard.state.partner_id = p.request_terms[1].clone();
            }
            if !password.trim().is_empty() {
                p.password = password.trim().to_string();
            }
            guard.latest_png.clear();
            guard.frames_applied = 0;
            guard.rectangles_applied = 0;
            guard.outbound_queue.clear();
            guard.state.state = "connecting".to_string();
            guard.state.frames = 0;
            guard.state.error = None;
            guard.state.last_message =
                format!("Connecting to partner {}...", guard.state.partner_id);
            Self::sync_state_fields(&mut guard);
            (guard.registration_profile.clone(), p)
        };

        let session_clone = self.clone();
        thread::spawn(move || {
            session_clone.run_worker(reg_profile, &mut partner_profile);
        });

        Ok(self.snapshot())
    }

    pub fn disconnect(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        let mut guard = self.inner.lock().unwrap();
        if let Some(ref mut r) = guard.relay {
            r.close();
        }
        guard.relay = None;
        if let Some(ref mut reg) = guard.registration {
            reg.close();
        }
        guard.registration = None;
        guard.route = None;
        guard.login_acknowledged = false;
        guard.state.state = "disconnected".to_string();
        guard.state.last_message = "Disconnected".to_string();
        Self::sync_state_fields(&mut guard);
    }

    fn run_worker(&self, reg_profile: RegistrationProfile, partner: &mut PartnerProfile) {
        let mut held_keys = BTreeSet::<i32>::new();
        let mut held_buttons = BTreeSet::<String>::new();

        let connect_res = self.establish_relay_session(&reg_profile, partner);
        if let Err(err) = connect_res {
            let mut guard = self.inner.lock().unwrap();
            guard.state.state = "disconnected".to_string();
            guard.state.error = Some(err.clone());
            guard.state.last_message = err;
            Self::sync_state_fields(&mut guard);
            return;
        }

        while !self.stop_flag.load(Ordering::SeqCst) {
            {
                let mut guard = self.inner.lock().unwrap();
                let queued: Vec<(Vec<u8>, Option<InputEvent>)> =
                    guard.outbound_queue.drain(..).collect();
                for (payload, evt_opt) in queued {
                    let inner = &mut *guard;
                    if let (Some(ref mut relay), Some(ref route)) = (&mut inner.relay, &inner.route)
                    {
                        let _ =
                            relay.send_relay(&payload, &route.public_ip, route.tcp_port, false);
                    }
                    if let Some(evt) = evt_opt {
                        match evt.event_type.as_str() {
                            "key" => {
                                if evt.down {
                                    held_keys.insert(evt.vk);
                                } else {
                                    held_keys.remove(&evt.vk);
                                }
                            }
                            "button" => {
                                if evt.down {
                                    held_buttons.insert(evt.button.clone());
                                } else {
                                    held_buttons.remove(&evt.button);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            let body_res = {
                let mut guard = self.inner.lock().unwrap();
                if let Some(ref mut relay) = guard.relay {
                    relay.receive_body()
                } else {
                    break;
                }
            };

            match body_res {
                Ok(body) => match decode_transport_body(&body) {
                    Ok(Some(pkt)) if pkt.kind == "relay" => {
                        self.handle_session_payload(&pkt.data);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        let mut guard = self.inner.lock().unwrap();
                        guard.state.error = Some(e);
                        break;
                    }
                },
                Err(err) => {
                    if err.starts_with("TimeoutError") {
                        continue;
                    }
                    if !self.stop_flag.load(Ordering::SeqCst) {
                        let mut guard = self.inner.lock().unwrap();
                        guard.state.error = Some(err.clone());
                        guard.state.last_message = err;
                        Self::sync_state_fields(&mut guard);
                    }
                    break;
                }
            }
        }

        {
            let mut guard = self.inner.lock().unwrap();
            let inner = &mut *guard;
            if let (Some(ref mut relay), Some(ref route)) = (&mut inner.relay, &inner.route) {
                for vk in held_keys {
                    if let Ok(pkt) = keyboard(vk, false, false) {
                        let _ = relay.send_relay(&pkt, &route.public_ip, route.tcp_port, false);
                    }
                }
                for button in held_buttons {
                    if let Ok(pkt) = mouse_button(&button, false) {
                        let _ = relay.send_relay(&pkt, &route.public_ip, route.tcp_port, false);
                    }
                }
            }
            if let Some(ref mut r) = guard.relay {
                r.close();
            }
            guard.relay = None;
            if let Some(ref mut reg) = guard.registration {
                reg.close();
            }
            guard.registration = None;
            guard.route = None;
            guard.state.state = "disconnected".to_string();
            Self::sync_state_fields(&mut guard);
        }
    }

    fn establish_relay_session(
        &self,
        reg_profile: &RegistrationProfile,
        partner: &PartnerProfile,
    ) -> Result<(), String> {
        let mut terms = partner.request_terms.clone();
        if terms.len() != 17 {
            return Err("partner request needs seventeen terms".to_string());
        }

        let mut reg = RegistrationTransport::new(8.0, 1024 * 1024, true)?;
        reg.connect(DEFAULT_REGISTRATION_HOST, 443)?;
        let reg_cmd = build_registration_command(&reg_profile.terms)?;
        reg.send_command(&reg_cmd)?;
        let identity = reg.receive_identity()?;

        terms[2] = connection_request_password_value(&partner.password, partner.password_type);
        terms[6] = identity.client_id.to_string();
        let hash_input = format!(
            "{}{}",
            terms[6],
            reg_profile.terms.get(2).cloned().unwrap_or_default()
        );
        terms[7] = md5_hex_lower(hash_input.as_bytes());

        let req_cmd = build_connection_request_command(&terms)?;
        reg.send_command(&req_cmd)?;

        let mut route_opt: Option<TargetRoute> = None;
        for _ in 0..4 {
            let response = reg.receive_mode1_payload()?;
            if response.starts_with(b"8:") {
                let text = String::from_utf8_lossy(&response);
                let parts: Vec<&str> = text.split(':').collect();
                if parts.len() >= 2 && parts[1] != "1" {
                    let status = parts[1];
                    let msg = match status {
                        "2" => "Sai mật khẩu (Wrong Password)".to_string(),
                        "11" => "Đối tác không online hoặc ID không tồn tại".to_string(),
                        "99" => "Không thể kết nối tới đối tác (Status 99)".to_string(),
                        _ => format!("Lỗi kết nối từ máy chủ (Mã: {})", status),
                    };
                    return Err(msg);
                }
                route_opt = Some(decode_target_route(&response)?);
                break;
            }
        }

        let route = route_opt
            .ok_or_else(|| "Không nhận được thông tin định tuyến từ đối tác".to_string())?;

        let mut relay = SessionTransport::new(8.0, 1024 * 1024, true)?;
        relay.connect(&route.tcp_server_ip, 443, &route.tcp_server_hostname, true)?;

        let fields = vec![
            "".to_string(),
            "".to_string(),
            "1280".to_string(),
            "720".to_string(),
            "30".to_string(),
            "15".to_string(),
            "15".to_string(),
            "0".to_string(),
            "0".to_string(),
            identity.client_id.to_string(),
            local_hostname(),
            "0".to_string(),
            "".to_string(),
            "".to_string(),
            random_digits(16),
            "2".to_string(),
            "80".to_string(),
            "0".to_string(),
            "0".to_string(),
            route.partner_secret_token.clone().unwrap_or_default(),
            "6.6.133".to_string(),
            "0".to_string(),
        ];
        let salt = random_digits(8);
        let login_payload = build_login_with_password(
            &fields,
            &partner.password,
            partner.password_type,
            Some(&salt),
        )?;
        relay.send_relay(&login_payload, &route.public_ip, route.tcp_port, true)?;
        relay.set_timeout(0.15);

        let mut guard = self.inner.lock().unwrap();
        let partner_ver = partner
            .request_terms
            .get(3)
            .map(|s| s.as_str())
            .unwrap_or("6.6.133");
        guard.files.modern_protocol =
            version_equal_or_greater(partner_ver, "6.4.25").unwrap_or(true);
        let decoder = ZstdDictionaryDecoder::new(guard.dictionary.clone(), 64 * 1024 * 1024);
        guard.screen = RGB555Screen::new((1024, 768), decoder, 1000, 32);
        guard.state.local_id = identity.client_id.to_string();
        guard.state.route_mode = "relay".to_string();
        guard.state.remote_endpoint = format!("{}:{}", route.public_ip, route.tcp_port);
        guard.registration = Some(reg);
        guard.relay = Some(relay);
        guard.route = Some(route);
        Self::sync_state_fields(&mut guard);
        Ok(())
    }

    pub fn move_pointer(&self, x: i32, y: i32) -> Result<Vec<u8>, String> {
        let payload = mouse_move(x, y)?;
        let mut guard = self.inner.lock().unwrap();
        Self::dispatch_payload(&mut guard, &payload, None)?;
        Ok(payload)
    }
}

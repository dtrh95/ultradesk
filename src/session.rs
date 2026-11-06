use crate::audio::{encode_sound_toggle_command, AudioBuffer};
use crate::clipboard::{
    clipboard_text_eq, decode_clipboard_packet, encode_clipboard_for_remote,
    get_linux_system_clipboard, set_linux_system_clipboard, ClipboardState,
};
use crate::cursor::{decode_cursor_parts, CursorManager};
use crate::file_transfer::{
    encode_chat_message, is_utf16_string_packet, FileTransferEntry, FileTransferManager,
};
use crate::host::{DesktopBackend, HostSession, HostStatus};
use crate::input::{
    encode_unicode_text_packets, keyboard, mouse_button, mouse_move, normalize_vk, InputEvent,
};
use crate::profile::{ClientProfile, HostProfile, ProfileManager};
use crate::protocol::{
    build_connection_request_command, build_login_with_password, build_registration_command,
    connection_request_password_value, decode_byte_array_fields, decode_target_route,
    decode_transport_body, encode_byte_array_fields, encode_relay_frame, encode_transport_frame,
    encrypt_registration_bytes, extract_command3_identity, http_service_preface, md5_hex_lower,
    Command3Identity, TargetRoute, REGISTRATION_PASSWORD,
};
use crate::screen::{
    desktop_size_from_login, DisplayLayout, MonitorSelection, RGB555Screen, ZstdDictionaryDecoder,
};
use crate::server::encode_base64;
use native_tls::{TlsConnector, TlsStream};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
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
            }
            Self::Tls(s) => {
                let _ = s.get_ref().set_read_timeout(timeout);
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

    pub fn set_timeout(&mut self, timeout_secs: f64) {
        self.timeout = Duration::from_secs_f64(timeout_secs.max(0.01));
    }

    pub fn connect(&mut self, host: &str, port: u16) -> Result<Vec<u8>, String> {
        self.connect_with_tls(host, port, true)
    }

    pub fn connect_with_tls(
        &mut self,
        host: &str,
        port: u16,
        use_tls: bool,
    ) -> Result<Vec<u8>, String> {
        if self.channel.is_some() {
            return Err("already connected".to_string());
        }
        let addr = resolve_addr(host, port)?;
        let raw = TcpStream::connect_timeout(&addr, self.timeout)
            .map_err(|e| format!("connect failed to {}:{}: {}", host, port, e))?;
        let _ = raw.set_read_timeout(Some(self.timeout));
        let _ = raw.set_write_timeout(Some(self.timeout));

        self.channel = if use_tls {
            let connector = build_tls_connector(self.original_certificate_policy)?;
            let tls = connector
                .connect(TLS_TARGET_HOST, raw)
                .map_err(|e| format!("TLS handshake failed: {}", e))?;
            Some(TransportChannel::Tls(tls))
        } else {
            Some(TransportChannel::Plain(raw))
        };

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

    pub fn send_system(&mut self, payload: &[u8], with_flag: bool) -> Result<(), String> {
        let ch = self
            .inner
            .channel
            .as_mut()
            .ok_or_else(|| "not connected".to_string())?;
        let mut body = Vec::with_capacity(2 + payload.len());
        body.push(1u8);
        if with_flag {
            body.push(0u8);
        }
        body.extend_from_slice(payload);
        let frame = encode_transport_frame(&body);
        if frame.len().saturating_sub(4) > self.inner.max_frame {
            return Err("outbound frame exceeds limit".to_string());
        }
        ch.write_all(&frame)
            .map_err(|e| format!("send_system write failed: {}", e))
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedRemoteEntry {
    pub partner_id: String,
    pub password: String,
    #[serde(default)]
    pub last_used: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCredentials {
    #[serde(default)]
    pub partner_id: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_remember_true")]
    pub remember: bool,
    #[serde(default)]
    pub saved_remotes: Vec<SavedRemoteEntry>,
    #[serde(default = "default_theme_mode")]
    pub theme: String,
    #[serde(default = "default_ui_language")]
    pub language: String,
    #[serde(default)]
    pub toolbar_collapsed: bool,
}

fn default_remember_true() -> bool {
    true
}

fn default_theme_mode() -> String {
    "auto-time".to_string()
}

fn default_ui_language() -> String {
    "en".to_string()
}

impl Default for SavedCredentials {
    fn default() -> Self {
        Self {
            partner_id: String::new(),
            password: String::new(),
            remember: true,
            saved_remotes: Vec::new(),
            theme: default_theme_mode(),
            language: default_ui_language(),
            toolbar_collapsed: false,
        }
    }
}

pub fn credentials_file_path() -> PathBuf {
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(x) => PathBuf::from(x),
        None => match std::env::var_os("HOME") {
            Some(h) => PathBuf::from(h).join(".config"),
            None => return std::env::temp_dir().join("ultradesk-credentials.json"),
        },
    };
    let new_path = base.join("ultradesk-rust").join("credentials.json");
    let legacy_path = base.join("ultraviewer-linux").join("credentials.json");
    if !new_path.exists() && legacy_path.exists() {
        if let Some(parent) = new_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::copy(&legacy_path, &new_path);
    }
    new_path
}

pub fn load_saved_credentials() -> SavedCredentials {
    let path = credentials_file_path();
    if let Ok(bytes) = fs::read(&path) {
        if let Ok(mut creds) = serde_json::from_slice::<SavedCredentials>(&bytes) {
            if creds.theme.trim().is_empty() {
                creds.theme = default_theme_mode();
            }
            if creds.language.trim().is_empty() {
                creds.language = default_ui_language();
            }
            if !creds.partner_id.is_empty()
                && !creds
                    .saved_remotes
                    .iter()
                    .any(|r| r.partner_id == creds.partner_id)
            {
                creds.saved_remotes.insert(
                    0,
                    SavedRemoteEntry {
                        partner_id: creds.partner_id.clone(),
                        password: creds.password.clone(),
                        last_used: now_epoch_secs() as u64,
                    },
                );
            }
            return creds;
        }
    }
    SavedCredentials::default()
}

pub fn save_credentials(creds: &SavedCredentials) -> Result<(), String> {
    let path = credentials_file_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let json = serde_json::to_vec_pretty(creds)
        .map_err(|e| format!("Failed to serialize credentials: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("Failed to write credentials: {}", e))
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
    #[serde(default)]
    pub saved_partner_id: String,
    #[serde(default)]
    pub saved_password: String,
    #[serde(default = "default_remember_true")]
    pub remember_credentials: bool,
    #[serde(default)]
    pub saved_remotes: Vec<SavedRemoteEntry>,
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
    #[serde(default = "default_theme_mode")]
    pub saved_theme: String,
    #[serde(default = "default_ui_language")]
    pub saved_language: String,
    #[serde(default)]
    pub saved_toolbar_collapsed: bool,
    #[serde(default)]
    pub frame_version: u64,
    #[serde(default)]
    pub monitors: Vec<(i32, i32, i32, i32)>,
    #[serde(default)]
    pub selected_monitor: Option<usize>,
    #[serde(default)]
    pub selected_monitors: Vec<usize>,
    #[serde(default)]
    pub cursor_x: i16,
    #[serde(default)]
    pub cursor_y: i16,
    #[serde(default)]
    pub cursor_hot_x: u16,
    #[serde(default)]
    pub cursor_hot_y: u16,
    #[serde(default)]
    pub cursor_width: u32,
    #[serde(default)]
    pub cursor_height: u32,
    #[serde(default)]
    pub cursor_hash: Option<i32>,
    #[serde(default)]
    pub cursor_png_base64: Option<String>,
    #[serde(default)]
    pub host_status: Option<HostStatus>,
    #[serde(default)]
    pub client_synthetic_mac: String,
    #[serde(default)]
    pub host_hardware_mac: String,
}

impl Default for SessionState {
    fn default() -> Self {
        let saved = load_saved_credentials();
        Self {
            state: "disconnected".to_string(),
            status: "disconnected".to_string(),
            frames: 0,
            width: 1024,
            height: 768,
            local_id: String::new(),
            partner_id: saved.partner_id.clone(),
            saved_partner_id: saved.partner_id,
            saved_password: saved.password,
            remember_credentials: saved.remember,
            saved_remotes: saved.saved_remotes,
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
            saved_theme: saved.theme,
            saved_language: saved.language,
            saved_toolbar_collapsed: saved.toolbar_collapsed,
            frame_version: 1,
            monitors: vec![(0, 0, 1024, 768)],
            selected_monitor: None,
            selected_monitors: vec![0],
            cursor_x: 0,
            cursor_y: 0,
            cursor_hot_x: 0,
            cursor_hot_y: 0,
            cursor_width: 0,
            cursor_height: 0,
            cursor_hash: None,
            cursor_png_base64: None,
            host_status: None,
            client_synthetic_mac: String::new(),
            host_hardware_mac: String::new(),
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
    pub cursor: CursorManager,
    pub cursor_png_cache: Option<(i32, String)>,
    pub host_session: HostSession,
    pub profile_manager: ProfileManager,
    pub client_profile: ClientProfile,
    pub host_profile: HostProfile,
    pub login_acknowledged: bool,
    pub outbound_queue: Vec<(Vec<u8>, bool, Option<InputEvent>)>,
}

#[derive(Clone)]
pub struct ControlSession {
    pub inner: Arc<Mutex<ControlSessionInner>>,
    pub stop_flag: Arc<AtomicBool>,
    pub generation: Arc<AtomicU64>,
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
        let profile_manager = ProfileManager::from_xdg().unwrap_or_else(|_| {
            ProfileManager::from_custom_dir(std::env::temp_dir().join("ultradesk-rust"))
        });
        let client_profile = profile_manager.load_or_create_client().unwrap_or_else(|_| {
            ClientProfile {
                device_id: "0123456789ABCDEF0123456789ABCDEF".to_string(),
                temp_password: "12345678".to_string(),
                display_name: "UltraDesk".to_string(),
                synthetic_mac: "0223456789AB".to_string(),
                local_ip: std::net::Ipv4Addr::new(127, 0, 0, 1),
                path: std::env::temp_dir().join("profile"),
            }
        });
        let host_profile = profile_manager.load_or_create_host().unwrap_or_else(|_| {
            HostProfile {
                device_id: "FEDCBA9876543210FEDCBA9876543210".to_string(),
                temp_password: "87654321".to_string(),
                display_name: "UltraDesk".to_string(),
                hardware_mac: "001122334455".to_string(),
                local_ip: std::net::Ipv4Addr::new(127, 0, 0, 1),
                master_password_proof: None,
                path: std::env::temp_dir().join("host-profile"),
            }
        });
        let host_session = HostSession::with_profile(
            &host_profile.device_id,
            "",
            &host_profile.hardware_mac,
            &host_profile.temp_password,
            host_profile.master_password_proof.clone(),
            &host_profile.display_name,
            screen.layout.clone(),
            DesktopBackend::auto_detect(),
        );
        let mut inner = ControlSessionInner {
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
            cursor: CursorManager::default(),
            cursor_png_cache: None,
            host_session,
            profile_manager,
            client_profile,
            host_profile,
            login_acknowledged: false,
            outbound_queue: Vec::new(),
        };
        Self::sync_state_fields(&mut inner);
        Self {
            inner: Arc::new(Mutex::new(inner)),
            stop_flag: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn snapshot(&self) -> SessionState {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::sync_state_fields(&mut guard);
        guard.state.clone()
    }

    pub fn frame_version(&self) -> u64 {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.screen.version
    }

    pub fn png_frame(&self) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.screen.get_or_encode_png().map(|(_, bytes)| bytes)
    }

    pub fn png_frame_versioned(&self) -> Result<(u64, Vec<u8>), String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.screen.get_or_encode_png()
    }

    pub fn jpeg_frame_versioned(&self, quality: u8) -> Result<(u64, Vec<u8>), String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.screen.get_or_encode_jpeg(quality)
    }

    pub fn bmp_frame_versioned(&self) -> (u64, Vec<u8>) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.screen.get_or_encode_bmp()
    }

    pub fn select_monitor(&self, monitor: Option<usize>) -> SessionState {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let sel = match monitor {
            None => MonitorSelection::All,
            Some(idx) => MonitorSelection::Index(idx),
        };
        guard.screen.set_monitor_selection(sel);
        Self::sync_state_fields(&mut guard);
        guard.state.clone()
    }

    pub fn select_monitors(&self, indices: Option<&[usize]>) -> SessionState {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let total = guard.screen.layout.monitors.len();
        let sel = MonitorSelection::from_indices(indices, total);
        guard.screen.set_monitor_selection(sel);
        Self::sync_state_fields(&mut guard);
        guard.state.clone()
    }

    pub fn host_select_monitors(
        &self,
        index: Option<usize>,
        indices: Option<&[usize]>,
    ) -> HostStatus {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let total = guard.host_session.status().monitors.len();
        let sel = if indices.is_some() {
            MonitorSelection::from_indices(indices, total)
        } else {
            match index {
                None => MonitorSelection::All,
                Some(idx) => MonitorSelection::Index(idx),
            }
        };
        let st = guard.host_session.select_monitors(sel);
        Self::sync_state_fields(&mut guard);
        st
    }

    pub fn send_text(&self, text: &str, caps_lock: bool) -> Result<usize, String> {
        let packets = encode_unicode_text_packets(text, caps_lock);
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let count = packets.len();
        for pkt in packets {
            Self::dispatch_payload(&mut guard, &pkt, false, None)?;
        }
        guard.state.last_message = format!("Sent Unicode text ({} chars)", text.chars().count());
        Self::sync_state_fields(&mut guard);
        Ok(count)
    }

    pub fn host_status(&self) -> HostStatus {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.host_session.status()
    }

    pub fn host_start(&self) -> HostStatus {
        let hs = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.host_session.clone()
        };
        let st = hs.start();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::sync_state_fields(&mut guard);
        st
    }

    pub fn host_stop(&self) -> HostStatus {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let _ = guard.host_session.stop();
        Self::sync_state_fields(&mut guard);
        guard.host_session.status()
    }

    pub fn host_approve(&self, allow_control: bool) -> Result<HostStatus, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let _ = guard.host_session.approve(allow_control)?;
        Self::sync_state_fields(&mut guard);
        Ok(guard.host_session.status())
    }

    pub fn host_decline(&self) -> Result<HostStatus, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let _ = guard.host_session.decline()?;
        Self::sync_state_fields(&mut guard);
        Ok(guard.host_session.status())
    }

    pub fn host_set_control(&self, allow_control: bool) -> Result<HostStatus, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let _ = guard.host_session.set_control_allowed(allow_control)?;
        Self::sync_state_fields(&mut guard);
        Ok(guard.host_session.status())
    }

    pub fn host_set_master_password(&self, password: Option<&str>) -> Result<HostStatus, String> {
        let hs = {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let cleaned = password.map(|s| s.trim()).filter(|s| !s.is_empty());
            guard.profile_manager.set_master_password(cleaned)?;
            let proof = guard.profile_manager.master_password_proof()?;
            guard.host_profile.master_password_proof = proof.clone();
            guard.host_session.set_master_password_proof(proof);
            guard.host_session.clone()
        };
        if hs.status().running {
            let _ = hs.stop();
            let _ = hs.start();
        }
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::sync_state_fields(&mut guard);
        Ok(guard.host_session.status())
    }

    pub fn audio_wav_since(&self, after_seq: u64) -> (u64, Vec<u8>) {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.audio.wav_since(after_seq)
    }

    pub fn downloaded_file_bytes(&self, file_id: &str) -> Option<(String, Vec<u8>)> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let entry = guard.files.transfers.get(file_id)?;
        Some((entry.file_name.clone(), entry.buffer.clone()))
    }

    fn sync_state_fields(inner: &mut ControlSessionInner) {
        let view = inner.screen.active_view_buffer();
        let view_w = view.width;
        let view_h = view.height;
        drop(view);
        inner.state.status = inner.state.state.clone();
        inner.state.width = view_w;
        inner.state.height = view_h;
        inner.state.screen_width = inner.screen.canvas.width;
        inner.state.screen_height = inner.screen.canvas.height;
        inner.state.frames = inner.frames_applied;
        inner.state.frames_received = inner.frames_applied;
        inner.state.rectangles_applied = inner.rectangles_applied;
        inner.state.frame_version = inner.screen.version;
        inner.state.monitors = inner
            .screen
            .layout
            .monitors
            .iter()
            .map(|m| (m.left, m.top, m.right, m.bottom))
            .collect();
        let total_mons = inner.screen.layout.monitors.len();
        let selected_indices = inner
            .screen
            .layout
            .selected_indices(&inner.screen.monitor_selection);
        inner.state.selected_monitors = selected_indices.clone();
        inner.state.selected_monitor = match &inner.screen.monitor_selection {
            MonitorSelection::Index(i) if *i < total_mons => Some(*i),
            _ if selected_indices.len() == 1 && total_mons > 1 => Some(selected_indices[0]),
            _ => None,
        };
        let bounds = inner
            .screen
            .layout
            .active_bounds(&inner.screen.monitor_selection);
        let (cx, cy) = inner.cursor.position();
        inner.state.cursor_x =
            (cx as i32 - bounds.left).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        inner.state.cursor_y =
            (cy as i32 - bounds.top).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        inner.state.cursor_hash = inner.cursor.active_hash();
        if let Some(cur) = inner.cursor.current_cursor() {
            inner.state.cursor_hot_x = cur.hot_x;
            inner.state.cursor_hot_y = cur.hot_y;
            inner.state.cursor_width = cur.width;
            inner.state.cursor_height = cur.height;
            let active_h = inner.cursor.active_hash().unwrap_or(0);
            let cached_b64 = match &inner.cursor_png_cache {
                Some((h, b64)) if *h == active_h => Some(b64.clone()),
                _ => cur.to_png().ok().map(|bytes| encode_base64(&bytes)),
            };
            if let Some(ref b64) = cached_b64 {
                inner.cursor_png_cache = Some((active_h, b64.clone()));
            }
            inner.state.cursor_png_base64 = cached_b64;
        }
        let (host_chats, host_clip) = inner.host_session.drain_auxiliary_events();
        if !host_chats.is_empty() {
            inner.files.chat_messages.extend(host_chats);
        }
        if let Some(clip_text) = host_clip {
            inner.clipboard.remote_text = clip_text.clone();
            inner.clipboard.local_text = clip_text;
            inner.clipboard.updates_received += 1;
        }
        inner.state.host_status = Some(inner.host_session.status());
        inner.state.client_synthetic_mac = inner.client_profile.synthetic_mac.clone();
        inner.state.host_hardware_mac = inner.host_profile.hardware_mac.clone();
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
        unicode_bytes: bool,
        event: Option<InputEvent>,
    ) -> Result<(), String> {
        let is_move = event
            .as_ref()
            .map(|e| e.event_type == "move")
            .unwrap_or(false);
        if is_move {
            if let Some(last) = inner.outbound_queue.last_mut() {
                if last
                    .2
                    .as_ref()
                    .map(|e| e.event_type == "move")
                    .unwrap_or(false)
                {
                    *last = (payload.to_vec(), unicode_bytes, event);
                    return Ok(());
                }
            }
        }
        inner
            .outbound_queue
            .push((payload.to_vec(), unicode_bytes, event));
        Ok(())
    }

    pub fn handle_session_payload(&self, payload: &[u8]) {
        if payload.is_empty() {
            return;
        }
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        if let Ok(fields) = decode_byte_array_fields(payload) {
            if !fields.is_empty() && !fields[0].is_empty() {
                let kind = fields[0][0];

                if kind == 0 {
                    if let Ok((w, h)) = desktop_size_from_login(&fields) {
                        let parsed_bs = if fields.len() >= 15 && fields[14].len() == 1 {
                            fields[14][0] as i32
                        } else if fields.len() >= 15 && fields[14].len() == 2 {
                            i16::from_le_bytes([fields[14][0], fields[14][1]]) as i32
                        } else if fields.len() >= 15 && fields[14].len() == 4 {
                            i32::from_le_bytes([
                                fields[14][0],
                                fields[14][1],
                                fields[14][2],
                                fields[14][3],
                            ])
                        } else {
                            32
                        };
                        let effective_bs = if (16..=256).contains(&parsed_bs) {
                            parsed_bs
                        } else {
                            32
                        };
                        let prev_version = guard.screen.version;
                        let decoder =
                            ZstdDictionaryDecoder::new(guard.dictionary.clone(), 64 * 1024 * 1024);
                        let mut new_screen = RGB555Screen::new((w, h), decoder, 1000, effective_bs);
                        new_screen.version = prev_version.wrapping_add(1);
                        if let Some(layout) = DisplayLayout::parse_from_login(&fields) {
                            new_screen.layout = layout;
                        }
                        guard.screen = new_screen;
                        guard.login_acknowledged = true;
                        if guard.clipboard.auto_sync_system {
                            if let Some(sys_clip) = get_linux_system_clipboard() {
                                guard.clipboard.local_text = sys_clip;
                            }
                        }
                        if let Ok(pkt48) = encode_byte_array_fields(&[
                            &[48u8][..],
                            &[effective_bs as u8][..],
                        ]) {
                            let _ = Self::dispatch_payload(&mut guard, &pkt48, false, None);
                        }
                        let _ = Self::dispatch_payload(&mut guard, b"17", false, None);
                        guard.state.state = "connected".to_string();
                        guard.state.last_message = format!("Remote desktop size {}x{}", w, h);
                        Self::sync_state_fields(&mut guard);
                        return;
                    }
                }

                if kind == 48 && fields.len() > 1 {
                    let block_size = if fields[1].len() == 1 {
                        fields[1][0] as i32
                    } else if fields[1].len() == 2 {
                        i16::from_le_bytes([fields[1][0], fields[1][1]]) as i32
                    } else if fields[1].len() == 4 {
                        i32::from_le_bytes([
                            fields[1][0],
                            fields[1][1],
                            fields[1][2],
                            fields[1][3],
                        ])
                    } else {
                        0
                    };
                    if block_size > 0 {
                        guard.screen.block_size = block_size;
                    }
                    return;
                }

                if kind == 1 && fields.len() == 11 {
                    match guard.screen.apply(&fields) {
                        Ok(applied) => {
                            guard.frames_applied += 1;
                            guard.rectangles_applied += applied;
                            let _ = Self::dispatch_payload(&mut guard, b"18", false, None);
                            guard.state.last_message = format!(
                                "Frame #{} ({} rects)",
                                guard.frames_applied, applied
                            );
                            Self::sync_state_fields(&mut guard);
                            return;
                        }
                        Err(err) => {
                            let _ = Self::dispatch_payload(&mut guard, b"18", false, None);
                            guard.state.last_message = format!("Frame decode warning: {}", err);
                            Self::sync_state_fields(&mut guard);
                            return;
                        }
                    }
                }

                if matches!(kind, 3 | 4 | 28) {
                    if let Ok(cmd) = decode_cursor_parts(&fields) {
                        if let Some(reply) = guard.cursor.handle_command(cmd) {
                            let _ = Self::dispatch_payload(&mut guard, &reply, false, None);
                        }
                        Self::sync_state_fields(&mut guard);
                        return;
                    }
                }
            }
        }

        if let Ok(Some(audio_pkt)) = guard.audio.ingest_packet(payload) {
            guard.state.last_message = format!(
                "Audio packet kind={} ({} PCM bytes)",
                audio_pkt.kind,
                audio_pkt.pcm_s16le.len()
            );
            Self::sync_state_fields(&mut guard);
            return;
        }

        if let Some(clip_msg) = decode_clipboard_packet(payload) {
            guard.clipboard.remote_text = clip_msg.text.clone();
            guard.clipboard.local_text = clip_msg.text.clone();
            guard.clipboard.updates_received += 1;
            if guard.clipboard.auto_sync_system {
                let _ = set_linux_system_clipboard(&clip_msg.text);
            }
            guard.state.last_message =
                format!("Clipboard synced from remote ({} chars)", clip_msg.text.len());
            Self::sync_state_fields(&mut guard);
            return;
        }

        if let Ok(Some((ft_pkt, replies))) = guard.files.handle_packet(payload) {
            if matches!(ft_pkt, crate::file_transfer::FileTransferPacket::ChatMessage { .. }) {
                if let Some(last) = guard.files.chat_messages.last_mut() {
                    if !last.starts_with("Partner: ") && !last.starts_with("Me: ") {
                        *last = format!("Partner: {}", last);
                    }
                }
            }
            guard.state.last_message = format!("File/chat event: {:?}", ft_pkt);
            for reply in replies {
                let is_utf16 = is_utf16_string_packet(&reply);
                let _ = Self::dispatch_payload(&mut guard, &reply, is_utf16, None);
            }
            Self::sync_state_fields(&mut guard);
            return;
        }

        Self::sync_state_fields(&mut guard);
    }

    pub fn send_input(&self, mut event: InputEvent) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if event.event_type == "move" {
            let bounds = guard
                .screen
                .layout
                .active_bounds(&guard.screen.monitor_selection);
            event.x = event.x.saturating_add(bounds.left);
            event.y = event.y.saturating_add(bounds.top);
            let _ = guard
                .cursor
                .handle_command(crate::cursor::CursorCommand::Position {
                    x: event.x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                    y: event.y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                });
            Self::sync_state_fields(&mut guard);
        }
        let payload = event.to_wire()?;
        Self::dispatch_payload(&mut guard, &payload, false, Some(event))?;
        Ok(payload)
    }

    pub fn toggle_remote_sound(&self, enable: bool) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.audio.enabled = enable;
        let payload = encode_sound_toggle_command(enable);
        Self::dispatch_payload(&mut guard, &payload, false, None)?;
        guard.state.last_message = if enable {
            "Requested remote computer sound ON (|||31|||)".to_string()
        } else {
            "Requested remote computer sound OFF (|||32|||)".to_string()
        };
        Self::sync_state_fields(&mut guard);
        Ok(payload)
    }

    pub fn send_clipboard(&self, text: &str, sync_system: bool) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.clipboard.local_text = text.to_string();
        guard.clipboard.updates_sent += 1;
        if sync_system {
            let _ = set_linux_system_clipboard(text);
        }
        let _ = guard.host_session.send_clipboard(text);
        let (payload, unicode_bytes) = encode_clipboard_for_remote(text)?;
        Self::dispatch_payload(&mut guard, &payload, unicode_bytes, None)?;
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
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (file_id, pkt) = guard.files.offer_file_bytes(file_name, data)?;
        let is_utf16 = is_utf16_string_packet(&pkt);
        Self::dispatch_payload(&mut guard, &pkt, is_utf16, None)?;
        guard.state.last_message = format!("Offered file {} ({})", file_name, file_id);
        Self::sync_state_fields(&mut guard);
        Ok(file_id)
    }

    pub fn offer_file_from_path(&self, path: &Path) -> Result<String, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (file_id, pkt) = guard.files.offer_file_from_path(path)?;
        let is_utf16 = is_utf16_string_packet(&pkt);
        Self::dispatch_payload(&mut guard, &pkt, is_utf16, None)?;
        guard.state.last_message = format!("Offered file {} ({})", path.display(), file_id);
        Self::sync_state_fields(&mut guard);
        Ok(file_id)
    }

    pub fn accept_file(&self, file_id: &str) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let pkt = guard.files.accept_incoming_offer(file_id)?;
        let is_utf16 = is_utf16_string_packet(&pkt);
        Self::dispatch_payload(&mut guard, &pkt, is_utf16, None)?;
        guard.state.last_message = format!("Accepted incoming file {}", file_id);
        Self::sync_state_fields(&mut guard);
        Ok(pkt)
    }

    pub fn cancel_file(&self, file_id: &str) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let pkt = guard.files.cancel_transfer(file_id)?;
        let is_utf16 = is_utf16_string_packet(&pkt);
        Self::dispatch_payload(&mut guard, &pkt, is_utf16, None)?;
        guard.state.last_message = format!("Cancelled file transfer {}", file_id);
        Self::sync_state_fields(&mut guard);
        Ok(pkt)
    }

    pub fn send_chat(&self, text: &str) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let modern = guard.files.modern_protocol;
        let pkt = encode_chat_message(text, modern)?;
        let is_utf16 = is_utf16_string_packet(&pkt);
        guard.files.chat_messages.push(format!("Me: {}", text));
        let _ = guard.host_session.send_chat(text);
        Self::dispatch_payload(&mut guard, &pkt, is_utf16, None)?;
        guard.state.last_message = format!("Sent chat message ({} chars)", text.len());
        Self::sync_state_fields(&mut guard);
        Ok(pkt)
    }

    pub fn update_saved_credentials(
        &self,
        partner_id: &str,
        password: &str,
        remember: bool,
    ) -> Result<(), String> {
        let cleaned_id: String = partner_id.chars().filter(|c| !c.is_whitespace()).collect();
        let cleaned_pass = password.trim().to_string();
        let mut existing = load_saved_credentials();
        if remember && !cleaned_id.is_empty() {
            existing
                .saved_remotes
                .retain(|r| r.partner_id != cleaned_id);
            existing.saved_remotes.insert(
                0,
                SavedRemoteEntry {
                    partner_id: cleaned_id.clone(),
                    password: cleaned_pass.clone(),
                    last_used: now_epoch_secs() as u64,
                },
            );
            existing.saved_remotes.truncate(20);
        }
        let creds = if remember {
            SavedCredentials {
                partner_id: cleaned_id.clone(),
                password: cleaned_pass.clone(),
                remember: true,
                saved_remotes: existing.saved_remotes,
                theme: existing.theme,
                language: existing.language,
                toolbar_collapsed: existing.toolbar_collapsed,
            }
        } else {
            SavedCredentials {
                partner_id: String::new(),
                password: String::new(),
                remember: false,
                saved_remotes: existing.saved_remotes,
                theme: existing.theme,
                language: existing.language,
                toolbar_collapsed: existing.toolbar_collapsed,
            }
        };
        let _ = save_credentials(&creds);
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.state.saved_partner_id = creds.partner_id;
        guard.state.saved_password = creds.password;
        guard.state.remember_credentials = creds.remember;
        guard.state.saved_remotes = creds.saved_remotes;
        Self::sync_state_fields(&mut guard);
        Ok(())
    }

    pub fn update_ui_preferences(
        &self,
        theme: Option<&str>,
        language: Option<&str>,
        toolbar_collapsed: Option<bool>,
    ) -> Result<(), String> {
        const VALID_THEMES: &[&str] = &[
            "auto-time",
            "auto-os",
            "onedark",
            "onelight",
            "vscode-dark",
            "vscode-light",
            "gruvbox-dark",
            "gruvbox-light",
        ];
        const VALID_LANGUAGES: &[&str] = &["en", "vi"];

        if let Some(t) = theme {
            if !VALID_THEMES.contains(&t) {
                return Err(format!("Invalid theme mode: {}", t));
            }
        }
        if let Some(l) = language {
            if !VALID_LANGUAGES.contains(&l) {
                return Err(format!("Invalid language: {}", l));
            }
        }

        let mut creds = load_saved_credentials();
        if let Some(t) = theme {
            creds.theme = t.to_string();
        }
        if let Some(l) = language {
            creds.language = l.to_string();
        }
        if let Some(c) = toolbar_collapsed {
            creds.toolbar_collapsed = c;
        }
        save_credentials(&creds)?;

        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.state.saved_theme = creds.theme;
        guard.state.saved_language = creds.language;
        guard.state.saved_toolbar_collapsed = creds.toolbar_collapsed;
        Ok(())
    }

    pub fn start_connect(&self, partner_id: &str, password: &str) -> Result<SessionState, String> {
        let remember = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.state.remember_credentials
        };
        self.start_connect_with_remember(partner_id, password, remember)
    }

    pub fn start_connect_with_remember(
        &self,
        partner_id: &str,
        password: &str,
        remember: bool,
    ) -> Result<SessionState, String> {
        self.stop_flag.store(true, Ordering::SeqCst);
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.stop_flag.store(false, Ordering::SeqCst);

        let _ = self.update_saved_credentials(partner_id, password, remember);

        let (reg_profile, partner_profile) = {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
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
            guard.login_acknowledged = false;
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
            session_clone.run_worker(gen, reg_profile, partner_profile);
        });

        Ok(self.snapshot())
    }

    pub fn disconnect(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.login_acknowledged = false;
        guard.outbound_queue.clear();
        guard.state.state = "disconnected".to_string();
        guard.state.last_message = "Disconnected".to_string();
        Self::sync_state_fields(&mut guard);
    }

    fn is_active(&self, gen: u64) -> bool {
        !self.stop_flag.load(Ordering::SeqCst) && self.generation.load(Ordering::SeqCst) == gen
    }

    fn drain_and_send(
        &self,
        relay: &mut SessionTransport,
        route: &TargetRoute,
        held_keys: &mut BTreeSet<i32>,
        held_buttons: &mut BTreeSet<String>,
    ) {
        let queued: Vec<(Vec<u8>, bool, Option<InputEvent>)> = {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.outbound_queue.drain(..).collect()
        };
        for (payload, unicode_bytes, evt_opt) in queued {
            let _ = relay.send_relay(&payload, &route.public_ip, route.tcp_port, unicode_bytes);
            if let Some(evt) = evt_opt {
                match evt.event_type.as_str() {
                    "key" => {
                        let norm_vk = normalize_vk(evt.vk);
                        if evt.down {
                            held_keys.insert(norm_vk);
                        } else {
                            held_keys.remove(&norm_vk);
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

    fn run_worker(&self, gen: u64, reg_profile: RegistrationProfile, partner: PartnerProfile) {
        let mut held_keys = BTreeSet::<i32>::new();
        let mut held_buttons = BTreeSet::<String>::new();

        let (mut reg, mut relay, route) = match self.establish_relay_session(&reg_profile, &partner)
        {
            Ok(tuple) => tuple,
            Err(err) => {
                if self.generation.load(Ordering::SeqCst) == gen {
                    let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                    guard.state.state = "disconnected".to_string();
                    guard.state.error = Some(err.clone());
                    guard.state.last_message = err;
                    Self::sync_state_fields(&mut guard);
                }
                return;
            }
        };

        let mut last_clip_poll = Instant::now();
        while self.is_active(gen) {
            if last_clip_poll.elapsed() >= Duration::from_millis(800) {
                last_clip_poll = Instant::now();
                let should_poll = {
                    let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                    guard.login_acknowledged && guard.clipboard.auto_sync_system
                };
                if should_poll {
                    if let Some(sys_clip) = get_linux_system_clipboard() {
                        if !sys_clip.is_empty() {
                            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                            if !clipboard_text_eq(&sys_clip, &guard.clipboard.local_text)
                                && !clipboard_text_eq(&sys_clip, &guard.clipboard.remote_text)
                            {
                                guard.clipboard.local_text = sys_clip.clone();
                                guard.clipboard.updates_sent += 1;
                                if let Ok((payload, unicode_bytes)) =
                                    encode_clipboard_for_remote(&sys_clip)
                                {
                                    let _ = Self::dispatch_payload(
                                        &mut guard,
                                        &payload,
                                        unicode_bytes,
                                        None,
                                    );
                                }
                                Self::sync_state_fields(&mut guard);
                            }
                        }
                    }
                }
            }
            self.drain_and_send(&mut relay, &route, &mut held_keys, &mut held_buttons);

            match relay.receive_body() {
                Ok(body) => match decode_transport_body(&body) {
                    Ok(Some(pkt)) if pkt.kind == "relay" => {
                        self.handle_session_payload(&pkt.data);
                        self.drain_and_send(&mut relay, &route, &mut held_keys, &mut held_buttons);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        if self.generation.load(Ordering::SeqCst) == gen {
                            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                            guard.state.error = Some(e);
                            Self::sync_state_fields(&mut guard);
                        }
                        break;
                    }
                },
                Err(err) => {
                    if err.starts_with("TimeoutError") {
                        continue;
                    }
                    if self.is_active(gen) {
                        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                        guard.state.error = Some(err.clone());
                        guard.state.last_message = err;
                        Self::sync_state_fields(&mut guard);
                    }
                    break;
                }
            }
        }

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
        relay.close();
        reg.close();

        if self.generation.load(Ordering::SeqCst) == gen {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.state.state = "disconnected".to_string();
            Self::sync_state_fields(&mut guard);
        }
    }

    fn establish_relay_session(
        &self,
        reg_profile: &RegistrationProfile,
        partner: &PartnerProfile,
    ) -> Result<(RegistrationTransport, SessionTransport, TargetRoute), String> {
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
        relay.set_timeout(0.015);

        {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.files.modern_protocol = false;
            let decoder = ZstdDictionaryDecoder::new(guard.dictionary.clone(), 64 * 1024 * 1024);
            guard.screen = RGB555Screen::new((1024, 768), decoder, 1000, 32);
            guard.state.local_id = identity.client_id.to_string();
            guard.state.route_mode = "relay".to_string();
            guard.state.remote_endpoint = format!("{}:{}", route.public_ip, route.tcp_port);
            Self::sync_state_fields(&mut guard);
        }
        Ok((reg, relay, route))
    }

    pub fn move_pointer(&self, x: i32, y: i32) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let bounds = guard
            .screen
            .layout
            .active_bounds(&guard.screen.monitor_selection);
        let rx = x.saturating_add(bounds.left);
        let ry = y.saturating_add(bounds.top);
        let _ = guard
            .cursor
            .handle_command(crate::cursor::CursorCommand::Position {
                x: rx.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                y: ry.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            });
        Self::sync_state_fields(&mut guard);
        let payload = mouse_move(rx, ry)?;
        Self::dispatch_payload(&mut guard, &payload, false, None)?;
        Ok(payload)
    }
}

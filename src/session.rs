use crate::audio::{encode_sound_toggle_command, AudioBuffer};
use crate::clipboard::{
    clipboard_text_eq, decode_clipboard_packet, encode_clipboard_for_remote,
    get_linux_system_clipboard, set_linux_system_clipboard, ClipboardState,
};
use crate::cursor::{decode_cursor_parts, CursorManager};
use crate::file_transfer::{
    encode_chat_message, encode_file_request, is_utf16_string_packet, FileTransferEntry,
    FileTransferManager,
};
use crate::host::{DesktopBackend, HostSession, HostStatus, DEFAULT_LAN_PORT};
use crate::input::{
    encode_unicode_text_packets, keyboard, mouse_button, mouse_move, normalize_vk, InputEvent,
};
use crate::profile::{ClientProfile, HostProfile, ProfileManager};
use crate::protocol::{
    build_connection_request_command, build_login_with_password, build_registration_command,
    connection_request_password_value, decode_byte_array_fields, decode_target_route,
    decode_transport_body, encode_byte_array_fields, encode_relay_frame, encode_transport_frame,
    encrypt_registration_bytes, extract_command3_identity, format_command8_status_error,
    http_service_preface, login_password_fields, md5_hex_lower, Command3Identity, TargetRoute,
    REGISTRATION_PASSWORD,
};
use crate::screen::{
    desktop_size_from_login, DisplayLayout, MonitorSelection, RGB555Screen, ZstdDictionaryDecoder,
};
use crate::server::encode_base64;
use image::ImageEncoder;
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

pub fn effective_registration_terms(
    reg_profile: &RegistrationProfile,
    detected_ip: &str,
) -> Vec<String> {
    let mut terms = reg_profile.terms.clone();
    if terms.len() > 4
        && !detected_ip.is_empty()
        && (terms[4].is_empty() || terms[4] == "127.0.0.1" || terms[4] == "0.0.0.0")
    {
        terms[4] = detected_ip.to_string();
    }
    if terms.len() > 21 && (terms[19].is_empty() || terms[19] == "0") {
        terms[21] = "0".to_string();
    }
    terms
}

pub fn effective_lookup_terms(
    partner: &PartnerProfile,
    client_id: i32,
    reg_temp_password: &str,
) -> Result<Vec<String>, String> {
    let mut terms = partner.request_terms.clone();
    if terms.len() != 17 {
        return Err("partner request needs seventeen terms".to_string());
    }
    terms[2] = connection_request_password_value(&partner.password, partner.password_type);
    terms[6] = client_id.to_string();
    let hash_input = format!("{}{}", terms[6], reg_temp_password);
    terms[7] = md5_hex_lower(hash_input.as_bytes());
    if terms[12].is_empty() {
        terms[12] = "0".to_string();
    }
    if terms[14].is_empty() || terms[14] == "0" {
        terms[14] = "1".to_string();
    }
    Ok(terms)
}

pub fn parse_direct_lan_target(partner_id: &str) -> Option<(String, u16)> {
    let mut trimmed = partner_id.trim();
    if let Some(rest) = trimmed.strip_prefix("lan://") {
        trimmed = rest.trim();
    } else if let Some(rest) = trimmed.strip_prefix("tcp://") {
        trimmed = rest.trim();
    }
    if trimmed.is_empty() {
        return None;
    }
    if let Some((host_part, port_part)) = trimmed.split_once(':') {
        let port = port_part.trim().parse::<u16>().ok().filter(|&p| p > 0)?;
        let host = host_part.trim();
        if host.eq_ignore_ascii_case("localhost") {
            return Some(("127.0.0.1".to_string(), port));
        }
        if host.parse::<std::net::Ipv4Addr>().is_ok() {
            return Some((host.to_string(), port));
        }
        return None;
    }
    if trimmed.eq_ignore_ascii_case("localhost") {
        return Some(("127.0.0.1".to_string(), DEFAULT_LAN_PORT));
    }
    if trimmed.parse::<std::net::Ipv4Addr>().is_ok() {
        return Some((trimmed.to_string(), DEFAULT_LAN_PORT));
    }
    None
}

pub fn discover_lan_peer_by_id(partner_id: &str, timeout: Duration) -> Option<(String, u16)> {
    let pid = partner_id.trim();
    if pid.is_empty() {
        return None;
    }
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(timeout));
    let probe = format!("ULTRADESK_DISCOVER:{}", pid);
    let _ = sock.send_to(probe.as_bytes(), ("127.0.0.1", DEFAULT_LAN_PORT));
    let _ = sock.send_to(probe.as_bytes(), ("255.255.255.255", DEFAULT_LAN_PORT));
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 256];
    while Instant::now() < deadline {
        match sock.recv_from(&mut buf) {
            Ok((n, src)) => {
                if let Ok(txt) = std::str::from_utf8(&buf[..n]) {
                    if let Some(rest) = txt.trim().strip_prefix("ULTRADESK_HERE:") {
                        if let Some((id_part, port_part)) = rest.split_once(':') {
                            if id_part.trim() == pid {
                                if let Ok(port) = port_part.trim().parse::<u16>() {
                                    if port > 0 {
                                        let ip = match src.ip() {
                                            std::net::IpAddr::V4(v4) => v4.to_string(),
                                            std::net::IpAddr::V6(_) => "127.0.0.1".to_string(),
                                        };
                                        return Some((ip, port));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }
    None
}

pub fn looks_like_tls_record(prefix: &[u8]) -> bool {
    if prefix.len() < 3 {
        return false;
    }
    matches!(prefix[0], 20..=23) && prefix[1] == 3 && prefix[2] <= 4
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
    if let Ok(out) = std::process::Command::new("hostname").arg("-s").output() {
        if out.status.success() {
            let trimmed = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
    }
    if cfg!(target_os = "macos") {
        "mac-host".to_string()
    } else {
        "linux-host".to_string()
    }
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
        let _ = raw.set_nodelay(true);
        let _ = raw.set_read_timeout(Some(self.timeout));
        let _ = raw.set_write_timeout(Some(self.timeout));

        self.channel = if use_tls {
            let insecure = allow_insecure_broker_tls();
            let connector = build_tls_connector(insecure)?;
            let tls_domain = if insecure { TLS_TARGET_HOST } else { host };
            let tls = connector
                .connect(tls_domain, raw)
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
                std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::Interrupted
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
        let body_deadline = Instant::now() + self.timeout.max(Duration::from_secs(5));
        while self.buffer.len() < 4 + size {
            self.receive_chunk(body_deadline)?;
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
        let mut last_payload = Vec::new();
        for _ in 0..8 {
            let payload = self.receive_mode1_payload()?;
            if payload.starts_with(b"3:") {
                return extract_command3_identity(&payload);
            }
            if payload.starts_with(b"60:") {
                return Err(
                    "registration server rejected unregistered MAC with command 60".to_string(),
                );
            }
            last_payload = payload;
        }
        extract_command3_identity(&last_payload)
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
        let _ = raw.set_nodelay(true);
        let _ = raw.set_read_timeout(Some(self.inner.timeout));
        let _ = raw.set_write_timeout(Some(self.inner.timeout));

        self.inner.channel = if use_ssl {
            let insecure = allow_insecure_relay_tls(self.inner.original_certificate_policy);
            let connector = build_tls_connector(insecure)?;
            let tls_domain = if insecure {
                TLS_TARGET_HOST
            } else if !http_hostname.is_empty() {
                http_hostname
            } else {
                host
            };
            let tls = connector
                .connect(tls_domain, raw)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsVerifyPolicy {
    BrokerStrict,
    RelayCompatible,
}

pub fn allow_insecure_broker_tls() -> bool {
    std::env::var("ULTRADESK_INSECURE_BROKER_TLS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn allow_insecure_relay_tls(original_certificate_policy: bool) -> bool {
    if std::env::var("ULTRADESK_STRICT_TLS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return false;
    }
    original_certificate_policy
}

fn build_tls_connector(insecure: bool) -> Result<TlsConnector, String> {
    let mut builder = TlsConnector::builder();
    if insecure {
        builder.danger_accept_invalid_certs(true);
        builder.danger_accept_invalid_hostnames(true);
    }
    builder
        .build()
        .map_err(|e| format!("TLS connector build failed: {}", e))
}

fn truncate_chat_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        s
    } else {
        let mut idx = max_bytes;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
        &s[..idx]
    }
}

fn find_crlf_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

pub fn e2e_channel_policy_for_target(partner_id: &str) -> crate::e2e::E2eChannelPolicy {
    if partner_id.trim().starts_with("lan://") {
        crate::e2e::E2eChannelPolicy::MandatoryLan
    } else {
        crate::e2e::E2eChannelPolicy::OpportunisticInternet
    }
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
    #[serde(default)]
    pub dismissed_update_version: Option<String>,
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
            dismissed_update_version: None,
        }
    }
}

fn valid_absolute_env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).and_then(|v| {
        let trimmed = v.to_string_lossy().trim().to_string();
        if trimmed.is_empty() {
            return None;
        }
        let p = PathBuf::from(trimmed);
        p.is_absolute().then_some(p)
    })
}

pub fn credentials_file_path() -> PathBuf {
    let base = match valid_absolute_env_path("XDG_CONFIG_HOME") {
        Some(x) => x,
        None => match valid_absolute_env_path("HOME") {
            Some(h) => h.join(".config"),
            None => return crate::profile::fallback_private_config_dir().join("credentials.json"),
        },
    };
    let new_path = base.join("ultradesk-rust").join("credentials.json");
    let legacy_path = base.join("ultraviewer-linux").join("credentials.json");
    if !new_path.exists() && legacy_path.exists() {
        if let Ok(bytes) = fs::read(&legacy_path) {
            let _ = crate::profile::write_private_bytes(&new_path, &bytes);
        }
    }
    new_path
}

pub fn load_saved_credentials_from(path: &std::path::Path) -> SavedCredentials {
    if let Ok(bytes) = fs::read(path) {
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

pub fn load_saved_credentials() -> SavedCredentials {
    load_saved_credentials_from(&credentials_file_path())
}

pub fn save_credentials_to(path: &std::path::Path, creds: &SavedCredentials) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(creds)
        .map_err(|e| format!("Failed to serialize credentials: {}", e))?;
    crate::profile::write_private_bytes(path, &json)
        .map_err(|e| format!("Failed to write credentials: {}", e))
}

pub fn save_credentials(creds: &SavedCredentials) -> Result<(), String> {
    save_credentials_to(&credentials_file_path(), creds)
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
    #[serde(default)]
    pub e2e_enabled: bool,
    #[serde(default)]
    pub e2e_cipher: String,
    #[serde(default)]
    pub e2e_fingerprint: String,
    #[serde(default)]
    pub update_status: crate::updater::UpdateStatusSnapshot,
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
            e2e_enabled: false,
            e2e_cipher: String::new(),
            e2e_fingerprint: String::new(),
            update_status: crate::updater::UpdateStatusSnapshot::default(),
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
    pub last_local_move_at: Option<Instant>,
    pub e2e: crate::e2e::E2eNegotiationState,
    pub e2e_policy: crate::e2e::E2eChannelPolicy,
    pub update_engine: Arc<crate::updater::UpdateEngine>,
    pub credentials_path: PathBuf,
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
        mut registration_profile: RegistrationProfile,
        default_partner: PartnerProfile,
        dictionary: Vec<u8>,
    ) -> Self {
        let decoder = ZstdDictionaryDecoder::new(dictionary.clone(), 64 * 1024 * 1024);
        let screen = RGB555Screen::new((1024, 768), decoder, 1000, 32);
        let profile_manager = ProfileManager::from_xdg().unwrap_or_else(|_| {
            ProfileManager::from_custom_dir(crate::profile::fallback_private_config_dir())
        });
        let client_profile = profile_manager.load_or_create_client().unwrap_or_else(|_| {
            let dev_id = crate::profile::generate_ephemeral_device_id();
            let syn_mac = crate::profile::synthesize_client_mac(&dev_id);
            ClientProfile {
                device_id: dev_id,
                temp_password: crate::profile::generate_ephemeral_temp_password(),
                display_name: "UltraDesk".to_string(),
                synthetic_mac: syn_mac,
                local_ip: std::net::Ipv4Addr::new(127, 0, 0, 1),
                path: crate::profile::fallback_private_config_dir().join("profile"),
            }
        });
        let host_profile = profile_manager.load_or_create_host().unwrap_or_else(|_| {
            let dev_id = crate::profile::generate_ephemeral_device_id();
            let hw_mac = crate::profile::synthesize_client_mac(&dev_id);
            let mp_file = profile_manager
                .config_dir()
                .join(crate::profile::MASTER_PASSWORD_FILE);
            let fallback_mp = match profile_manager.master_password_proof() {
                Ok(proof) => proof,
                Err(_) if mp_file.exists() => Some("!locked-invalid-master-password!".to_string()),
                Err(_) => None,
            };
            HostProfile {
                device_id: dev_id,
                temp_password: crate::profile::generate_ephemeral_temp_password(),
                display_name: "UltraDesk".to_string(),
                hardware_mac: hw_mac,
                local_ip: std::net::Ipv4Addr::new(127, 0, 0, 1),
                master_password_proof: fallback_mp,
                path: crate::profile::fallback_private_config_dir().join("host-profile"),
            }
        });
        if registration_profile.terms.len() > 8 {
            if registration_profile.terms[1].is_empty()
                || registration_profile.terms[1] == "00000000000000000000000000000000"
            {
                registration_profile.terms[1] = client_profile.device_id.clone();
            }
            if registration_profile.terms[2].is_empty()
                || registration_profile.terms[2] == "00000000"
            {
                registration_profile.terms[2] = client_profile.temp_password.clone();
            }
            if registration_profile.terms[8].is_empty()
                || registration_profile.terms[8] == "020000000000"
            {
                registration_profile.terms[8] = client_profile.synthetic_mac.clone();
            }
        }
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
        let update_engine = Arc::new(crate::updater::UpdateEngine::new());
        let credentials_path = credentials_file_path();
        let saved_creds = load_saved_credentials_from(&credentials_path);
        if let Some(ref dismissed) = saved_creds.dismissed_update_version {
            update_engine.set_dismissed_version(Some(dismissed.clone()));
        }
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
            last_local_move_at: None,
            e2e: crate::e2e::E2eNegotiationState::Disabled,
            e2e_policy: crate::e2e::E2eChannelPolicy::OpportunisticInternet,
            update_engine,
            credentials_path,
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
        self.png_frame_versioned().map(|(_, bytes)| bytes)
    }

    pub fn png_frame_versioned(&self) -> Result<(u64, Vec<u8>), String> {
        let (ver, view) = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((v, ref bytes)) = guard.screen.cached_png {
                if v == guard.screen.version {
                    return Ok((v, bytes.clone()));
                }
            }
            (
                guard.screen.version,
                guard.screen.active_view_buffer().into_owned(),
            )
        };
        let mut out = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut out);
        encoder
            .write_image(
                &view.pixels,
                view.width,
                view.height,
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| format!("PNG encoding failed: {}", e))?;
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.screen.version == ver {
            guard.screen.cached_png = Some((ver, out.clone()));
        }
        Ok((ver, out))
    }

    pub fn jpeg_frame_versioned(&self, quality: u8) -> Result<(u64, Vec<u8>), String> {
        let q = quality.clamp(1, 100);
        let (ver, view) = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((v, cached_q, ref bytes)) = guard.screen.cached_jpeg {
                if v == guard.screen.version && cached_q == q {
                    return Ok((v, bytes.clone()));
                }
            }
            (
                guard.screen.version,
                guard.screen.active_view_buffer().into_owned(),
            )
        };
        let bytes = view.to_jpeg(q)?;
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.screen.version == ver {
            guard.screen.cached_jpeg = Some((ver, q, bytes.clone()));
        }
        Ok((ver, bytes))
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

    pub fn host_request_macos_permission(
        &self,
        kind: crate::host::MacOsPermissionKind,
    ) -> HostStatus {
        let _ = crate::host::MacOsPermissionState::request(kind);
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::sync_state_fields(&mut guard);
        guard.host_session.status()
    }

    pub fn host_open_macos_permission_settings(
        &self,
        kind: crate::host::MacOsPermissionKind,
    ) -> Result<(&'static str, HostStatus), String> {
        let pane = crate::host::MacOsPermissionState::open_settings(kind)?;
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::sync_state_fields(&mut guard);
        Ok((pane, guard.host_session.status()))
    }

    pub fn host_capture_image(
        &self,
        format: &str,
        quality: u8,
    ) -> Result<(u32, u32, Vec<u8>), String> {
        let hs = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.host_session.clone()
        };
        let (w, h, rgb) = hs.capture_rgb888_snapshot()?;
        let mut out = Vec::new();
        if format.eq_ignore_ascii_case("jpeg") || format.eq_ignore_ascii_case("jpg") {
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100))
                .write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)
                .map_err(|e| format!("failed to encode host JPEG: {}", e))?;
        } else {
            image::codecs::png::PngEncoder::new(&mut out)
                .write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)
                .map_err(|e| format!("failed to encode host PNG: {}", e))?;
        }
        Ok((w, h, out))
    }

    pub fn host_inject_input(&self, event: InputEvent) -> Result<Vec<u8>, String> {
        let wire = event.to_wire()?;
        let hs = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.host_session.clone()
        };
        hs.inject_local_wire_input(&wire)?;
        Ok(wire)
    }

    pub fn host_send_text(&self, text: &str, caps_lock: bool) -> Result<usize, String> {
        let packets = encode_unicode_text_packets(text, caps_lock);
        let hs = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.host_session.clone()
        };
        let count = packets.len();
        for pkt in packets {
            hs.inject_local_wire_input(&pkt)?;
        }
        Ok(count)
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
        if let Some(res) = guard.files.read_completed_download(file_id) {
            return Some(res);
        }
        if let Some(entry) = guard.files.transfers.get(file_id) {
            if entry.direction == crate::file_transfer::TransferDirection::Send
                && entry.saved_path.is_none()
                && !entry.buffer.is_empty()
            {
                return Some((entry.file_name.clone(), entry.buffer.clone()));
            }
        }
        guard.host_session.downloaded_file_bytes(file_id)
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
        for msg in host_chats {
            let bounded = truncate_chat_utf8(&msg, crate::file_transfer::MAX_CHAT_MESSAGE_BYTES);
            inner.files.chat_messages.push(bounded.to_string());
        }
        if inner.files.chat_messages.len() > crate::file_transfer::MAX_CHAT_HISTORY_MESSAGES {
            let overflow =
                inner.files.chat_messages.len() - crate::file_transfer::MAX_CHAT_HISTORY_MESSAGES;
            inner.files.chat_messages.drain(0..overflow);
        }
        if let Some(clip_text) = host_clip {
            inner.clipboard.remote_text = clip_text.clone();
            if !clip_text.is_empty() {
                inner.clipboard.local_text = clip_text;
            }
            inner.clipboard.updates_received += 1;
        }
        let hs = inner.host_session.status();
        if inner.state.local_id.is_empty() && !hs.host_id.is_empty() {
            inner.state.local_id = hs.host_id.clone();
        }
        inner.state.host_hardware_mac = hs.hardware_mac.clone();
        inner.state.host_status = Some(hs);
        inner.state.client_synthetic_mac = inner.client_profile.synthetic_mac.clone();
        inner.state.audio_enabled = inner.audio.enabled;
        inner.state.audio_chunks_received = inner.audio.chunks_received;
        inner.state.audio_bytes_decoded = inner.audio.bytes_decoded;
        inner.state.clipboard_local = inner.clipboard.local_text.clone();
        inner.state.clipboard_remote = inner.clipboard.remote_text.clone();
        inner.state.clipboard_auto_sync = inner.clipboard.auto_sync_system;
        inner.state.clipboard_updates_sent = inner.clipboard.updates_sent;
        inner.state.clipboard_updates_received = inner.clipboard.updates_received;
        let mut combined_transfers: Vec<FileTransferEntry> =
            inner.files.transfers.values().cloned().collect();
        for host_tr in inner.host_session.file_transfers() {
            if !combined_transfers
                .iter()
                .any(|t| t.file_id == host_tr.file_id)
            {
                combined_transfers.push(host_tr);
            }
        }
        inner.state.file_transfers = combined_transfers;
        inner.state.chat_messages = inner.files.chat_messages.clone();
        inner.state.e2e_enabled = inner.e2e.is_enabled();
        inner.state.e2e_cipher = inner.e2e.cipher_name().to_string();
        inner.state.e2e_fingerprint = inner.e2e.fingerprint().to_string();
        inner.state.update_status = inner.update_engine.snapshot();
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

        let decrypted_buf: Vec<u8>;
        let payload: &[u8] = if payload == [1, 0, 0, 0, 22] && !guard.e2e.is_enabled() {
            guard.e2e = crate::e2e::E2eNegotiationState::Disabled;
            payload
        } else if let Some(cipher) = guard.e2e.cipher_mut() {
            match cipher.open(payload) {
                Ok((plain, _unicode_flag)) => {
                    decrypted_buf = plain;
                    &decrypted_buf
                }
                Err(err) => {
                    guard.login_acknowledged = false;
                    guard.state.state = "disconnected".to_string();
                    guard.state.error = Some(err.clone());
                    guard.state.last_message = err;
                    self.stop_flag.store(true, Ordering::SeqCst);
                    Self::sync_state_fields(&mut guard);
                    return;
                }
            }
        } else if crate::e2e::is_e2e_frame(payload) {
            let msg = "Unexpected E2E encrypted packet before handshake".to_string();
            guard.login_acknowledged = false;
            guard.state.state = "disconnected".to_string();
            guard.state.error = Some(msg.clone());
            guard.state.last_message = msg;
            self.stop_flag.store(true, Ordering::SeqCst);
            Self::sync_state_fields(&mut guard);
            return;
        } else {
            payload
        };

        if let Ok(fields) = decode_byte_array_fields(payload) {
            if !fields.is_empty() && !fields[0].is_empty() {
                let kind = fields[0][0];

                if kind == 22 {
                    let msg = "Sai mật khẩu (Wrong Password)".to_string();
                    guard.login_acknowledged = false;
                    guard.state.state = "disconnected".to_string();
                    guard.state.error = Some(msg.clone());
                    guard.state.last_message = msg;
                    self.stop_flag.store(true, Ordering::SeqCst);
                    Self::sync_state_fields(&mut guard);
                    return;
                }
                if kind == 27 {
                    let msg = "Đối tác đã từ chối hoặc ngắt kết nối".to_string();
                    guard.login_acknowledged = false;
                    guard.state.state = "disconnected".to_string();
                    guard.state.error = Some(msg.clone());
                    guard.state.last_message = msg;
                    self.stop_flag.store(true, Ordering::SeqCst);
                    Self::sync_state_fields(&mut guard);
                    return;
                }

                if kind == 0 {
                    if let Ok((w, h)) = desktop_size_from_login(&fields) {
                        if matches!(guard.e2e, crate::e2e::E2eNegotiationState::Offered { .. }) {
                            let field3 = fields.get(3).map(Vec::as_slice).unwrap_or(&[]);
                            match crate::e2e::finalize_viewer_handshake_with_policy(
                                &guard.e2e,
                                field3,
                                guard.e2e_policy.requires_e2e(),
                            ) {
                                Ok(next_state) => {
                                    guard.e2e = next_state;
                                }
                                Err(err) => {
                                    guard.e2e = crate::e2e::E2eNegotiationState::Disabled;
                                    guard.login_acknowledged = false;
                                    guard.state.state = "disconnected".to_string();
                                    guard.state.error = Some(err.clone());
                                    guard.state.last_message = err;
                                    self.stop_flag.store(true, Ordering::SeqCst);
                                    Self::sync_state_fields(&mut guard);
                                    return;
                                }
                            }
                        }
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
                        guard.outbound_queue.clear();
                        let should_seed_clip = guard.clipboard.auto_sync_system;
                        if let Ok(pkt48) =
                            encode_byte_array_fields(&[&[48u8][..], &[effective_bs as u8][..]])
                        {
                            let _ = Self::dispatch_payload(&mut guard, &pkt48, false, None);
                        }
                        let _ = Self::dispatch_payload(&mut guard, b"17", false, None);
                        guard.state.state = "connected".to_string();
                        guard.state.last_message = format!("Remote desktop size {}x{}", w, h);
                        Self::sync_state_fields(&mut guard);
                        drop(guard);
                        if should_seed_clip {
                            if let Some(sys_clip) = get_linux_system_clipboard() {
                                let mut guard =
                                    self.inner.lock().unwrap_or_else(|e| e.into_inner());
                                guard.clipboard.local_text = sys_clip;
                                Self::sync_state_fields(&mut guard);
                            }
                        }
                        return;
                    }
                }

                if kind == 48 && fields.len() > 1 {
                    if !guard.login_acknowledged {
                        return;
                    }
                    let block_size = if fields[1].len() == 1 {
                        fields[1][0] as i32
                    } else if fields[1].len() == 2 {
                        i16::from_le_bytes([fields[1][0], fields[1][1]]) as i32
                    } else if fields[1].len() == 4 {
                        i32::from_le_bytes([fields[1][0], fields[1][1], fields[1][2], fields[1][3]])
                    } else {
                        0
                    };
                    if (16..=256).contains(&block_size) {
                        guard.screen.block_size = block_size;
                    }
                    return;
                }

                if kind == 1 && fields.len() == 11 {
                    if !guard.login_acknowledged {
                        return;
                    }
                    match guard.screen.apply(&fields) {
                        Ok(applied) => {
                            guard.frames_applied += 1;
                            guard.rectangles_applied += applied;
                            let _ = Self::dispatch_payload(&mut guard, b"18", false, None);
                            guard.state.last_message =
                                format!("Frame #{} ({} rects)", guard.frames_applied, applied);
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
                    if !guard.login_acknowledged {
                        return;
                    }
                    if let Ok(cmd) = decode_cursor_parts(&fields) {
                        if matches!(cmd, crate::cursor::CursorCommand::Position { .. })
                            && guard
                                .last_local_move_at
                                .is_some_and(|t| t.elapsed() < Duration::from_millis(300))
                        {
                            return;
                        }
                        if let Some(reply) = guard.cursor.handle_command(cmd) {
                            let _ = Self::dispatch_payload(&mut guard, &reply, false, None);
                        }
                        Self::sync_state_fields(&mut guard);
                        return;
                    }
                }
            }
        }

        if let Some(clip_msg) = decode_clipboard_packet(payload) {
            let should_write_os = guard.login_acknowledged
                && guard.clipboard.auto_sync_system
                && !clip_msg.text.is_empty()
                && !clipboard_text_eq(&clip_msg.text, &guard.clipboard.remote_text);
            if should_write_os {
                let text = clip_msg.text.clone();
                drop(guard);
                let wrote_ok = set_linux_system_clipboard(&text);
                let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                if guard.login_acknowledged {
                    guard
                        .clipboard
                        .apply_remote_clipboard_update(&text, wrote_ok);
                    guard.state.last_message =
                        format!("Clipboard synced from remote ({} chars)", text.len());
                    Self::sync_state_fields(&mut guard);
                }
            } else {
                guard
                    .clipboard
                    .apply_remote_clipboard_update(&clip_msg.text, false);
                guard.state.last_message = format!(
                    "Clipboard synced from remote ({} chars)",
                    clip_msg.text.len()
                );
                Self::sync_state_fields(&mut guard);
            }
            return;
        }

        if !guard.login_acknowledged {
            return;
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

        if let Ok(Some((ft_pkt, replies))) = guard.files.handle_packet(payload) {
            if matches!(
                ft_pkt,
                crate::file_transfer::FileTransferPacket::ChatMessage { .. }
            ) {
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
            guard.last_local_move_at = Some(Instant::now());
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
        guard.audio.set_enabled(enable);
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

    pub fn set_clipboard_sync_mode(&self, mode: crate::clipboard::ClipboardSyncMode) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.clipboard.set_sync_mode(mode);
        Self::sync_state_fields(&mut guard);
    }

    pub fn send_clipboard(&self, text: &str, sync_system: bool) -> Result<Vec<u8>, String> {
        let payload = {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.clipboard.local_text = text.to_string();
            guard.clipboard.updates_sent += 1;
            let _ = guard.host_session.send_clipboard(text);
            let (payload, unicode_bytes) = encode_clipboard_for_remote(text)?;
            Self::dispatch_payload(&mut guard, &payload, unicode_bytes, None)?;
            guard.state.last_message = format!("Sent clipboard to remote ({} chars)", text.len());
            Self::sync_state_fields(&mut guard);
            payload
        };
        if sync_system {
            let _ = set_linux_system_clipboard(text);
        }
        Ok(payload)
    }

    pub fn sync_from_system_clipboard(&self) -> Result<String, String> {
        let text = get_linux_system_clipboard().unwrap_or_default();
        self.send_clipboard(&text, false)?;
        Ok(text)
    }

    pub fn offer_file_bytes(&self, file_name: &str, data: Vec<u8>) -> Result<String, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.state.state != "connected" {
            let hs = guard.host_session.status();
            if matches!(
                hs.phase,
                crate::host::HostPhase::Starting | crate::host::HostPhase::Streaming
            ) {
                let file_id = guard.host_session.offer_file_bytes(file_name, data)?;
                guard.state.last_message = format!("Host offered file {} ({})", file_name, file_id);
                Self::sync_state_fields(&mut guard);
                return Ok(file_id);
            }
        }
        let (file_id, pkt) = guard.files.offer_file_bytes(file_name, data)?;
        let is_utf16 = is_utf16_string_packet(&pkt);
        Self::dispatch_payload(&mut guard, &pkt, is_utf16, None)?;
        guard.state.last_message = format!("Offered file {} ({})", file_name, file_id);
        Self::sync_state_fields(&mut guard);
        Ok(file_id)
    }

    pub fn offer_file_from_path(&self, path: &Path) -> Result<String, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.state.state != "connected" {
            let hs = guard.host_session.status();
            if matches!(
                hs.phase,
                crate::host::HostPhase::Starting | crate::host::HostPhase::Streaming
            ) {
                let file_id = guard.host_session.offer_file_from_path(path)?;
                guard.state.last_message =
                    format!("Host offered file {} ({})", path.display(), file_id);
                Self::sync_state_fields(&mut guard);
                return Ok(file_id);
            }
        }
        let (file_id, pkt) = guard.files.offer_file_from_path(path)?;
        let is_utf16 = is_utf16_string_packet(&pkt);
        Self::dispatch_payload(&mut guard, &pkt, is_utf16, None)?;
        guard.state.last_message = format!("Offered file {} ({})", path.display(), file_id);
        Self::sync_state_fields(&mut guard);
        Ok(file_id)
    }

    pub fn request_remote_file(&self, remote_path: &str) -> Result<Vec<u8>, String> {
        let cleaned = remote_path.trim();
        if cleaned.is_empty() {
            return Err("Remote file path cannot be empty".to_string());
        }
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.files.record_requested_remote_path(cleaned);
        let modern = guard.files.modern_protocol;
        let pkt = encode_file_request(cleaned, modern)?;
        let is_utf16 = is_utf16_string_packet(&pkt);
        Self::dispatch_payload(&mut guard, &pkt, is_utf16, None)?;
        guard.state.last_message = format!("Requested remote file {}", cleaned);
        Self::sync_state_fields(&mut guard);
        Ok(pkt)
    }

    pub fn accept_file(&self, file_id: &str) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.files.transfers.contains_key(file_id) {
            let pkt = guard.files.accept_incoming_offer(file_id)?;
            let is_utf16 = is_utf16_string_packet(&pkt);
            Self::dispatch_payload(&mut guard, &pkt, is_utf16, None)?;
            guard.state.last_message = format!("Accepted incoming file {}", file_id);
            Self::sync_state_fields(&mut guard);
            return Ok(pkt);
        }
        let pkt = guard.host_session.accept_file(file_id)?;
        guard.state.last_message = format!("Host accepted incoming file {}", file_id);
        Self::sync_state_fields(&mut guard);
        Ok(pkt)
    }

    pub fn cancel_file(&self, file_id: &str) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.files.transfers.contains_key(file_id) {
            let pkt = guard.files.cancel_transfer(file_id)?;
            let is_utf16 = is_utf16_string_packet(&pkt);
            Self::dispatch_payload(&mut guard, &pkt, is_utf16, None)?;
            guard.state.last_message = format!("Cancelled file transfer {}", file_id);
            Self::sync_state_fields(&mut guard);
            return Ok(pkt);
        }
        let pkt = guard.host_session.cancel_file(file_id)?;
        guard.state.last_message = format!("Host cancelled file transfer {}", file_id);
        Self::sync_state_fields(&mut guard);
        Ok(pkt)
    }

    pub fn send_chat(&self, text: &str) -> Result<Vec<u8>, String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let bounded = truncate_chat_utf8(text, crate::file_transfer::MAX_CHAT_MESSAGE_BYTES);
        let modern = guard.files.modern_protocol;
        let pkt = encode_chat_message(bounded, modern)?;
        let is_utf16 = is_utf16_string_packet(&pkt);
        guard.files.chat_messages.push(format!("Me: {}", bounded));
        if guard.files.chat_messages.len() > crate::file_transfer::MAX_CHAT_HISTORY_MESSAGES {
            let overflow =
                guard.files.chat_messages.len() - crate::file_transfer::MAX_CHAT_HISTORY_MESSAGES;
            guard.files.chat_messages.drain(0..overflow);
        }
        let _ = guard.host_session.send_chat(bounded);
        Self::dispatch_payload(&mut guard, &pkt, is_utf16, None)?;
        guard.state.last_message = format!("Sent chat message ({} chars)", bounded.len());
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
        let creds_path = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.credentials_path.clone()
        };
        let mut existing = load_saved_credentials_from(&creds_path);
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
                dismissed_update_version: existing.dismissed_update_version,
            }
        } else {
            let mut remotes = existing.saved_remotes;
            for r in &mut remotes {
                r.password.clear();
            }
            SavedCredentials {
                partner_id: String::new(),
                password: String::new(),
                remember: false,
                saved_remotes: remotes,
                theme: existing.theme,
                language: existing.language,
                toolbar_collapsed: existing.toolbar_collapsed,
                dismissed_update_version: existing.dismissed_update_version,
            }
        };
        let _ = save_credentials_to(&creds_path, &creds);
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

        let creds_path = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.credentials_path.clone()
        };
        let mut creds = load_saved_credentials_from(&creds_path);
        if let Some(t) = theme {
            creds.theme = t.to_string();
        }
        if let Some(l) = language {
            creds.language = l.to_string();
        }
        if let Some(c) = toolbar_collapsed {
            creds.toolbar_collapsed = c;
        }
        save_credentials_to(&creds_path, &creds)?;

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
            if guard.state.partner_id.trim().is_empty() || p.password.trim().is_empty() {
                let err = "Partner ID and password are required".to_string();
                guard.state.state = "disconnected".to_string();
                guard.state.error = Some(err.clone());
                guard.state.last_message = err.clone();
                Self::sync_state_fields(&mut guard);
                return Err(err);
            }
            guard.latest_png.clear();
            guard.frames_applied = 0;
            guard.rectangles_applied = 0;
            guard.outbound_queue.clear();
            guard.login_acknowledged = false;
            guard.e2e = crate::e2e::E2eNegotiationState::Disabled;
            guard.e2e_policy = e2e_channel_policy_for_target(partner_id);
            guard.files.reset_for_new_session();
            guard.audio.clear();
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
        guard.e2e = crate::e2e::E2eNegotiationState::Disabled;
        guard.files.reset_for_new_session();
        guard.audio.clear();
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
        was_logged_in: &mut bool,
        last_cipher: &mut Option<crate::e2e::E2eSessionCipher>,
    ) {
        let queued: Vec<(Vec<u8>, bool, Option<InputEvent>)> = {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if !guard.login_acknowledged {
                return;
            }
            *was_logged_in = true;
            let raw_items: Vec<_> = std::mem::take(&mut guard.outbound_queue);
            let sealed: Vec<_> = raw_items
                .into_iter()
                .map(|(payload, unicode_bytes, evt_opt)| {
                    if let Some(cipher) = guard.e2e.cipher_mut() {
                        (cipher.seal(&payload, unicode_bytes), false, evt_opt)
                    } else {
                        (payload, unicode_bytes, evt_opt)
                    }
                })
                .collect();
            *last_cipher = guard.e2e.cipher_mut().cloned();
            sealed
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

    pub fn resolve_lan_endpoint(&self, partner_id: &str) -> Option<(String, u16)> {
        if let Some(target) = parse_direct_lan_target(partner_id) {
            return Some(target);
        }
        let pid = partner_id.trim();
        if pid.is_empty() {
            return None;
        }
        let hs = self.host_status();
        let dev_id = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.host_profile.device_id.clone()
        };
        if hs.running && hs.lan_port > 0 && (hs.host_id == pid || dev_id == pid) {
            return Some(("127.0.0.1".to_string(), hs.lan_port));
        }
        None
    }

    fn establish_lan_session(
        &self,
        lan_ip: &str,
        lan_port: u16,
        partner: &PartnerProfile,
    ) -> Result<(SessionTransport, TargetRoute), String> {
        let mut transport = SessionTransport::new(5.0, 16 * 1024 * 1024, true)?;
        transport.connect(lan_ip, lan_port, "", false)?;
        let local_id = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if !guard.state.local_id.is_empty() {
                guard.state.local_id.clone()
            } else {
                guard.client_profile.device_id.chars().take(9).collect()
            }
        };
        let salt = random_digits(8);
        let auth = login_password_fields(&partner.password, partner.password_type, Some(&salt))?;
        let is_temp_pw = !partner.password.is_empty()
            && partner.password.len() <= 8
            && partner.password.chars().all(|c| c.is_ascii_digit());
        let requires_e2e = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.e2e_policy.requires_e2e()
        };
        let (e2e_wire, e2e_state) = if !partner.password.is_empty() && (is_temp_pw || requires_e2e)
        {
            crate::e2e::create_viewer_offer_for_password(&auth.salt, &partner.password, "")
        } else {
            (String::new(), crate::e2e::E2eNegotiationState::Disabled)
        };
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
            local_id.clone(),
            local_hostname(),
            "0".to_string(),
            "".to_string(),
            "".to_string(),
            random_digits(16),
            "2".to_string(),
            "80".to_string(),
            "0".to_string(),
            "0".to_string(),
            e2e_wire,
            "6.6.133".to_string(),
            "0".to_string(),
        ];
        let login_payload = build_login_with_password(
            &fields,
            &partner.password,
            partner.password_type,
            Some(&salt),
        )?;
        transport.send_relay(&login_payload, lan_ip, lan_port, true)?;
        transport.set_timeout(0.005);

        let route = TargetRoute {
            lan_ip: lan_ip.to_string(),
            public_ip: lan_ip.to_string(),
            public_port: lan_port,
            lan_port,
            command_port: lan_port,
            computer_name_encoded: String::new(),
            tcp_port: lan_port,
            tcp_server_ip: lan_ip.to_string(),
            matched_custom_password: 0,
            target_session_id: 0,
            tcp_server_hostname: String::new(),
            partner_secret_token: None,
        };
        {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.e2e = e2e_state;
            guard.files.modern_protocol = false;
            let decoder = ZstdDictionaryDecoder::new(guard.dictionary.clone(), 64 * 1024 * 1024);
            guard.screen = RGB555Screen::new((1024, 768), decoder, 1000, 32);
            guard.state.local_id = local_id;
            guard.state.route_mode = "lan".to_string();
            guard.state.remote_endpoint = format!("{}:{}", lan_ip, lan_port);
            Self::sync_state_fields(&mut guard);
        }
        Ok((transport, route))
    }

    fn run_worker(&self, gen: u64, reg_profile: RegistrationProfile, partner: PartnerProfile) {
        let mut held_keys = BTreeSet::<i32>::new();
        let mut held_buttons = BTreeSet::<String>::new();
        let mut was_logged_in = false;
        let mut last_cipher: Option<crate::e2e::E2eSessionCipher> = None;
        let target_id = partner.request_terms.get(1).cloned().unwrap_or_default();

        let (mut reg_opt, mut relay, route) =
            if let Some((lan_ip, lan_port)) = self.resolve_lan_endpoint(&target_id) {
                match self.establish_lan_session(&lan_ip, lan_port, &partner) {
                    Ok((transport, route)) => (None, transport, route),
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
                }
            } else {
                match self.establish_relay_session(&reg_profile, &partner) {
                    Ok((reg, relay, route)) => (Some(reg), relay, route),
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
                }
            };

        let mut last_clip_poll = Instant::now();
        while self.is_active(gen) {
            if last_clip_poll.elapsed() >= Duration::from_millis(800) {
                last_clip_poll = Instant::now();
                let (should_poll, prev_recv, prev_sent) = {
                    let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        guard.login_acknowledged && guard.clipboard.auto_sync_system,
                        guard.clipboard.updates_received,
                        guard.clipboard.updates_sent,
                    )
                };
                if should_poll {
                    if let Some(sys_clip) = get_linux_system_clipboard() {
                        if !sys_clip.is_empty() {
                            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                            if guard.login_acknowledged
                                && guard.clipboard.auto_sync_system
                                && guard.clipboard.updates_received == prev_recv
                                && guard.clipboard.updates_sent == prev_sent
                                && !clipboard_text_eq(&sys_clip, &guard.clipboard.local_text)
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
            self.drain_and_send(
                &mut relay,
                &route,
                &mut held_keys,
                &mut held_buttons,
                &mut was_logged_in,
                &mut last_cipher,
            );

            match relay.receive_body() {
                Ok(body) => match decode_transport_body(&body) {
                    Ok(Some(pkt)) if pkt.kind == "relay" => {
                        self.handle_session_payload(&pkt.data);
                        self.drain_and_send(
                            &mut relay,
                            &route,
                            &mut held_keys,
                            &mut held_buttons,
                            &mut was_logged_in,
                            &mut last_cipher,
                        );
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
                let wire_pkt = if let Some(ref mut cipher) = last_cipher {
                    cipher.seal(&pkt, false)
                } else {
                    pkt
                };
                let _ = relay.send_relay(&wire_pkt, &route.public_ip, route.tcp_port, false);
            }
        }
        for button in held_buttons {
            if let Ok(pkt) = mouse_button(&button, false) {
                let wire_pkt = if let Some(ref mut cipher) = last_cipher {
                    cipher.seal(&pkt, false)
                } else {
                    pkt
                };
                let _ = relay.send_relay(&wire_pkt, &route.public_ip, route.tcp_port, false);
            }
        }
        if was_logged_in {
            let wire_19 = if let Some(ref mut cipher) = last_cipher {
                cipher.seal(b"19", false)
            } else {
                b"19".to_vec()
            };
            let _ = relay.send_relay(&wire_19, &route.public_ip, route.tcp_port, false);
        }
        relay.close();
        if let Some(ref mut reg) = reg_opt {
            reg.close();
        }

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
        if partner.request_terms.len() != 17 {
            return Err("partner request needs seventeen terms".to_string());
        }

        let local_ip = detect_local_ip();
        let mut reg_terms = effective_registration_terms(reg_profile, &local_ip);
        let target_partner_id = partner
            .request_terms
            .get(1)
            .map(String::as_str)
            .unwrap_or_default();
        if target_partner_id == "134462379"
            && reg_terms.len() > 8
            && reg_terms[8].eq_ignore_ascii_case(crate::host::DEFAULT_FALLBACK_HOST_MAC)
        {
            reg_terms[8] = crate::host::SECONDARY_FALLBACK_HOST_MAC.to_string();
        }

        let mut reg = RegistrationTransport::new(8.0, 1024 * 1024, true)?;
        reg.connect(DEFAULT_REGISTRATION_HOST, 443)?;
        let reg_cmd = build_registration_command(&reg_terms)?;
        reg.send_command(&reg_cmd)?;
        let identity = match reg.receive_identity() {
            Ok(id) => id,
            Err(_)
                if reg_terms.len() > 8
                    && !reg_terms[8]
                        .eq_ignore_ascii_case(crate::host::DEFAULT_FALLBACK_HOST_MAC) =>
            {
                reg.close();
                reg_terms[8] = if target_partner_id == "134462379" {
                    crate::host::SECONDARY_FALLBACK_HOST_MAC.to_string()
                } else {
                    crate::host::DEFAULT_FALLBACK_HOST_MAC.to_string()
                };
                reg = RegistrationTransport::new(8.0, 1024 * 1024, true)?;
                reg.connect(DEFAULT_REGISTRATION_HOST, 443)?;
                let fallback_cmd = build_registration_command(&reg_terms)?;
                reg.send_command(&fallback_cmd)?;
                reg.receive_identity()?
            }
            Err(e) => return Err(e),
        };

        let reg_temp_password = reg_terms.get(2).map(String::as_str).unwrap_or_default();
        let terms = effective_lookup_terms(partner, identity.client_id, reg_temp_password)?;

        let req_cmd = build_connection_request_command(&terms)?;
        reg.send_command(&req_cmd)?;

        let mut route_opt: Option<TargetRoute> = None;
        for _ in 0..4 {
            let response = reg.receive_mode1_payload()?;
            if response.starts_with(b"8:") {
                if let Some(msg) = format_command8_status_error(&response) {
                    return Err(msg);
                }
                route_opt = Some(decode_target_route(&response)?);
                break;
            }
        }

        let route = route_opt
            .ok_or_else(|| "Không nhận được thông tin định tuyến từ đối tác".to_string())?;

        let mut relay = SessionTransport::new(8.0, 16 * 1024 * 1024, true)?;
        relay.connect(&route.tcp_server_ip, 443, &route.tcp_server_hostname, true)?;

        let salt = random_digits(8);
        let auth = login_password_fields(&partner.password, partner.password_type, Some(&salt))?;
        let is_temp_pw = !partner.password.is_empty()
            && partner.password.len() <= 8
            && partner.password.chars().all(|c| c.is_ascii_digit());
        let requires_e2e = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.e2e_policy.requires_e2e()
        };
        let (e2e_wire, e2e_state) = if !partner.password.is_empty() && (is_temp_pw || requires_e2e)
        {
            crate::e2e::create_viewer_offer_for_password(&auth.salt, &partner.password, "")
        } else {
            (String::new(), crate::e2e::E2eNegotiationState::Disabled)
        };
        let token_or_offer = match route.partner_secret_token.as_deref() {
            Some(tok) if !tok.is_empty() => format!("{}|{}", tok, e2e_wire),
            _ => e2e_wire,
        };
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
            token_or_offer,
            "6.6.133".to_string(),
            "0".to_string(),
        ];
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
            guard.e2e = e2e_state;
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

    pub fn check_for_updates(&self) -> Result<crate::updater::UpdateStatusSnapshot, String> {
        let engine = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.update_engine.clone()
        };
        let snap = engine.check_for_updates()?;
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::sync_state_fields(&mut guard);
        Ok(snap)
    }

    pub fn confirm_update(&self) -> Result<String, String> {
        let engine = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.update_engine.clone()
        };
        let msg = engine.confirm_and_apply()?;
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::sync_state_fields(&mut guard);
        Ok(msg)
    }

    pub fn confirm_update_for_version(
        &self,
        expected_version: Option<&str>,
    ) -> Result<String, String> {
        let engine = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.update_engine.clone()
        };
        let status = engine.confirm_and_apply_for_version(expected_version)?;
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::sync_state_fields(&mut guard);
        Ok(status.message.unwrap_or_default())
    }

    pub fn dismiss_update(&self, version: Option<&str>) {
        let (engine, target_ver, creds_path) = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let v = version
                .map(str::to_string)
                .or_else(|| guard.update_engine.snapshot().latest_version);
            (
                guard.update_engine.clone(),
                v,
                guard.credentials_path.clone(),
            )
        };
        engine.set_dismissed_version(target_ver.clone());
        let mut creds = load_saved_credentials_from(&creds_path);
        creds.dismissed_update_version = target_ver;
        let _ = save_credentials_to(&creds_path, &creds);
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::sync_state_fields(&mut guard);
    }

    pub fn set_update_api_url(&self, url: String) {
        let engine = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.update_engine.clone()
        };
        engine.set_api_url(url);
    }

    pub fn update_snapshot(&self) -> crate::updater::UpdateStatusSnapshot {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.update_engine.snapshot()
    }

    pub fn spawn_background_update_check(&self) {
        if std::env::var("ULTRADESK_DISABLE_AUTO_UPDATE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
        {
            return;
        }
        let session = self.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1200));
            let _ = session.check_for_updates();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_lan_endpoint_no_udp_broadcast_hijack() {
        let session = ControlSession::new();
        assert_eq!(session.resolve_lan_endpoint("99999999"), None);
        assert_eq!(session.resolve_lan_endpoint("12345678"), None);
        assert_eq!(
            session.resolve_lan_endpoint("lan://127.0.0.1:5938"),
            Some(("127.0.0.1".to_string(), 5938))
        );
        assert_eq!(
            session.resolve_lan_endpoint("127.0.0.1:5938"),
            Some(("127.0.0.1".to_string(), 5938))
        );
    }

    #[test]
    fn test_anti_downgrade_established_e2e_rejects_unencrypted_kind_22() {
        let session = ControlSession::new();
        let raw_kind_22 = [1, 0, 0, 0, 22];

        session.handle_session_payload(&raw_kind_22);
        assert_eq!(session.snapshot().state, "disconnected");
        assert_eq!(
            session.snapshot().last_message,
            "Sai mật khẩu (Wrong Password)"
        );

        {
            let mut guard = session.inner.lock().unwrap();
            session.stop_flag.store(false, Ordering::SeqCst);
            guard.state.state = "connected".to_string();
            guard.login_acknowledged = true;
            let cipher = crate::e2e::E2eSessionCipher::new(
                &[7u8; 32],
                "0102030405060708".to_string(),
                crate::e2e::E2eRole::Viewer,
            );
            guard.e2e = crate::e2e::E2eNegotiationState::Established(cipher);
        }

        session.handle_session_payload(&raw_kind_22);
        assert_eq!(session.snapshot().state, "disconnected");
        assert_ne!(
            session.snapshot().last_message,
            "Sai mật khẩu (Wrong Password)"
        );
        {
            let guard = session.inner.lock().unwrap();
            assert!(!matches!(
                guard.e2e,
                crate::e2e::E2eNegotiationState::Disabled
            ));
        }
    }

    #[test]
    fn test_e2e_policy_lan_mandatory_vs_numeric_opportunistic_and_mitm_abort() {
        assert_eq!(
            e2e_channel_policy_for_target("lan://127.0.0.1:5938"),
            crate::e2e::E2eChannelPolicy::MandatoryLan
        );
        assert!(e2e_channel_policy_for_target("lan://127.0.0.1:5938").requires_e2e());
        assert_eq!(
            e2e_channel_policy_for_target("12345678"),
            crate::e2e::E2eChannelPolicy::OpportunisticInternet
        );
        assert!(!e2e_channel_policy_for_target("12345678").requires_e2e());
        assert_eq!(
            e2e_channel_policy_for_target("192.168.1.50:5938"),
            crate::e2e::E2eChannelPolicy::OpportunisticInternet
        );

        let plain_login_reply = encode_byte_array_fields(&[
            &[0u8][..],
            &1024i32.to_le_bytes()[..],
            &768i32.to_le_bytes()[..],
            &[][..],
        ])
        .unwrap();

        let session_lan = ControlSession::new();
        let (_offer_wire, offer_state) =
            crate::e2e::create_viewer_offer_for_password("nonce1", "12345678", "");
        {
            let mut guard = session_lan.inner.lock().unwrap();
            guard.e2e_policy = crate::e2e::E2eChannelPolicy::MandatoryLan;
            guard.e2e = offer_state;
        }
        session_lan.handle_session_payload(&plain_login_reply);
        assert_eq!(session_lan.snapshot().state, "disconnected");
        assert_eq!(
            session_lan.snapshot().last_message,
            "Host did not provide required E2E handshake answer"
        );

        let session_win = ControlSession::new();
        let (_offer_wire2, offer_state2) =
            crate::e2e::create_viewer_offer_for_password("nonce2", "12345678", "");
        {
            let mut guard = session_win.inner.lock().unwrap();
            guard.e2e_policy = crate::e2e::E2eChannelPolicy::OpportunisticInternet;
            guard.e2e = offer_state2;
        }
        session_win.handle_session_payload(&plain_login_reply);
        assert_eq!(session_win.snapshot().state, "connected");
        assert_eq!(
            session_win.inner.lock().unwrap().e2e,
            crate::e2e::E2eNegotiationState::Disabled
        );

        let session_mitm = ControlSession::new();
        let (_offer_wire3, offer_state3) =
            crate::e2e::create_viewer_offer_for_password("nonce3", "12345678", "");
        let forged_ans = format!(
            "E2E1|{}|{}",
            crate::e2e::hex_encode(&crate::e2e::x25519_basepoint(&[7u8; 32])),
            "00".repeat(32)
        );
        let forged_login_reply = encode_byte_array_fields(&[
            &[0u8][..],
            &1024i32.to_le_bytes()[..],
            &768i32.to_le_bytes()[..],
            forged_ans.as_bytes(),
        ])
        .unwrap();
        {
            let mut guard = session_mitm.inner.lock().unwrap();
            guard.e2e_policy = crate::e2e::E2eChannelPolicy::OpportunisticInternet;
            guard.e2e = offer_state3;
        }
        session_mitm.handle_session_payload(&forged_login_reply);
        assert_eq!(session_mitm.snapshot().state, "disconnected");
        assert!(session_mitm.snapshot().error.is_some());
    }

    #[test]
    fn test_pre_login_packets_ignored_when_login_not_acknowledged() {
        let session = ControlSession::new();
        {
            let guard = session.inner.lock().unwrap();
            assert!(!guard.login_acknowledged);
            assert_eq!(guard.frames_applied, 0);
            assert_eq!(guard.audio.chunks_received, 0);
            assert_eq!(guard.clipboard.updates_received, 0);
        }

        let dummy_frame = encode_byte_array_fields(&[
            &[1u8][..],
            &[0u8][..],
            &[0u8][..],
            &[0u8][..],
            &[0u8][..],
            &[0u8][..],
            &[0u8][..],
            &[0u8][..],
            &[0u8][..],
            &[0u8][..],
            &[0u8][..],
        ])
        .unwrap();
        session.handle_session_payload(&dummy_frame);

        let audio_pkt =
            crate::audio::encode_audio_packet(crate::audio::WaveSource::PcSound, &[0u8; 10], false)
                .unwrap();
        session.handle_session_payload(&audio_pkt);

        let clip_pkt = crate::clipboard::encode_clipboard_packet(1, "secret_clip").unwrap();
        session.handle_session_payload(&clip_pkt);

        {
            let guard = session.inner.lock().unwrap();
            assert_eq!(guard.frames_applied, 0);
            assert_eq!(guard.audio.chunks_received, 0);
            assert_eq!(guard.clipboard.local_text, "");
        }
    }

    #[test]
    fn test_update_saved_credentials_scrubs_passwords_when_remember_false() {
        let temp_dir = std::env::temp_dir().join(format!(
            "ultradesk-creds-test-{}-{}",
            std::process::id(),
            random_digits(8)
        ));
        let temp_path = temp_dir.join("credentials.json");
        let session = ControlSession::new();
        {
            let mut guard = session.inner.lock().unwrap();
            guard.credentials_path = temp_path.clone();
        }
        session
            .update_saved_credentials("12345678", "secret_pass", true)
            .unwrap();
        let creds = load_saved_credentials_from(&temp_path);
        assert_eq!(creds.password, "secret_pass");
        assert_eq!(creds.saved_remotes[0].password, "secret_pass");

        session
            .update_saved_credentials("12345678", "secret_pass", false)
            .unwrap();
        let creds = load_saved_credentials_from(&temp_path);
        assert!(creds.password.is_empty());
        assert!(!creds.remember);
        for entry in &creds.saved_remotes {
            assert!(entry.password.is_empty());
        }
        let _ = fs::remove_dir_all(&temp_dir);
    }
}

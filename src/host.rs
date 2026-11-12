use crate::input::{decode_host_input, HostInputEvent, KEY_DOWN, SYS_KEY_DOWN, VK_PACKET};
use crate::protocol::{
    decode_command_fields, decode_transport_body, encode_byte_array_fields, encode_relay_frame,
    md5_hex_lower, FrameStream, DEFAULT_REGISTRATION_HOST,
};
use crate::screen::{compress_rgb555_standard, DisplayLayout, MonitorRect, MonitorSelection};
use crate::server::encode_base64;
use crate::session::{detect_local_ip, RegistrationTransport, SessionTransport};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xproto::{self, ConnectionExt as _, ImageFormat, ImageOrder};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

pub const FIRST_FRAME_NUMBER: i32 = i32::MIN + 2;
pub const HOST_VERSION: &[u8] = b"6.6.133";
pub const DEFAULT_LAN_PORT: u16 = 5938;
pub const DEFAULT_FALLBACK_HOST_MAC: &str = "BC241128BDE1";
pub const SECONDARY_FALLBACK_HOST_MAC: &str = "8C3B4A4E2604";
pub const MAX_FAILURES_BEFORE_LOCKOUT: u32 = 5;
pub const LOCKOUT_DURATION: Duration = Duration::from_secs(5 * 60);

pub fn candidate_registration_macs(primary_mac: &str) -> Vec<String> {
    let mut macs = Vec::with_capacity(3);
    if !primary_mac.is_empty() {
        macs.push(primary_mac.to_string());
    }
    for fallback in [DEFAULT_FALLBACK_HOST_MAC, SECONDARY_FALLBACK_HOST_MAC] {
        if !macs.iter().any(|m| m.eq_ignore_ascii_case(fallback)) {
            macs.push(fallback.to_string());
        }
    }
    macs
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRegistrationReply {
    pub public_port: u16,
    pub id: String,
    pub public_ip: String,
    pub status: i32,
    pub ping_list: String,
    pub ping_host_name_list: String,
    pub opaque_server_token: String,
}

pub fn build_host_registration_command(
    device_id: &str,
    temp_password: &str,
    local_ip: &str,
    computer_name_b64: &str,
    hardware_mac: &str,
    custom_password_proof: Option<&str>,
    user_name: &str,
) -> Result<Vec<u8>, String> {
    let custom_pw = custom_password_proof.unwrap_or("");
    let fields = [
        "3",
        device_id,
        temp_password,
        "0",
        local_ip,
        "0",
        computer_name_b64,
        "6.6.133",
        hardware_mac,
        "",
        "0|6|0|8|0|0",
        "0",
        "",
        custom_pw,
        "0",
        "",
        user_name,
        "0",
        "0",
        "0",
        "0",
        "0",
        "0",
        "0",
        "0",
        "",
        "",
        "",
    ];
    for (i, f) in fields.iter().enumerate() {
        if !f.bytes().all(|b| (32..=126).contains(&b) && b != b':') {
            return Err(format!("invalid host registration field {}", i));
        }
    }
    Ok(fields.join(":").into_bytes())
}

pub fn parse_host_registration_reply(payload: &[u8]) -> Result<HostRegistrationReply, String> {
    let fields = decode_command_fields(payload)?;
    if fields.len() < 11 || fields[0] != "3" {
        return Err("expected command 3 host registration reply".to_string());
    }
    let public_port = fields[1]
        .parse::<u16>()
        .ok()
        .filter(|&p| p != 0 && fields[1].bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| "invalid public port in registration reply".to_string())?;
    let id_num = fields[2]
        .parse::<u64>()
        .ok()
        .filter(|&n| n != 0 && fields[2].bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| "invalid assigned ID in registration reply".to_string())?;
    let status = fields[7]
        .parse::<i32>()
        .map_err(|_| "invalid status in registration reply".to_string())?;
    Ok(HostRegistrationReply {
        public_port,
        id: id_num.to_string(),
        public_ip: fields[3].clone(),
        status,
        ping_list: fields[6].clone(),
        ping_host_name_list: fields[9].clone(),
        opaque_server_token: fields[10].clone(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayNotification {
    pub tag: Option<String>,
    pub hash: Option<String>,
    pub relay_ip: String,
    pub relay_host: String,
}

pub fn parse_relay_notification(text: &str) -> Option<RelayNotification> {
    parse_relay_notification_with_defaults(text, "", "")
}

pub fn parse_relay_notification_with_defaults(
    text: &str,
    default_ips: &str,
    default_hosts: &str,
) -> Option<RelayNotification> {
    parse_relay_notification_with_policy(text, default_ips, default_hosts, false)
}

pub fn parse_relay_notification_with_policy(
    text: &str,
    default_ips: &str,
    default_hosts: &str,
    allow_loopback: bool,
) -> Option<RelayNotification> {
    let is_allowed_ip = |ip: std::net::Ipv4Addr| {
        crate::protocol::is_public_routable_ipv4(ip) || (allow_loopback && ip.is_loopback())
    };
    let is_allowed_host = |h: &str| {
        crate::protocol::is_valid_hostname(h)
            && (allow_loopback || !h.eq_ignore_ascii_case("localhost"))
    };
    let parts: Vec<&str> = text.split(':').collect();
    if parts.is_empty() {
        return None;
    }
    if parts[0] == "11" && parts.len() >= 4 {
        let relay_ip = parts[1].trim().to_string();
        let ip_addr = relay_ip.parse::<std::net::Ipv4Addr>().ok()?;
        if !is_allowed_ip(ip_addr) {
            return None;
        }
        let relay_host = if parts[3].trim().is_empty() {
            DEFAULT_REGISTRATION_HOST.to_string()
        } else {
            let candidate = parts[3].trim();
            if !is_allowed_host(candidate) {
                return None;
            }
            candidate.to_string()
        };
        return Some(RelayNotification {
            tag: None,
            hash: None,
            relay_ip,
            relay_host,
        });
    }
    if parts[0] == "17" && parts.len() >= 5 {
        let tag = if parts[1].is_empty() {
            None
        } else {
            Some(parts[1].to_string())
        };
        let hash = if parts[2].is_empty() {
            None
        } else {
            Some(parts[2].to_string())
        };
        let ip_source = if parts[3].is_empty() {
            default_ips
        } else {
            parts[3]
        };
        let host_source = if parts[3].is_empty() && parts[4].is_empty() {
            default_hosts
        } else {
            parts[4]
        };
        let ips: Vec<&str> = ip_source.split(';').collect();
        let hosts: Vec<&str> = host_source.split(';').collect();
        for (idx, ip_candidate) in ips.iter().enumerate() {
            let ip_trimmed = ip_candidate.trim();
            if let Ok(ip_addr) = ip_trimmed.parse::<std::net::Ipv4Addr>() {
                if !is_allowed_ip(ip_addr) {
                    continue;
                }
                let host_raw = hosts.get(idx).copied().unwrap_or("").trim();
                let relay_host = if host_raw.is_empty() {
                    DEFAULT_REGISTRATION_HOST.to_string()
                } else {
                    if !is_allowed_host(host_raw) {
                        continue;
                    }
                    host_raw.to_string()
                };
                return Some(RelayNotification {
                    tag,
                    hash,
                    relay_ip: ip_trimmed.to_string(),
                    relay_host,
                });
            }
        }
    }
    None
}

pub fn parse_relay_address(payload: &[u8]) -> Option<(u16, String)> {
    let text = std::str::from_utf8(payload).ok()?;
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() < 3 || parts[0] != "0" || !parts[1].bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port = parts[1].parse::<u16>().ok().filter(|&p| p != 0)?;
    let ip_addr = parts[2].trim().parse::<std::net::Ipv4Addr>().ok()?;
    if ip_addr.is_unspecified() || ip_addr.is_broadcast() || ip_addr.is_multicast() {
        return None;
    }
    Some((port, ip_addr.to_string()))
}

pub fn encode_availability_reply(
    notif: &RelayNotification,
    public_port: u16,
    public_ip: &str,
) -> Vec<u8> {
    format!(
        "17:{}:{}:{}:{}:{}:0:|0",
        notif.tag.as_deref().unwrap_or(""),
        notif.hash.as_deref().unwrap_or(""),
        notif.relay_ip,
        public_port,
        public_ip
    )
    .into_bytes()
}

pub fn decode_peer_payload(payload: &[u8], unicode_bytes: bool) -> Option<Vec<u8>> {
    if !unicode_bytes {
        return Some(payload.to_vec());
    }
    if payload.len() > 2_097_152 || !payload.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = payload
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let text = String::from_utf16(&units).ok()?;
    if text
        .chars()
        .all(|c| c as u32 >= 32 || matches!(c as u32, 9 | 10 | 13))
    {
        Some(text.into_bytes())
    } else {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewerLogin {
    pub nonce: String,
    pub password_proof: String,
    pub viewer_width: u32,
    pub viewer_height: u32,
    pub claimed_id: String,
    pub display_name: String,
    pub wants_control: bool,
    pub custom_password_proof: String,
    pub color_mode: i32,
    pub client_version: String,
    pub e2e_offer: Option<String>,
}

pub fn parse_viewer_login(text: &str) -> Option<ViewerLogin> {
    let fields: Vec<&str> = text.split(':').collect();
    if fields.len() < 23 || fields[0] != "login" {
        return None;
    }
    let nonce = fields[1].to_string();
    if nonce.len() != 8 || !nonce.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let password_proof = fields[2].to_ascii_lowercase();
    let viewer_width = fields[3].parse::<u32>().ok()?;
    let viewer_height = fields[4].parse::<u32>().ok()?;
    let claimed_id = fields[10].to_string();
    let display_name = fields[11].to_string();
    let control_mode = fields[12].parse::<i16>().unwrap_or(1);
    let custom_password_proof = fields[13].to_ascii_lowercase();
    let color_mode = fields[16].parse::<i32>().unwrap_or(2);
    let e2e_offer = fields
        .get(20)
        .map(|s| s.trim())
        .filter(|s| s.contains(crate::e2e::E2E_HANDSHAKE_PREFIX))
        .map(ToString::to_string);
    let client_version = fields[21].to_string();

    Some(ViewerLogin {
        nonce,
        password_proof,
        viewer_width,
        viewer_height,
        claimed_id,
        display_name,
        wants_control: control_mode != 2,
        custom_password_proof,
        color_mode,
        client_version,
        e2e_offer,
    })
}

pub fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

pub fn compute_master_password_proof(password: &str) -> String {
    md5_hex_lower(format!("{}{}", " ".repeat(32), password).as_bytes())
}

pub fn verify_login_credentials(
    login: &ViewerLogin,
    temp_password: &str,
    master_password_proof: Option<&str>,
) -> (bool, bool) {
    let expected_temp = md5_hex_lower(format!("{}{}", login.nonce, temp_password).as_bytes());
    if ct_eq(&login.password_proof, &expected_temp) {
        return (true, false);
    }
    if let Some(mp) = master_password_proof {
        if !login.custom_password_proof.is_empty() && ct_eq(&login.custom_password_proof, mp) {
            return (true, true);
        }
    }
    (false, false)
}

#[derive(Debug, Clone, Default)]
pub struct LoginThrottle {
    failures: u32,
    locked_until: Option<Instant>,
    last_failure_at: Option<Instant>,
}

impl LoginThrottle {
    pub fn is_locked(&self, now: Instant) -> bool {
        self.locked_until.is_some_and(|until| now < until)
    }

    pub fn record_failure(&mut self, now: Instant) -> bool {
        self.last_failure_at = Some(now);
        if self.locked_until.is_some_and(|until| now >= until) {
            self.locked_until = None;
            self.failures = 0;
        }
        if self.locked_until.is_some() {
            return false;
        }
        self.failures += 1;
        if self.failures >= MAX_FAILURES_BEFORE_LOCKOUT {
            self.locked_until = Some(now + LOCKOUT_DURATION);
            return true;
        }
        false
    }

    pub fn record_success(&mut self) {
        self.failures = 0;
        self.locked_until = None;
        self.last_failure_at = None;
    }

    pub fn remaining_lockout(&self, now: Instant) -> Option<Duration> {
        self.locked_until
            .and_then(|until| until.checked_duration_since(now))
    }

    pub fn is_expired(&self, now: Instant) -> bool {
        if let Some(until) = self.locked_until {
            now >= until
        } else if let Some(last) = self.last_failure_at {
            now.saturating_duration_since(last) >= LOCKOUT_DURATION
        } else {
            self.failures == 0
        }
    }

    pub fn last_activity(&self) -> Option<Instant> {
        self.locked_until.or(self.last_failure_at)
    }
}

pub fn peer_throttle_key(ip: std::net::IpAddr) -> String {
    match ip {
        std::net::IpAddr::V4(v4) => v4.to_string(),
        std::net::IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                v4.to_string()
            } else if v6.is_loopback() {
                "::1".to_string()
            } else {
                let s = v6.segments();
                format!("v6:{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
        }
    }
}

pub fn encode_host_screen_init(layout: &DisplayLayout) -> Vec<u8> {
    encode_host_screen_init_with_e2e(layout, &[])
}

pub fn encode_host_screen_init_with_e2e(layout: &DisplayLayout, e2e_answer: &[u8]) -> Vec<u8> {
    let width_bytes = (layout.width as i32).to_le_bytes();
    let height_bytes = (layout.height as i32).to_le_bytes();
    let ox = layout.virtual_left;
    let oy = layout.virtual_top;
    let monitor_bounds = if layout.monitors.is_empty() {
        format!("0:0:{}:{}|", layout.width as i32, layout.height as i32)
    } else {
        let mut s = String::new();
        for m in &layout.monitors {
            s.push_str(&format!(
                "{}:{}:{}:{}|",
                m.left - ox,
                m.top - oy,
                m.right - ox,
                m.bottom - oy
            ));
        }
        s
    };
    let vl_bytes = 0i32.to_le_bytes();
    let vt_bytes = 0i32.to_le_bytes();
    let fields: [&[u8]; 11] = [
        &[0],
        &width_bytes,
        &height_bytes,
        e2e_answer,
        &[],
        &[0],
        HOST_VERSION,
        &[1],
        monitor_bounds.as_bytes(),
        &vl_bytes,
        &vt_bytes,
    ];
    encode_byte_array_fields(&fields).unwrap()
}

pub fn offset_host_input_packet(packet: &[u8], offset_x: i32, offset_y: i32) -> Vec<u8> {
    if offset_x == 0 && offset_y == 0 {
        return packet.to_vec();
    }
    if let Some(HostInputEvent::Mouse {
        msg, wparam, x, y, ..
    }) = decode_host_input(packet)
    {
        if msg == 0x0200 {
            return format!(
                "8|||{}|||{}|||{}|||{}",
                0x0200,
                wparam,
                x.saturating_add(offset_x),
                y.saturating_add(offset_y)
            )
            .into_bytes();
        }
        if let Ok(text) = std::str::from_utf8(packet) {
            if text.split("|||").count() == 4 {
                let lp = crate::input::pack_lparam(
                    x.saturating_add(offset_x),
                    y.saturating_add(offset_y),
                );
                return format!("8|||{}|||{}|||{}", msg, wparam, lp).into_bytes();
            }
        }
    }
    packet.to_vec()
}

pub fn encode_host_control_permission(allow_control: bool) -> Vec<u8> {
    encode_byte_array_fields(&[&[49], &[if allow_control { 0 } else { 1 }]]).unwrap()
}

pub fn encode_host_auth_failed() -> Vec<u8> {
    encode_byte_array_fields(&[&[22]]).unwrap()
}

pub fn encode_host_reject() -> Vec<u8> {
    encode_byte_array_fields(&[&[27]]).unwrap()
}

pub fn encode_host_keepalive_reply() -> Vec<u8> {
    encode_byte_array_fields(&[&[20]]).unwrap()
}

pub fn encode_host_chat(text: &str) -> Vec<u8> {
    let utf16 = crate::protocol::encode_senddata_string(text, true);
    encode_byte_array_fields(&[&[12, 0], &utf16]).unwrap()
}

pub fn encode_host_clipboard(text: &str) -> Vec<u8> {
    let utf16 = crate::protocol::encode_senddata_string(text, true);
    if text.chars().count() < 150 {
        encode_byte_array_fields(&[&[5, 0], &utf16]).unwrap()
    } else {
        let zstd_bytes = zstd::bulk::compress(&utf16, 1).unwrap_or_else(|_| utf16.clone());
        let len_bytes = (utf16.len() as i32).to_le_bytes();
        encode_byte_array_fields(&[&[33, 0], &len_bytes, &zstd_bytes]).unwrap()
    }
}

const MAX_SAFE_RELAY_FRAME_PAYLOAD: usize = 950_000;

fn mask_rgb555_lsb(raw: &[u8], mask: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    for chunk in raw.as_chunks::<2>().0 {
        let px = u16::from_le_bytes([chunk[0], chunk[1]]) & mask;
        out.extend_from_slice(&px.to_le_bytes());
    }
    if raw.len() % 2 == 1 {
        out.push(*raw.last().unwrap());
    }
    out
}

pub fn encode_host_frame(
    rect: (u32, u32, u32, u32),
    rgb555_top_down: &[u8],
    seq: i32,
) -> Result<Vec<u8>, String> {
    let mut compressed = compress_rgb555_standard(rgb555_top_down, 1)?;
    if compressed.len() > MAX_SAFE_RELAY_FRAME_PAYLOAD {
        compressed = compress_rgb555_standard(rgb555_top_down, 9)?;
        for mask in [0x7BDEu16, 0x739Cu16, 0x6318u16] {
            if compressed.len() <= MAX_SAFE_RELAY_FRAME_PAYLOAD {
                break;
            }
            let quantized = mask_rgb555_lsb(rgb555_top_down, mask);
            compressed = compress_rgb555_standard(&quantized, 9)?;
        }
    }
    let mut rect_bytes = Vec::with_capacity(8);
    rect_bytes.extend_from_slice(&(rect.0 as u16).to_le_bytes());
    rect_bytes.extend_from_slice(&(rect.1 as u16).to_le_bytes());
    rect_bytes.extend_from_slice(&(rect.2 as u16).to_le_bytes());
    rect_bytes.extend_from_slice(&(rect.3 as u16).to_le_bytes());

    let seq_bytes = seq.to_le_bytes();
    let fields: [&[u8]; 11] = [
        &[1],
        &rect_bytes,
        &compressed,
        &[],
        &seq_bytes,
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
    ];
    encode_byte_array_fields(&fields)
}

fn peer_command_tag(payload: &[u8]) -> &[u8] {
    let stripped = payload.strip_prefix(b"|||").unwrap_or(payload);
    let tag = stripped.split(|&b| b == b'|').next().unwrap_or(&[]);
    if !tag.is_empty() && tag.len() <= 3 && tag.iter().all(|b| b.is_ascii_digit()) {
        tag
    } else {
        b"other"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostPhase {
    None,
    Approval,
    Starting,
    Streaming,
}

#[derive(Debug, PartialEq, Clone)]
pub enum HostAction {
    SendPeer(Vec<u8>),
    PromptApproval(ViewerLogin),
    SessionStarted(ViewerLogin),
    RequestCapture,
    InjectInput(Vec<u8>),
    ReleaseAllInput,
    ChatReceived(String),
    ClipboardReceived(String),
    FileTransferEvent(crate::file_transfer::FileTransferPacket),
    SessionEnded(String),
}

#[derive(Clone, Debug)]
pub struct VideoQos {
    pub last_frame_sent_at: Option<Instant>,
    pub rtt_ema_ms: f32,
    pub target_fps: u32,
    pub static_refresh_remaining: u8,
}

impl Default for VideoQos {
    fn default() -> Self {
        Self {
            last_frame_sent_at: None,
            rtt_ema_ms: 25.0,
            target_fps: 30,
            static_refresh_remaining: 0,
        }
    }
}

impl VideoQos {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn target_interval(&self) -> Duration {
        Duration::from_millis((1000 / self.target_fps.max(1)).max(16) as u64)
    }

    pub fn on_frame_sent(&mut self, now: Instant) {
        self.last_frame_sent_at = Some(now);
    }

    pub fn on_frame_ack(&mut self, now: Instant) {
        if let Some(sent_at) = self.last_frame_sent_at {
            let sample_ms = now.saturating_duration_since(sent_at).as_millis() as f32;
            self.rtt_ema_ms = 0.8 * self.rtt_ema_ms + 0.2 * sample_ms;
            self.target_fps = if self.rtt_ema_ms < 25.0 {
                60
            } else if self.rtt_ema_ms < 60.0 {
                35
            } else if self.rtt_ema_ms < 120.0 {
                25
            } else {
                15
            };
        }
    }
}

pub fn compute_dirty_rect(
    prev: &[u8],
    curr: &[u8],
    width: u32,
    height: u32,
) -> Option<(u32, u32, u32, u32)> {
    let expected_len = (width as usize) * (height as usize) * 2;
    if prev.len() != curr.len() || prev.len() != expected_len || width == 0 || height == 0 {
        return Some((0, 0, width, height));
    }
    if prev == curr {
        return None;
    }
    let row_bytes = (width as usize) * 2;
    let mut top = None;
    let mut bottom = 0usize;

    for y in 0..(height as usize) {
        let start = y * row_bytes;
        let end = start + row_bytes;
        if prev[start..end] != curr[start..end] {
            if top.is_none() {
                top = Some(y);
            }
            bottom = y;
        }
    }

    let y0 = top.unwrap_or(0);
    let y1 = bottom + 1;

    let mut min_x = width as usize;
    let mut max_x = 0usize;

    for y in y0..y1 {
        let row_start = y * row_bytes;
        let prev_row = &prev[row_start..row_start + row_bytes];
        let curr_row = &curr[row_start..row_start + row_bytes];

        for (x, (p_chunk, c_chunk)) in prev_row
            .as_chunks::<2>()
            .0
            .iter()
            .zip(curr_row.as_chunks::<2>().0.iter())
            .enumerate()
        {
            if p_chunk != c_chunk {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
            }
        }
    }

    if min_x > max_x {
        return None;
    }

    let x0 = min_x as u32;
    let w = (max_x - min_x + 1) as u32;
    let y0_u32 = y0 as u32;
    let h = (y1 - y0) as u32;

    Some((x0, y0_u32, w, h))
}

pub struct HostCore {
    pub phase: HostPhase,
    pub temp_password: String,
    pub master_password_proof: Option<String>,
    pub auto_approve: bool,
    pub auto_approve_master: bool,
    pub throttle: LoginThrottle,
    pub active_viewer: Option<ViewerLogin>,
    pub control_allowed: bool,
    pub waiting_ack: bool,
    pub next_frame_seq: i32,
    pub layout: DisplayLayout,
    pub qos: VideoQos,
    pub e2e: crate::e2e::E2eNegotiationState,
    pub files: crate::file_transfer::FileTransferManager,
    pub used_nonces: std::collections::VecDeque<String>,
}

impl HostCore {
    pub fn new(
        temp_password: &str,
        master_password_proof: Option<String>,
        layout: DisplayLayout,
    ) -> Self {
        let mut files = crate::file_transfer::FileTransferManager::default();
        files.id_prefix = "uvh_".to_string();
        files.auto_accept = false;
        files.allow_remote_requests = false;
        Self {
            phase: HostPhase::None,
            temp_password: temp_password.to_string(),
            master_password_proof,
            auto_approve: false,
            auto_approve_master: crate::e2e::env_flag_enabled("ULTRADESK_UNATTENDED_MASTER"),
            throttle: LoginThrottle::default(),
            active_viewer: None,
            control_allowed: true,
            waiting_ack: false,
            next_frame_seq: FIRST_FRAME_NUMBER,
            layout,
            qos: VideoQos::default(),
            e2e: crate::e2e::E2eNegotiationState::Disabled,
            files,
            used_nonces: std::collections::VecDeque::new(),
        }
    }

    pub fn seal_peer_packet(&mut self, payload: &[u8]) -> Vec<u8> {
        if let Some(cipher) = self.e2e.cipher_mut() {
            cipher.seal(payload, false)
        } else {
            payload.to_vec()
        }
    }

    pub fn phase(&self) -> HostPhase {
        self.phase
    }

    pub fn control_allowed(&self) -> bool {
        self.control_allowed
    }

    pub fn active_viewer(&self) -> Option<&ViewerLogin> {
        self.active_viewer.as_ref()
    }

    pub fn handle_packet(&mut self, payload: &[u8], now: Instant) -> Vec<HostAction> {
        let mut actions = Vec::new();
        if payload.starts_with(b"login:") {
            if self.phase != HostPhase::None {
                actions.push(HostAction::SendPeer(encode_host_auth_failed()));
                return actions;
            }
            if self.throttle.is_locked(now) {
                actions.push(HostAction::SendPeer(encode_host_auth_failed()));
                return actions;
            }
            let text = String::from_utf8_lossy(payload);
            let Some(login) = parse_viewer_login(&text) else {
                self.throttle.record_failure(now);
                actions.push(HostAction::SendPeer(encode_host_auth_failed()));
                return actions;
            };

            if self.used_nonces.iter().any(|n| n == &login.nonce) {
                self.throttle.record_failure(now);
                actions.push(HostAction::SendPeer(encode_host_auth_failed()));
                return actions;
            }

            let (valid, is_master) = verify_login_credentials(
                &login,
                &self.temp_password,
                self.master_password_proof.as_deref(),
            );
            if !valid {
                self.throttle.record_failure(now);
                actions.push(HostAction::SendPeer(encode_host_auth_failed()));
                return actions;
            }

            self.throttle.record_success();
            if login.color_mode != 2 {
                actions.push(HostAction::SendPeer(encode_host_reject()));
                return actions;
            }

            if self.used_nonces.len() >= 256 {
                self.used_nonces.pop_front();
            }
            self.used_nonces.push_back(login.nonce.clone());

            let negotiated = if !is_master
                && login
                    .e2e_offer
                    .as_deref()
                    .is_some_and(crate::e2e::offer_uses_derived_temp_secret)
            {
                let derived = crate::e2e::derive_e2e_temp_secret(&login.nonce, &self.temp_password);
                login.e2e_offer.as_deref().and_then(|offer_str| {
                    crate::e2e::accept_viewer_offer_on_host(offer_str, &login.nonce, &derived)
                })
            } else {
                None
            };
            if crate::e2e::env_flag_enabled("ULTRADESK_REQUIRE_E2E") && negotiated.is_none() {
                actions.push(HostAction::SendPeer(encode_host_reject()));
                return actions;
            }

            if self.auto_approve || (is_master && self.auto_approve_master) {
                self.active_viewer = Some(login.clone());
                self.phase = HostPhase::Starting;
                self.control_allowed = login.wants_control;
                self.next_frame_seq = FIRST_FRAME_NUMBER;
                self.waiting_ack = false;
                let (answer_bytes, next_e2e) = match negotiated {
                    Some((ans, cipher)) => {
                        (ans, crate::e2e::E2eNegotiationState::Established(cipher))
                    }
                    None => (Vec::new(), crate::e2e::E2eNegotiationState::Disabled),
                };
                actions.push(HostAction::SendPeer(encode_host_screen_init_with_e2e(
                    &self.layout,
                    &answer_bytes,
                )));
                self.e2e = next_e2e;
                let ctrl_pkt =
                    self.seal_peer_packet(&encode_host_control_permission(self.control_allowed));
                actions.push(HostAction::SendPeer(ctrl_pkt));
                actions.push(HostAction::SessionStarted(login));
            } else {
                self.active_viewer = Some(login.clone());
                self.phase = HostPhase::Approval;
                self.e2e = match negotiated {
                    Some((answer_token, cipher)) => crate::e2e::E2eNegotiationState::Offered {
                        offer: crate::e2e::E2eOfferPayload::HostPending {
                            answer_token,
                            cipher,
                        },
                    },
                    None => crate::e2e::E2eNegotiationState::Disabled,
                };
                actions.push(HostAction::PromptApproval(login));
            }
            return actions;
        }

        let decrypted_buf: Vec<u8>;
        let payload: &[u8] = if let Some(cipher) = self.e2e.cipher_mut() {
            if self.phase == HostPhase::Starting
                && cipher.rx_seqnum == 0
                && !crate::e2e::is_e2e_frame(payload)
            {
                if peer_command_tag(payload) == b"19" {
                    self.phase = HostPhase::None;
                    self.active_viewer = None;
                    self.waiting_ack = false;
                    self.e2e = crate::e2e::E2eNegotiationState::Disabled;
                    actions.push(HostAction::ReleaseAllInput);
                    actions.push(HostAction::SessionEnded("Viewer disconnected".to_string()));
                }
                return actions;
            }
            let Ok((raw, unicode_flag)) = cipher.open(payload) else {
                return self.end_session("E2E decryption or authentication failed");
            };
            decrypted_buf = decode_peer_payload(&raw, unicode_flag).unwrap_or(raw);
            &decrypted_buf
        } else if crate::e2e::is_e2e_frame(payload) {
            return self.end_session("Unexpected E2E frame on unencrypted session");
        } else {
            payload
        };

        let tag = peer_command_tag(payload);

        if tag == b"17" && self.phase == HostPhase::Starting {
            self.phase = HostPhase::Streaming;
            self.waiting_ack = false;
            actions.push(HostAction::RequestCapture);
            return actions;
        }

        if tag == b"18" && self.phase == HostPhase::Streaming && self.waiting_ack {
            self.waiting_ack = false;
            self.qos.on_frame_ack(now);
            actions.push(HostAction::RequestCapture);
            return actions;
        }

        if tag == b"19" {
            self.phase = HostPhase::None;
            self.active_viewer = None;
            self.waiting_ack = false;
            self.e2e = crate::e2e::E2eNegotiationState::Disabled;
            self.files.reset_for_new_session();
            actions.push(HostAction::ReleaseAllInput);
            actions.push(HostAction::SessionEnded("Viewer disconnected".to_string()));
            return actions;
        }

        if tag == b"20" {
            let reply = self.seal_peer_packet(&encode_host_keepalive_reply());
            actions.push(HostAction::SendPeer(reply));
            return actions;
        }

        if tag == b"32" && (payload == b"32|||1" || payload == b"|||32|||1") {
            return self.end_session("Unsupported viewer color mode");
        }

        if payload.starts_with(b"8|||") || payload.starts_with(&[30, b'|', b'|', b'|']) {
            if self.phase == HostPhase::Streaming && self.control_allowed {
                actions.push(HostAction::InjectInput(payload.to_vec()));
            }
            return actions;
        }

        if matches!(self.phase, HostPhase::Starting | HostPhase::Streaming) {
            if tag == b"12" {
                let stripped = payload.strip_prefix(b"|||").unwrap_or(payload);
                if let Some(rest) = stripped.strip_prefix(b"12|||") {
                    let text = String::from_utf8_lossy(rest).to_string();
                    actions.push(HostAction::ChatReceived(text));
                    return actions;
                }
            }
            if (tag == b"5" || tag == b"33")
                && self.phase == HostPhase::Streaming
                && self.control_allowed
            {
                if let Some(clip) = crate::clipboard::decode_clipboard_packet(payload) {
                    actions.push(HostAction::ClipboardReceived(clip.text));
                    return actions;
                }
            }
            if let Some(decoded_ft) = crate::file_transfer::decode_file_transfer_packet(payload) {
                match decoded_ft {
                    crate::file_transfer::FileTransferPacket::ChatMessage { text } => {
                        actions.push(HostAction::ChatReceived(text));
                        return actions;
                    }
                    other => {
                        if self.control_allowed {
                            match self.files.handle_packet(payload) {
                                Ok(Some((ft_pkt, replies))) => {
                                    for reply in replies {
                                        let sealed = self.seal_peer_packet(&reply);
                                        actions.push(HostAction::SendPeer(sealed));
                                    }
                                    actions.push(HostAction::FileTransferEvent(ft_pkt));
                                    return actions;
                                }
                                Err(err) => {
                                    if matches!(
                                        other,
                                        crate::file_transfer::FileTransferPacket::Request { .. }
                                    ) {
                                        let notice = encode_host_chat(&format!(
                                            "File request failed: {}",
                                            err
                                        ));
                                        let sealed = self.seal_peer_packet(&notice);
                                        actions.push(HostAction::SendPeer(sealed));
                                    }
                                    return actions;
                                }
                                Ok(None) => {}
                            }
                        } else if let crate::file_transfer::FileTransferPacket::Offer {
                            file_id,
                            ..
                        } = other
                        {
                            if let Ok(cancel_pkt) = crate::file_transfer::encode_file_cancel(
                                &file_id,
                                self.files.modern_protocol,
                            ) {
                                let sealed = self.seal_peer_packet(&cancel_pkt);
                                actions.push(HostAction::SendPeer(sealed));
                            }
                            return actions;
                        }
                    }
                }
            }
        }

        actions
    }

    pub fn approve(&mut self, allow_control: bool) -> Vec<HostAction> {
        let mut actions = Vec::new();
        if self.phase != HostPhase::Approval {
            return actions;
        }
        self.phase = HostPhase::Starting;
        self.control_allowed = allow_control;
        self.next_frame_seq = FIRST_FRAME_NUMBER;
        self.waiting_ack = false;
        let (answer_bytes, next_e2e) = match std::mem::take(&mut self.e2e) {
            crate::e2e::E2eNegotiationState::Offered {
                offer:
                    crate::e2e::E2eOfferPayload::HostPending {
                        answer_token,
                        cipher,
                    },
            } => (
                answer_token,
                crate::e2e::E2eNegotiationState::Established(cipher),
            ),
            other => (Vec::new(), other),
        };
        actions.push(HostAction::SendPeer(encode_host_screen_init_with_e2e(
            &self.layout,
            &answer_bytes,
        )));
        self.e2e = next_e2e;
        let ctrl_pkt = self.seal_peer_packet(&encode_host_control_permission(allow_control));
        actions.push(HostAction::SendPeer(ctrl_pkt));
        if let Some(ref v) = self.active_viewer {
            actions.push(HostAction::SessionStarted(v.clone()));
        }
        actions
    }

    pub fn decline(&mut self) -> Vec<HostAction> {
        let mut actions = Vec::new();
        self.phase = HostPhase::None;
        self.active_viewer = None;
        self.e2e = crate::e2e::E2eNegotiationState::Disabled;
        self.files.reset_for_new_session();
        actions.push(HostAction::SendPeer(encode_host_reject()));
        actions.push(HostAction::SessionEnded("Session declined".to_string()));
        actions
    }

    pub fn set_control_allowed(&mut self, allowed: bool) -> Vec<HostAction> {
        let mut actions = Vec::new();
        if !matches!(self.phase, HostPhase::Starting | HostPhase::Streaming) {
            return actions;
        }
        self.control_allowed = allowed;
        if !allowed {
            actions.push(HostAction::ReleaseAllInput);
        }
        let ctrl_pkt = self.seal_peer_packet(&encode_host_control_permission(allowed));
        actions.push(HostAction::SendPeer(ctrl_pkt));
        actions
    }

    pub fn update_layout(&mut self, layout: DisplayLayout) -> Vec<HostAction> {
        let mut actions = Vec::new();
        if self.layout == layout {
            return actions;
        }
        self.layout = layout;
        if matches!(self.phase, HostPhase::Starting | HostPhase::Streaming) {
            self.phase = HostPhase::Starting;
            self.waiting_ack = false;
            actions.push(HostAction::ReleaseAllInput);
            let init_pkt = self.seal_peer_packet(&encode_host_screen_init(&self.layout));
            let ctrl_pkt =
                self.seal_peer_packet(&encode_host_control_permission(self.control_allowed));
            actions.push(HostAction::SendPeer(init_pkt));
            actions.push(HostAction::SendPeer(ctrl_pkt));
        }
        actions
    }

    pub fn on_frame_acknowledged(&mut self) -> Vec<HostAction> {
        let mut actions = Vec::new();
        if self.phase == HostPhase::Streaming && self.waiting_ack {
            self.waiting_ack = false;
            self.qos.on_frame_ack(Instant::now());
            actions.push(HostAction::RequestCapture);
        }
        actions
    }

    pub fn send_frame_rect(
        &mut self,
        rect: (u32, u32, u32, u32),
        rgb555_top_down: &[u8],
    ) -> Result<Vec<HostAction>, String> {
        if self.phase != HostPhase::Streaming || self.waiting_ack {
            return Ok(Vec::new());
        }
        let seq = self.next_frame_seq;
        self.next_frame_seq = self.next_frame_seq.wrapping_add(1);
        self.waiting_ack = true;
        self.qos.on_frame_sent(Instant::now());
        let packet = self.seal_peer_packet(&encode_host_frame(rect, rgb555_top_down, seq)?);
        Ok(vec![HostAction::SendPeer(packet)])
    }

    pub fn send_frame(&mut self, rgb555_top_down: &[u8]) -> Result<Vec<HostAction>, String> {
        self.send_frame_rect(
            (0, 0, self.layout.width, self.layout.height),
            rgb555_top_down,
        )
    }

    pub fn end_session(&mut self, reason: &str) -> Vec<HostAction> {
        let mut actions = Vec::new();
        if self.active_viewer.is_some() {
            let reject_pkt = self.seal_peer_packet(&encode_host_reject());
            actions.push(HostAction::SendPeer(reject_pkt));
        }
        self.phase = HostPhase::None;
        self.active_viewer = None;
        self.waiting_ack = false;
        self.e2e = crate::e2e::E2eNegotiationState::Disabled;
        self.files.reset_for_new_session();
        actions.push(HostAction::ReleaseAllInput);
        actions.push(HostAction::SessionEnded(reason.to_string()));
        actions
    }
}

pub trait Capturer: Send {
    fn enumerate_displays(&mut self) -> Result<Vec<MonitorRect>, String>;
    fn capture_screen(&mut self, rect: MonitorRect) -> Result<Vec<u8>, String>;
}

pub trait Injector: Send {
    fn inject_event(&mut self, packet: &[u8]) -> Result<(), String>;
    fn release_all(&mut self);
}

pub struct HeadlessCapturer {
    pub width: u32,
    pub height: u32,
    pub cached: Option<(usize, usize, Vec<u8>)>,
}

impl HeadlessCapturer {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            cached: None,
        }
    }
}

impl Capturer for HeadlessCapturer {
    fn enumerate_displays(&mut self) -> Result<Vec<MonitorRect>, String> {
        Ok(vec![MonitorRect {
            left: 0,
            top: 0,
            right: self.width as i32,
            bottom: self.height as i32,
        }])
    }

    fn capture_screen(&mut self, rect: MonitorRect) -> Result<Vec<u8>, String> {
        let w = rect.width() as usize;
        let h = rect.height() as usize;
        if let Some((cw, ch, ref bytes)) = self.cached {
            if cw == w && ch == h {
                return Ok(bytes.clone());
            }
        }
        let mut raw = Vec::with_capacity(w * h * 2);
        for y in 0..h {
            for x in 0..w {
                let px = (((x % 31) << 10) | (((y / 2) % 31) << 5) | (x % 31)) as u16;
                raw.extend_from_slice(&px.to_le_bytes());
            }
        }
        self.cached = Some((w, h, raw.clone()));
        Ok(raw)
    }
}

#[derive(Default)]
pub struct HeadlessInjector {
    #[cfg(test)]
    pub injected_packets: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl HeadlessInjector {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Injector for HeadlessInjector {
    fn inject_event(&mut self, packet: &[u8]) -> Result<(), String> {
        #[cfg(test)]
        self.injected_packets.lock().unwrap().push(packet.to_vec());
        let _ = packet;
        Ok(())
    }

    fn release_all(&mut self) {
        #[cfg(test)]
        self.injected_packets
            .lock()
            .unwrap()
            .push(b"RELEASE_ALL".to_vec());
    }
}

pub struct X11CommandInjector {
    #[cfg(test)]
    pub injected_packets: Arc<Mutex<Vec<Vec<u8>>>>,
    pub use_xdotool: bool,
    tx: Option<std::sync::mpsc::SyncSender<Vec<String>>>,
}

impl X11CommandInjector {
    pub fn new(use_xdotool: bool) -> Self {
        let tx = if use_xdotool {
            let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<String>>(256);
            std::thread::spawn(move || {
                while let Ok(args) = rx.recv() {
                    let _ = Command::new("xdotool").args(&args).status();
                }
            });
            Some(tx)
        } else {
            None
        };
        Self {
            #[cfg(test)]
            injected_packets: Arc::new(Mutex::new(Vec::new())),
            use_xdotool,
            tx,
        }
    }
}

impl Injector for X11CommandInjector {
    fn inject_event(&mut self, packet: &[u8]) -> Result<(), String> {
        #[cfg(test)]
        self.injected_packets.lock().unwrap().push(packet.to_vec());
        let Some(ref tx) = self.tx else {
            return Ok(());
        };
        if let Some(evt) = decode_host_input(packet) {
            let args: Option<Vec<String>> = match evt {
                HostInputEvent::Mouse {
                    msg, x, y, wheel, ..
                } => match msg {
                    0x0200 => Some(vec!["mousemove".to_string(), x.to_string(), y.to_string()]),
                    0x0201 => Some(vec!["mousedown".to_string(), "1".to_string()]),
                    0x0202 => Some(vec!["mouseup".to_string(), "1".to_string()]),
                    0x0204 => Some(vec!["mousedown".to_string(), "3".to_string()]),
                    0x0205 => Some(vec!["mouseup".to_string(), "3".to_string()]),
                    0x0207 => Some(vec!["mousedown".to_string(), "2".to_string()]),
                    0x0208 => Some(vec!["mouseup".to_string(), "2".to_string()]),
                    0x020A => {
                        let btn = if wheel >= 0 { "4" } else { "5" };
                        Some(vec!["click".to_string(), btn.to_string()])
                    }
                    _ => None,
                },
                HostInputEvent::Key { kind, vk, ch, .. } => {
                    if vk == VK_PACKET && (kind == KEY_DOWN || kind == SYS_KEY_DOWN) && ch >= 0x20 {
                        char::from_u32(ch as u32)
                            .map(|c| vec!["type".to_string(), "--".to_string(), c.to_string()])
                    } else {
                        None
                    }
                }
            };
            if let Some(cmd_args) = args {
                let _ = tx.try_send(cmd_args);
            }
        }
        Ok(())
    }

    fn release_all(&mut self) {
        #[cfg(test)]
        self.injected_packets
            .lock()
            .unwrap()
            .push(b"RELEASE_ALL".to_vec());
    }
}

pub const STRIP_ROWS: u16 = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct X11PixelFormat {
    pub bits_per_pixel: u8,
    pub scanline_pad: u8,
    pub msb_first: bool,
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
}

impl X11PixelFormat {
    pub fn stride(&self, width: usize) -> usize {
        let pad = self.scanline_pad.max(8) as usize;
        (width * self.bits_per_pixel as usize).div_ceil(pad) * pad / 8
    }
}

fn channel5(pixel: u32, mask: u32) -> u16 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).count_ones();
    let v = (pixel & mask) >> shift;
    if bits >= 5 {
        (v >> (bits - 5)) as u16
    } else {
        (v << (5 - bits)) as u16
    }
}

pub fn zpixmap_to_rgb555(
    data: &[u8],
    width: usize,
    rows: usize,
    f: &X11PixelFormat,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let bpp = f.bits_per_pixel as usize;
    if ![16, 24, 32].contains(&bpp) {
        return Err(format!("unsupported X11 pixel size {bpp} bits"));
    }
    let stride = f.stride(width);
    if data.len() < stride * rows {
        return Err("X11 returned a short image".to_string());
    }
    let bytes = bpp / 8;
    if bytes == 4
        && !f.msb_first
        && f.red_mask == 0xff0000
        && f.green_mask == 0xff00
        && f.blue_mask == 0xff
    {
        for y in (0..rows).rev() {
            let row = &data[y * stride..y * stride + width * 4];
            for px in row.as_chunks::<4>().0 {
                let w = (((px[2] as u16) >> 3) << 10)
                    | (((px[1] as u16) >> 3) << 5)
                    | ((px[0] as u16) >> 3);
                out.extend_from_slice(&w.to_le_bytes());
            }
        }
        return Ok(());
    }
    for y in (0..rows).rev() {
        let row = &data[y * stride..y * stride + width * bytes];
        for px in row.chunks_exact(bytes) {
            let p = match (bytes, f.msb_first) {
                (4, false) => u32::from_le_bytes([px[0], px[1], px[2], px[3]]),
                (4, true) => u32::from_be_bytes([px[0], px[1], px[2], px[3]]),
                (3, false) => u32::from_le_bytes([px[0], px[1], px[2], 0]),
                (3, true) => u32::from_be_bytes([0, px[0], px[1], px[2]]),
                (_, false) => u16::from_le_bytes([px[0], px[1]]) as u32,
                (_, true) => u16::from_be_bytes([px[0], px[1]]) as u32,
            };
            let w = (channel5(p, f.red_mask) << 10)
                | (channel5(p, f.green_mask) << 5)
                | channel5(p, f.blue_mask);
            out.extend_from_slice(&w.to_le_bytes());
        }
    }
    Ok(())
}

pub fn vk_keysym(vk: u8, scan: u16) -> Option<u32> {
    let extended = scan & 0xff00 == 0xe000 || scan & 0x0100 != 0;
    Some(match vk {
        0x08 => 0xff08,
        0x09 => 0xff09,
        0x0c => 0xff0b,
        0x0d => {
            if extended {
                0xff8d
            } else {
                0xff0d
            }
        }
        0x10 => {
            if scan & 0xff == 0x36 {
                0xffe2
            } else {
                0xffe1
            }
        }
        0x11 => {
            if extended {
                0xffe4
            } else {
                0xffe3
            }
        }
        0x12 => {
            if extended {
                0xffea
            } else {
                0xffe9
            }
        }
        0x13 => 0xff13,
        0x14 => 0xffe5,
        0x1b => 0xff1b,
        0x20 => 0x20,
        0x21 => 0xff55,
        0x22 => 0xff56,
        0x23 => 0xff57,
        0x24 => 0xff50,
        0x25 => 0xff51,
        0x26 => 0xff52,
        0x27 => 0xff53,
        0x28 => 0xff54,
        0x2c => 0xff61,
        0x2d => 0xff63,
        0x2e => 0xffff,
        0x30..=0x39 => vk as u32,
        0x41..=0x5a => vk as u32 + 0x20,
        0x5b => 0xffeb,
        0x5c => 0xffec,
        0x5d => 0xff67,
        0x60..=0x69 => 0xffb0 + (vk - 0x60) as u32,
        0x6a => 0xffaa,
        0x6b => 0xffab,
        0x6c => 0xffac,
        0x6d => 0xffad,
        0x6e => 0xffae,
        0x6f => 0xffaf,
        0x70..=0x87 => 0xffbe + (vk - 0x70) as u32,
        0x90 => 0xff7f,
        0x91 => 0xff14,
        0xa0 => 0xffe1,
        0xa1 => 0xffe2,
        0xa2 => 0xffe3,
        0xa3 => 0xffe4,
        0xa4 => 0xffe9,
        0xa5 => 0xffea,
        0xba => 0x3b,
        0xbb => 0x3d,
        0xbc => 0x2c,
        0xbd => 0x2d,
        0xbe => 0x2e,
        0xbf => 0x2f,
        0xc0 => 0x60,
        0xdb => 0x5b,
        0xdc => 0x5c,
        0xdd => 0x5d,
        0xde => 0x27,
        0xe2 => 0x3c,
        _ => return None,
    })
}

pub fn char_keysym(ch: u16) -> Option<u32> {
    match ch {
        0x20..=0x7e | 0xa0..=0xff => Some(ch as u32),
        0xd800..=0xdfff | 0..=0x1f | 0x7f..=0x9f => None,
        _ => Some(0x0100_0000 | ch as u32),
    }
}

pub fn evdev_keycode(scan: u16) -> Option<u32> {
    let make = (scan & 0xff) as u32;
    if scan & 0xff00 == 0 {
        return (1..=0x58).contains(&make).then_some(make);
    }
    if scan & 0xff00 != 0xe000 && scan & 0x0100 == 0 {
        return None;
    }
    Some(match make {
        0x1c => 96,
        0x1d => 97,
        0x35 => 98,
        0x37 => 99,
        0x38 => 100,
        0x47 => 102,
        0x48 => 103,
        0x49 => 104,
        0x4b => 105,
        0x4d => 106,
        0x4f => 107,
        0x50 => 108,
        0x51 => 109,
        0x52 => 110,
        0x53 => 111,
        0x5b => 125,
        0x5c => 126,
        0x5d => 127,
        _ => return None,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum PointerAction {
    Move(i32, i32),
    Button(u8, bool),
    Wheel(i32),
    None,
}

pub fn pointer_action(event: &HostInputEvent, wheel_remainder: &mut i32) -> PointerAction {
    let HostInputEvent::Mouse {
        msg, x, y, wheel, ..
    } = *event
    else {
        return PointerAction::None;
    };
    match msg {
        0x0200 => PointerAction::Move(x, y),
        0x020A => {
            *wheel_remainder += wheel as i32;
            let steps = *wheel_remainder / 120;
            *wheel_remainder %= 120;
            if steps == 0 {
                PointerAction::None
            } else {
                PointerAction::Wheel(steps)
            }
        }
        0x0201 | 0x0203 => PointerAction::Button(1, true),
        0x0202 => PointerAction::Button(1, false),
        0x0204 | 0x0206 => PointerAction::Button(3, true),
        0x0205 => PointerAction::Button(3, false),
        0x0207 | 0x0209 => PointerAction::Button(2, true),
        0x0208 => PointerAction::Button(2, false),
        _ => PointerAction::None,
    }
}

struct X11ShmSegment {
    seg: u32,
    addr: *mut u8,
    size: usize,
}

unsafe impl Send for X11ShmSegment {}

impl X11ShmSegment {
    fn create(conn: &RustConnection, size: usize) -> Option<Self> {
        unsafe {
            let id = libc::shmget(libc::IPC_PRIVATE, size, libc::IPC_CREAT | 0o600);
            if id < 0 {
                return None;
            }
            let addr = libc::shmat(id, std::ptr::null(), libc::SHM_RDONLY);
            let ok = addr as isize != -1;
            let seg = conn.generate_id().ok();
            let attached = ok
                && seg.is_some_and(|seg| {
                    conn.shm_attach(seg, id as u32, false)
                        .ok()
                        .is_some_and(|c| c.check().is_ok())
                });
            libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut());
            if !attached {
                if ok {
                    libc::shmdt(addr);
                }
                return None;
            }
            Some(Self {
                seg: seg.unwrap(),
                addr: addr as *mut u8,
                size,
            })
        }
    }

    fn release(&mut self, conn: &RustConnection) {
        let _ = conn.shm_detach(self.seg);
        let _ = conn.flush();
        unsafe {
            libc::shmdt(self.addr as *const libc::c_void);
        }
    }
}

pub struct X11Capturer {
    conn: RustConnection,
    root: xproto::Window,
    format: X11PixelFormat,
    pub shm_supported: bool,
    shm: Option<X11ShmSegment>,
}

impl X11Capturer {
    pub fn connect() -> Result<Self, String> {
        if std::env::var_os("DISPLAY").is_none_or(|d| d.is_empty()) {
            return Err("DISPLAY not set".to_string());
        }
        let (conn, screen_num) =
            x11rb::connect(None).map_err(|e| format!("Cannot open X11 display: {e}"))?;
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        let depth = screen.root_depth;
        let pf = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == depth)
            .ok_or_else(|| "X11 has no pixmap format for root depth".to_string())?;
        let visual = screen
            .allowed_depths
            .iter()
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual_id == screen.root_visual)
            .ok_or_else(|| "X11 root visual not found".to_string())?;
        let format = X11PixelFormat {
            bits_per_pixel: pf.bits_per_pixel,
            scanline_pad: pf.scanline_pad,
            msb_first: setup.image_byte_order == ImageOrder::MSB_FIRST,
            red_mask: visual.red_mask,
            green_mask: visual.green_mask,
            blue_mask: visual.blue_mask,
        };
        let root = screen.root;
        let shm_supported = std::env::var_os("ULTRADESK_NO_SHM").is_none()
            && std::env::var_os("ULTRAVIEWER_NO_SHM").is_none()
            && conn
                .extension_information(x11rb::protocol::shm::X11_EXTENSION_NAME)
                .map_err(|e| e.to_string())?
                .is_some();
        if conn
            .extension_information(x11rb::protocol::randr::X11_EXTENSION_NAME)
            .map_err(|e| e.to_string())?
            .is_some()
        {
            if let Ok(cookie) = conn.randr_query_version(1, 5) {
                let _ = cookie.reply();
            }
        }
        let mut capture = Self {
            conn,
            root,
            format,
            shm_supported,
            shm: None,
        };
        if capture.shm_supported {
            let size = capture.format.stride(1).max(4096);
            capture.shm = X11ShmSegment::create(&capture.conn, size);
            capture.shm_supported = capture.shm.is_some();
        }
        Ok(capture)
    }

    fn root_rect(&self) -> Result<MonitorRect, String> {
        let g = self
            .conn
            .get_geometry(self.root)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok(MonitorRect {
            left: 0,
            top: 0,
            right: g.width as i32,
            bottom: g.height as i32,
        })
    }

    fn capture_shm(&mut self, rect: MonitorRect) -> Option<Vec<u8>> {
        let w = rect.width() as usize;
        let h = rect.height() as usize;
        let size = self.format.stride(w) * h;
        if self.shm.as_ref().is_none_or(|s| s.size < size) {
            if let Some(mut old) = self.shm.take() {
                old.release(&self.conn);
            }
            self.shm = X11ShmSegment::create(&self.conn, size);
        }
        let shm = self.shm.as_ref()?;
        let reply = self
            .conn
            .shm_get_image(
                self.root,
                rect.left as i16,
                rect.top as i16,
                w as u16,
                h as u16,
                !0,
                ImageFormat::Z_PIXMAP.into(),
                shm.seg,
                0,
            )
            .ok()?
            .reply()
            .ok()?;
        if (reply.size as usize) < size {
            return None;
        }
        let data = unsafe { std::slice::from_raw_parts(shm.addr, size) };
        let mut out = Vec::with_capacity(w * h * 2);
        zpixmap_to_rgb555(data, w, h, &self.format, &mut out).ok()?;
        Some(out)
    }

    fn capture_get_image(&self, rect: MonitorRect) -> Result<Vec<u8>, String> {
        let w = rect.width() as usize;
        let h = rect.height() as i32;
        let mut out = Vec::with_capacity(w * h as usize * 2);
        let mut bottom = h;
        while bottom > 0 {
            let rows = bottom.min(STRIP_ROWS as i32);
            let top = bottom - rows;
            let reply = self
                .conn
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    self.root,
                    rect.left as i16,
                    (rect.top + top) as i16,
                    w as u16,
                    rows as u16,
                    !0,
                )
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|e| format!("X11 screen capture failed: {e}"))?;
            zpixmap_to_rgb555(&reply.data, w, rows as usize, &self.format, &mut out)?;
            bottom = top;
        }
        Ok(out)
    }
}

impl Drop for X11Capturer {
    fn drop(&mut self) {
        if let Some(mut shm) = self.shm.take() {
            shm.release(&self.conn);
        }
    }
}

pub fn rgb555_bottom_up_to_top_down(bottom_up: &[u8], width: usize) -> Vec<u8> {
    let stride = width * 2;
    if stride == 0 || bottom_up.is_empty() || !bottom_up.len().is_multiple_of(stride) {
        return bottom_up.to_vec();
    }
    let mut top_down = Vec::with_capacity(bottom_up.len());
    for row in bottom_up.chunks_exact(stride).rev() {
        top_down.extend_from_slice(row);
    }
    top_down
}

impl Capturer for X11Capturer {
    fn enumerate_displays(&mut self) -> Result<Vec<MonitorRect>, String> {
        let root = self.root_rect()?;
        let monitors = self
            .conn
            .randr_get_monitors(self.root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.monitors)
            .unwrap_or_default();
        let mut list: Vec<(bool, MonitorRect)> = monitors
            .iter()
            .filter_map(|m| {
                let left = m.x as i32;
                let top = m.y as i32;
                let right = (left + m.width as i32).min(root.right);
                let bottom = (top + m.height as i32).min(root.bottom);
                (left >= 0 && top >= 0 && right > left && bottom > top).then_some((
                    m.primary,
                    MonitorRect {
                        left,
                        top,
                        right,
                        bottom,
                    },
                ))
            })
            .collect();
        list.dedup_by(|a, b| a.1 == b.1);
        list.sort_by_key(|(primary, r)| (!*primary, r.left, r.top));
        if list.is_empty() {
            return Ok(vec![root]);
        }
        Ok(list.into_iter().map(|(_, r)| r).collect())
    }

    fn capture_screen(&mut self, rect: MonitorRect) -> Result<Vec<u8>, String> {
        let width = rect.width() as usize;
        if self.shm_supported {
            if let Some(pixels) = self.capture_shm(rect) {
                return Ok(rgb555_bottom_up_to_top_down(&pixels, width));
            }
            self.shm_supported = false;
        }
        let pixels = self.capture_get_image(rect)?;
        Ok(rgb555_bottom_up_to_top_down(&pixels, width))
    }
}

pub struct X11Injector {
    conn: RustConnection,
    root: xproto::Window,
    min_keycode: u8,
    keysyms_per_keycode: u8,
    keycodes: HashMap<u32, u8>,
    spare_keycode: Option<u8>,
    pointer: Option<(i16, i16)>,
    held_keys: Vec<u8>,
    held_buttons: Vec<u8>,
    wheel_remainder: i32,
}

impl X11Injector {
    pub fn connect() -> Result<Self, String> {
        if std::env::var_os("DISPLAY").is_none_or(|d| d.is_empty()) {
            return Err("DISPLAY not set".to_string());
        }
        let (conn, screen_num) =
            x11rb::connect(None).map_err(|e| format!("Cannot open X11 display: {e}"))?;
        if conn
            .extension_information(x11rb::protocol::xtest::X11_EXTENSION_NAME)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Err("X server has no XTEST extension".to_string());
        }
        let setup = conn.setup();
        let root = setup.roots[screen_num].root;
        let (min_keycode, max_keycode) = (setup.min_keycode, setup.max_keycode);
        let mut input = Self {
            conn,
            root,
            min_keycode,
            keysyms_per_keycode: 0,
            keycodes: HashMap::new(),
            spare_keycode: None,
            pointer: None,
            held_keys: Vec::new(),
            held_buttons: Vec::new(),
            wheel_remainder: 0,
        };
        input.load_keymap(max_keycode)?;
        Ok(input)
    }

    fn load_keymap(&mut self, max_keycode: u8) -> Result<(), String> {
        let count = max_keycode - self.min_keycode + 1;
        let map = self
            .conn
            .get_keyboard_mapping(self.min_keycode, count)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let per = map.keysyms_per_keycode as usize;
        self.keysyms_per_keycode = map.keysyms_per_keycode;
        for column in 0..per {
            for (i, syms) in map.keysyms.chunks(per.max(1)).enumerate() {
                let keycode = self.min_keycode + i as u8;
                if syms[column] != 0 {
                    self.keycodes.entry(syms[column]).or_insert(keycode);
                }
            }
        }
        self.spare_keycode = map
            .keysyms
            .chunks(per.max(1))
            .enumerate()
            .rev()
            .find(|(_, syms)| syms.iter().all(|&s| s == 0))
            .map(|(i, _)| self.min_keycode + i as u8);
        Ok(())
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) -> Result<(), String> {
        self.conn
            .xtest_fake_input(kind, detail, x11rb::CURRENT_TIME, self.root, x, y, 0)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn button(&mut self, button: u8, pressed: bool) -> Result<(), String> {
        let Some((x, y)) = self.pointer else {
            return Ok(());
        };
        if pressed {
            if !self.held_buttons.contains(&button) {
                self.held_buttons.push(button);
            }
        } else if let Some(i) = self.held_buttons.iter().position(|&b| b == button) {
            self.held_buttons.remove(i);
        } else {
            return Ok(());
        }
        let kind = if pressed {
            xproto::BUTTON_PRESS_EVENT
        } else {
            xproto::BUTTON_RELEASE_EVENT
        };
        self.fake(kind, button, x, y)
    }

    fn key(&mut self, keycode: u8, pressed: bool) -> Result<(), String> {
        if pressed {
            if !self.held_keys.contains(&keycode) {
                self.held_keys.push(keycode);
            }
        } else if let Some(i) = self.held_keys.iter().position(|&k| k == keycode) {
            self.held_keys.remove(i);
        } else {
            return Ok(());
        }
        self.fake(
            if pressed {
                xproto::KEY_PRESS_EVENT
            } else {
                xproto::KEY_RELEASE_EVENT
            },
            keycode,
            0,
            0,
        )
    }

    fn keycode_for(&self, vk: u8, scan: u16) -> Option<u8> {
        if let Some(code) = vk_keysym(vk, scan).and_then(|s| self.keycodes.get(&s)) {
            return Some(*code);
        }
        if vk == 0x12 && scan & 0xff00 == 0xe000 {
            if let Some(code) = self.keycodes.get(&0xfe03) {
                return Some(*code);
            }
        }
        evdev_keycode(scan).map(|k| (k + 8) as u8)
    }

    fn type_char(&mut self, ch: u16) -> Result<(), String> {
        let (Some(code), Some(sym)) = (self.spare_keycode, char_keysym(ch)) else {
            return Ok(());
        };
        if let Some(&existing) = self.keycodes.get(&sym) {
            self.fake(xproto::KEY_PRESS_EVENT, existing, 0, 0)?;
            return self.fake(xproto::KEY_RELEASE_EVENT, existing, 0, 0);
        }
        let per = self.keysyms_per_keycode.max(1);
        let syms = vec![sym; per as usize];
        self.conn
            .change_keyboard_mapping(1, code, per, &syms)
            .map_err(|e| e.to_string())?;
        self.conn.sync().map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(15));
        self.fake(xproto::KEY_PRESS_EVENT, code, 0, 0)?;
        self.fake(xproto::KEY_RELEASE_EVENT, code, 0, 0)?;
        self.conn.sync().map_err(|e| e.to_string())?;
        Ok(())
    }
}

impl Injector for X11Injector {
    fn inject_event(&mut self, packet: &[u8]) -> Result<(), String> {
        let Some(event) = decode_host_input(packet) else {
            return Ok(());
        };
        match pointer_action(&event, &mut self.wheel_remainder) {
            PointerAction::Move(x, y) => {
                let (x, y) = (
                    x.clamp(0, i16::MAX as i32) as i16,
                    y.clamp(0, i16::MAX as i32) as i16,
                );
                self.pointer = Some((x, y));
                self.fake(xproto::MOTION_NOTIFY_EVENT, 0, x, y)?;
            }
            PointerAction::Button(b, pressed) => self.button(b, pressed)?,
            PointerAction::Wheel(steps) => {
                let button = if steps > 0 { 4 } else { 5 };
                for _ in 0..steps.unsigned_abs() {
                    self.button(button, true)?;
                    self.button(button, false)?;
                }
            }
            PointerAction::None => {}
        }
        if let HostInputEvent::Key {
            kind, vk, scan, ch, ..
        } = event
        {
            let down = kind == KEY_DOWN || kind == SYS_KEY_DOWN;
            if vk == VK_PACKET {
                if down {
                    self.type_char(ch)?;
                }
            } else if let Some(code) = self.keycode_for(vk, scan) {
                if down && self.held_keys.contains(&code) {
                    self.fake(xproto::KEY_PRESS_EVENT, code, 0, 0)?;
                } else {
                    self.key(code, down)?;
                }
            }
        }
        self.conn.flush().map_err(|e| e.to_string())?;
        Ok(())
    }

    fn release_all(&mut self) {
        for code in std::mem::take(&mut self.held_keys).into_iter().rev() {
            let _ = self.fake(xproto::KEY_RELEASE_EVENT, code, 0, 0);
        }
        if let Some((x, y)) = self.pointer {
            for b in std::mem::take(&mut self.held_buttons) {
                let _ = self.fake(xproto::BUTTON_RELEASE_EVENT, b, x, y);
            }
        }
        self.held_buttons.clear();
        self.wheel_remainder = 0;
        let _ = self.conn.flush();
    }
}

pub fn is_wayland_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland"))
}

pub fn parse_ppm_to_rgb555_top_down(ppm: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if ppm.len() < 11 || &ppm[..2] != b"P6" {
        return Err("invalid PPM magic".to_string());
    }
    let mut idx = 2usize;
    let mut tokens: [u32; 3] = [0; 3];
    for slot in &mut tokens {
        while idx < ppm.len() {
            if ppm[idx] == b'#' {
                while idx < ppm.len() && ppm[idx] != b'\n' {
                    idx += 1;
                }
            } else if ppm[idx].is_ascii_whitespace() {
                idx += 1;
            } else {
                break;
            }
        }
        let start = idx;
        while idx < ppm.len() && ppm[idx].is_ascii_digit() {
            idx += 1;
        }
        if start == idx {
            return Err("malformed PPM header token".to_string());
        }
        let val = std::str::from_utf8(&ppm[start..idx])
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| "invalid PPM numeric header".to_string())?;
        *slot = val;
    }
    if idx >= ppm.len() || !ppm[idx].is_ascii_whitespace() {
        return Err("missing PPM header delimiter".to_string());
    }
    idx += 1;
    let width = tokens[0];
    let height = tokens[1];
    let maxval = tokens[2];
    if width == 0 || height == 0 || maxval != 255 {
        return Err("unsupported PPM dimensions or maxval".to_string());
    }
    let pixel_count = (width as usize).saturating_mul(height as usize);
    let expected_rgb = pixel_count.saturating_mul(3);
    if ppm.len().saturating_sub(idx) < expected_rgb {
        return Err("PPM pixel payload truncated".to_string());
    }
    let rgb = &ppm[idx..idx + expected_rgb];
    let mut out = Vec::with_capacity(pixel_count * 2);
    for px in rgb.as_chunks::<3>().0 {
        let w =
            (((px[0] as u16) >> 3) << 10) | (((px[1] as u16) >> 3) << 5) | ((px[2] as u16) >> 3);
        out.extend_from_slice(&w.to_le_bytes());
    }
    Ok((width, height, out))
}

pub struct WaylandGrimCapturer {
    shared_size: Arc<Mutex<(u32, u32)>>,
}

impl WaylandGrimCapturer {
    pub fn probe(shared_size: Arc<Mutex<(u32, u32)>>) -> Result<Self, String> {
        if !is_wayland_session() {
            return Err("not a Wayland session".to_string());
        }
        let out = Command::new("grim")
            .args(["-t", "ppm", "-s", "1", "-"])
            .output()
            .map_err(|e| format!("grim launch failed: {e}"))?;
        if !out.status.success() {
            return Err("grim returned non-zero status".to_string());
        }
        let (w, h, _) = parse_ppm_to_rgb555_top_down(&out.stdout)?;
        *shared_size.lock().unwrap() = (w, h);
        Ok(Self { shared_size })
    }
}

pub fn parse_niri_outputs_json(json_text: &str) -> Vec<MonitorRect> {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(json_text) else {
        return Vec::new();
    };
    let Some(obj) = val.as_object() else {
        return Vec::new();
    };
    let mut raw_rects = Vec::new();
    for out in obj.values() {
        let Some(logical) = out.get("logical") else {
            continue;
        };
        let Some(x) = logical.get("x").and_then(|v| v.as_i64()) else {
            continue;
        };
        let Some(y) = logical.get("y").and_then(|v| v.as_i64()) else {
            continue;
        };
        let Some(w) = logical.get("width").and_then(|v| v.as_i64()) else {
            continue;
        };
        let Some(h) = logical.get("height").and_then(|v| v.as_i64()) else {
            continue;
        };
        if w > 0 && h > 0 {
            raw_rects.push((x as i32, y as i32, w as i32, h as i32));
        }
    }
    if raw_rects.is_empty() {
        return Vec::new();
    }
    raw_rects.sort_by_key(|&(x, y, _, _)| (x, y));
    let min_x = raw_rects.iter().map(|r| r.0).min().unwrap_or(0);
    let min_y = raw_rects.iter().map(|r| r.1).min().unwrap_or(0);
    raw_rects
        .into_iter()
        .map(|(x, y, w, h)| MonitorRect {
            left: x - min_x,
            top: y - min_y,
            right: x - min_x + w,
            bottom: y - min_y + h,
        })
        .collect()
}

impl Capturer for WaylandGrimCapturer {
    fn enumerate_displays(&mut self) -> Result<Vec<MonitorRect>, String> {
        let (mut w, mut h) = *self.shared_size.lock().unwrap();
        if w == 0 || h == 0 {
            let out = Command::new("grim")
                .args(["-t", "ppm", "-s", "1", "-"])
                .output()
                .map_err(|e| format!("grim launch failed: {e}"))?;
            if out.status.success() {
                if let Ok((pw, ph, _)) = parse_ppm_to_rgb555_top_down(&out.stdout) {
                    w = pw;
                    h = ph;
                    *self.shared_size.lock().unwrap() = (w, h);
                }
            }
        }
        if let Ok(mut x11) = X11Capturer::connect() {
            if let Ok(mons) = x11.enumerate_displays() {
                let max_r = mons.iter().map(|m| m.right).max().unwrap_or(0) as u32;
                let max_b = mons.iter().map(|m| m.bottom).max().unwrap_or(0) as u32;
                if !mons.is_empty() && max_r == w && max_b == h {
                    return Ok(mons);
                }
            }
        }
        if let Ok(out) = Command::new("niri").args(["msg", "-j", "outputs"]).output() {
            if out.status.success() {
                let mons = parse_niri_outputs_json(&String::from_utf8_lossy(&out.stdout));
                let max_r = mons.iter().map(|m| m.right).max().unwrap_or(0) as u32;
                let max_b = mons.iter().map(|m| m.bottom).max().unwrap_or(0) as u32;
                if !mons.is_empty() && (w == 0 || (max_r == w && max_b == h)) {
                    return Ok(mons);
                }
            }
        }
        if w > 0 && h > 0 {
            Ok(vec![MonitorRect {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: h as i32,
            }])
        } else {
            Err("unable to determine Wayland display dimensions".to_string())
        }
    }

    fn capture_screen(&mut self, rect: MonitorRect) -> Result<Vec<u8>, String> {
        let out = Command::new("grim")
            .args(["-t", "ppm", "-s", "1", "-"])
            .output()
            .map_err(|e| format!("grim capture failed: {e}"))?;
        if !out.status.success() {
            return Err("grim capture exited with error".to_string());
        }
        let (w, h, pixels) = parse_ppm_to_rgb555_top_down(&out.stdout)?;
        *self.shared_size.lock().unwrap() = (w, h);
        Ok(crop_or_resize_rgb555(&pixels, w as usize, h as usize, rect))
    }
}

pub fn crop_or_resize_rgb555(
    pixels: &[u8],
    src_w: usize,
    src_h: usize,
    rect: MonitorRect,
) -> Vec<u8> {
    let target_w = rect.width() as usize;
    let target_h = rect.height() as usize;
    if rect.left == 0 && rect.top == 0 && src_w == target_w && src_h == target_h {
        return pixels.to_vec();
    }
    if rect.left >= 0
        && rect.top >= 0
        && (rect.right as usize) <= src_w
        && (rect.bottom as usize) <= src_h
    {
        let start_x = rect.left as usize;
        let start_y = rect.top as usize;
        let mut cropped = vec![0u8; target_w * target_h * 2];
        let row_bytes = target_w * 2;
        for row in 0..target_h {
            let src_start = ((start_y + row) * src_w + start_x) * 2;
            let dst_start = row * row_bytes;
            cropped[dst_start..dst_start + row_bytes]
                .copy_from_slice(&pixels[src_start..src_start + row_bytes]);
        }
        return cropped;
    }
    let mut resized = vec![0u8; target_w * target_h * 2];
    for y in 0..target_h {
        let sy = (y * src_h / target_h.max(1)).min(src_h.saturating_sub(1));
        for x in 0..target_w {
            let sx = (x * src_w / target_w.max(1)).min(src_w.saturating_sub(1));
            let src_idx = (sy * src_w + sx) * 2;
            let dst_idx = (y * target_w + x) * 2;
            resized[dst_idx] = pixels[src_idx];
            resized[dst_idx + 1] = pixels[src_idx + 1];
        }
    }
    resized
}

pub fn parse_png_to_rgb555_top_down(png: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let img =
        image::load_from_memory(png).map_err(|e| format!("failed to decode capture image: {e}"))?;
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    if w == 0 || h == 0 {
        return Err("empty capture dimensions".to_string());
    }
    let raw = rgb.as_raw();
    let mut out = Vec::with_capacity((w as usize) * (h as usize) * 2);
    for px in raw.as_chunks::<3>().0 {
        let word =
            (((px[0] as u16) >> 3) << 10) | (((px[1] as u16) >> 3) << 5) | ((px[2] as u16) >> 3);
        out.extend_from_slice(&word.to_le_bytes());
    }
    Ok((w, h, out))
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct MacCgPoint {
    x: f64,
    y: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct MacCgSize {
    width: f64,
    height: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct MacCgRect {
    origin: MacCgPoint,
    size: MacCgSize,
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGDisplayPixelsHigh(display: u32) -> usize;
    fn CGGetActiveDisplayList(
        max_displays: u32,
        active_displays: *mut u32,
        display_count: *mut u32,
    ) -> i32;
    fn CGDisplayBounds(display: u32) -> MacCgRect;
    fn CGDisplayCreateImage(display: u32) -> *mut libc::c_void;
    fn CGImageGetWidth(image: *const libc::c_void) -> usize;
    fn CGImageGetHeight(image: *const libc::c_void) -> usize;
    fn CGImageGetBytesPerRow(image: *const libc::c_void) -> usize;
    fn CGImageGetBitsPerPixel(image: *const libc::c_void) -> usize;
    fn CGImageGetBitmapInfo(image: *const libc::c_void) -> u32;
    fn CGImageGetDataProvider(image: *const libc::c_void) -> *const libc::c_void;
    fn CGDataProviderCopyData(provider: *const libc::c_void) -> *const libc::c_void;
    fn CGImageRelease(image: *mut libc::c_void);
    fn CGEventCreateMouseEvent(
        source: *const libc::c_void,
        mouse_type: u32,
        mouse_cursor_position: MacCgPoint,
        mouse_button: u32,
    ) -> *mut libc::c_void;
    fn CGEventCreateScrollWheelEvent2(
        source: *const libc::c_void,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> *mut libc::c_void;
    fn CGEventCreateKeyboardEvent(
        source: *const libc::c_void,
        virtual_key: u16,
        key_down: bool,
    ) -> *mut libc::c_void;
    fn CGEventKeyboardSetUnicodeString(
        event: *mut libc::c_void,
        string_length: libc::c_ulong,
        unicode_string: *const u16,
    );
    fn CGEventPost(tap: u32, event: *mut libc::c_void);
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(the_dict: *const libc::c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: *const libc::c_void;
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: *const libc::c_void;
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    fn CFDictionaryCreate(
        allocator: *const libc::c_void,
        keys: *const *const libc::c_void,
        values: *const *const libc::c_void,
        num_values: isize,
        key_callbacks: *const libc::c_void,
        value_callbacks: *const libc::c_void,
    ) -> *mut libc::c_void;
    fn CFRelease(cf: *const libc::c_void);
    fn CFDataGetBytePtr(the_data: *const libc::c_void) -> *const u8;
    fn CFDataGetLength(the_data: *const libc::c_void) -> isize;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacOsPermissionKind {
    ScreenRecording,
    Accessibility,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacOsPermissionStatus {
    Granted,
    NotGranted,
    NotApplicable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacOsPermissionState {
    pub supported: bool,
    pub screen_recording: MacOsPermissionStatus,
    pub accessibility: MacOsPermissionStatus,
    pub can_stream: bool,
    pub can_control: bool,
}

impl MacOsPermissionState {
    pub fn preflight() -> Self {
        #[cfg(target_os = "macos")]
        {
            let screen_ok = unsafe { CGPreflightScreenCaptureAccess() };
            let ax_ok = unsafe { AXIsProcessTrusted() };
            Self {
                supported: true,
                screen_recording: if screen_ok {
                    MacOsPermissionStatus::Granted
                } else {
                    MacOsPermissionStatus::NotGranted
                },
                accessibility: if ax_ok {
                    MacOsPermissionStatus::Granted
                } else {
                    MacOsPermissionStatus::NotGranted
                },
                can_stream: screen_ok,
                can_control: ax_ok,
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self {
                supported: false,
                screen_recording: MacOsPermissionStatus::NotApplicable,
                accessibility: MacOsPermissionStatus::NotApplicable,
                can_stream: true,
                can_control: true,
            }
        }
    }

    pub fn request(kind: MacOsPermissionKind) -> Self {
        #[cfg(target_os = "macos")]
        {
            let allow_prompt = std::env::var_os("ULTRADESK_SKIP_MACOS_PROMPT").is_none();
            if allow_prompt {
                match kind {
                    MacOsPermissionKind::ScreenRecording => {
                        if unsafe { !CGPreflightScreenCaptureAccess() } {
                            unsafe {
                                let _ = CGRequestScreenCaptureAccess();
                            }
                        }
                    }
                    MacOsPermissionKind::Accessibility => {
                        if unsafe { !AXIsProcessTrusted() } {
                            unsafe {
                                let keys = [kAXTrustedCheckOptionPrompt];
                                let values = [kCFBooleanTrue];
                                let dict = CFDictionaryCreate(
                                    std::ptr::null(),
                                    keys.as_ptr(),
                                    values.as_ptr(),
                                    1,
                                    &kCFTypeDictionaryKeyCallBacks as *const u8
                                        as *const libc::c_void,
                                    &kCFTypeDictionaryValueCallBacks as *const u8
                                        as *const libc::c_void,
                                );
                                if !dict.is_null() {
                                    let _ = AXIsProcessTrustedWithOptions(dict);
                                    CFRelease(dict);
                                }
                            }
                        }
                    }
                }
            }
            Self::preflight()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = kind;
            Self::preflight()
        }
    }

    pub fn open_settings(kind: MacOsPermissionKind) -> Result<&'static str, String> {
        let pane = match kind {
            MacOsPermissionKind::ScreenRecording => "Privacy_ScreenCapture",
            MacOsPermissionKind::Accessibility => "Privacy_Accessibility",
        };
        #[cfg(target_os = "macos")]
        {
            if std::env::var_os("ULTRADESK_SKIP_MACOS_PROMPT").is_none() {
                let url = format!(
                    "x-apple.systempreferences:com.apple.preference.security?{}",
                    pane
                );
                Command::new("/usr/bin/open")
                    .arg(&url)
                    .status()
                    .map_err(|e| format!("failed to open macOS System Settings: {e}"))?;
            }
            Ok(pane)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(pane)
        }
    }
}

pub fn ensure_macos_host_permissions(prompt: bool) -> (bool, bool) {
    if !prompt {
        let st = MacOsPermissionState::preflight();
        (st.can_stream, st.can_control)
    } else {
        let _ = MacOsPermissionState::request(MacOsPermissionKind::ScreenRecording);
        let st = MacOsPermissionState::request(MacOsPermissionKind::Accessibility);
        (st.can_stream, st.can_control)
    }
}

pub fn convert_cg_rgba_rows_to_rgb555(
    src: &[u8],
    width: usize,
    height: usize,
    bytes_per_row: usize,
    bitmap_info: u32,
) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("invalid zero image dimension".to_string());
    }
    if bytes_per_row < width * 4 {
        return Err(format!(
            "bytes_per_row ({bytes_per_row}) is smaller than width * 4 ({})",
            width * 4
        ));
    }
    let required_len = height.saturating_mul(bytes_per_row);
    if src.len() < required_len {
        return Err(format!(
            "source buffer too short ({} < {required_len})",
            src.len()
        ));
    }

    let byte_order = bitmap_info & 0x7000;
    let alpha_info = bitmap_info & 0x1F;
    let is_argb = byte_order == 0x4000 && (alpha_info == 2 || alpha_info == 4 || alpha_info == 6);
    let is_rgba = byte_order == 0x4000 && !is_argb;

    let mut out = vec![0u8; width * height * 2];

    for y in 0..height {
        let row_start = y * bytes_per_row;
        let row = &src[row_start..row_start + width * 4];
        let dst_row = &mut out[y * width * 2..(y + 1) * width * 2];
        for x in 0..width {
            let px = &row[x * 4..(x + 1) * 4];
            let (r, g, b) = if is_argb {
                (px[1], px[2], px[3])
            } else if is_rgba {
                (px[0], px[1], px[2])
            } else {
                (px[2], px[1], px[0])
            };
            let word: u16 =
                (((r as u16) >> 3) << 10) | (((g as u16) >> 3) << 5) | ((b as u16) >> 3);
            dst_row[x * 2] = word as u8;
            dst_row[x * 2 + 1] = (word >> 8) as u8;
        }
    }

    Ok(out)
}

pub fn vk_to_macos_keycode(vk: u8) -> Option<u16> {
    Some(match vk {
        0x08 => 0x33,
        0x09 => 0x30,
        0x0d => 0x24,
        0x10 | 0xa0 => 0x38,
        0xa1 => 0x3c,
        0x11 | 0xa2 => 0x3b,
        0xa3 => 0x3e,
        0x12 | 0xa4 => 0x3a,
        0xa5 => 0x3d,
        0x14 => 0x39,
        0x1b => 0x35,
        0x20 => 0x31,
        0x21 => 0x74,
        0x22 => 0x79,
        0x23 => 0x77,
        0x24 => 0x73,
        0x25 => 0x7b,
        0x26 => 0x7e,
        0x27 => 0x7c,
        0x28 => 0x7d,
        0x2e => 0x75,
        0x30 => 0x1d,
        0x31 => 0x12,
        0x32 => 0x13,
        0x33 => 0x14,
        0x34 => 0x15,
        0x35 => 0x17,
        0x36 => 0x16,
        0x37 => 0x1a,
        0x38 => 0x1c,
        0x39 => 0x19,
        0x41 => 0x00,
        0x42 => 0x0b,
        0x43 => 0x08,
        0x44 => 0x02,
        0x45 => 0x0e,
        0x46 => 0x03,
        0x47 => 0x05,
        0x48 => 0x04,
        0x49 => 0x22,
        0x4a => 0x26,
        0x4b => 0x28,
        0x4c => 0x25,
        0x4d => 0x2e,
        0x4e => 0x2d,
        0x4f => 0x1f,
        0x50 => 0x23,
        0x51 => 0x0c,
        0x52 => 0x0f,
        0x53 => 0x01,
        0x54 => 0x11,
        0x55 => 0x20,
        0x56 => 0x09,
        0x57 => 0x0d,
        0x58 => 0x07,
        0x59 => 0x10,
        0x5a => 0x06,
        0x5b | 0x5c => 0x37,
        0x70 => 0x7a,
        0x71 => 0x78,
        0x72 => 0x63,
        0x73 => 0x76,
        0x74 => 0x60,
        0x75 => 0x61,
        0x76 => 0x62,
        0x77 => 0x64,
        0x78 => 0x65,
        0x79 => 0x6d,
        0x7a => 0x67,
        0x7b => 0x6f,
        0xba => 0x29,
        0xbb => 0x18,
        0xbc => 0x2b,
        0xbd => 0x1b,
        0xbe => 0x2f,
        0xbf => 0x2c,
        0xc0 => 0x32,
        0xdb => 0x21,
        0xdc => 0x2a,
        0xdd => 0x1e,
        0xde => 0x27,
        _ => return None,
    })
}

pub struct MacOsCapturer {
    shared_size: Arc<Mutex<(u32, u32)>>,
}

impl MacOsCapturer {
    pub fn probe(shared_size: Arc<Mutex<(u32, u32)>>) -> Result<Self, String> {
        if !cfg!(target_os = "macos") {
            return Err("not a macOS host".to_string());
        }
        #[cfg(target_os = "macos")]
        {
            let main_id = unsafe { CGMainDisplayID() };
            let w = unsafe { CGDisplayPixelsWide(main_id) } as u32;
            let h = unsafe { CGDisplayPixelsHigh(main_id) } as u32;
            *shared_size.lock().unwrap() = (w.max(1), h.max(1));
        }
        Ok(Self { shared_size })
    }
}

impl Capturer for MacOsCapturer {
    fn enumerate_displays(&mut self) -> Result<Vec<MonitorRect>, String> {
        #[cfg(target_os = "macos")]
        {
            let mut displays = [0u32; 16];
            let mut count = 0u32;
            let res = unsafe { CGGetActiveDisplayList(16, displays.as_mut_ptr(), &mut count) };
            if res == 0 && count > 0 {
                let mut rects = Vec::with_capacity(count as usize);
                for &d in &displays[..count as usize] {
                    let bounds = unsafe { CGDisplayBounds(d) };
                    rects.push(MonitorRect {
                        left: bounds.origin.x as i32,
                        top: bounds.origin.y as i32,
                        right: (bounds.origin.x + bounds.size.width) as i32,
                        bottom: (bounds.origin.y + bounds.size.height) as i32,
                    });
                }
                return Ok(rects);
            }
        }
        let (w, h) = *self.shared_size.lock().unwrap();
        if w > 0 && h > 0 {
            Ok(vec![MonitorRect {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: h as i32,
            }])
        } else {
            Err("unable to determine macOS display dimensions".to_string())
        }
    }

    fn capture_screen(&mut self, rect: MonitorRect) -> Result<Vec<u8>, String> {
        #[cfg(target_os = "macos")]
        {
            if unsafe { !CGPreflightScreenCaptureAccess() } {
                return Err("macOS Screen Recording permission not granted".to_string());
            }
            let main_id = unsafe { CGMainDisplayID() };
            let cg_img = unsafe { CGDisplayCreateImage(main_id) };
            if cg_img.is_null() {
                return Err("CGDisplayCreateImage returned null".to_string());
            }
            let w = unsafe { CGImageGetWidth(cg_img) };
            let h = unsafe { CGImageGetHeight(cg_img) };
            let bpr = unsafe { CGImageGetBytesPerRow(cg_img) };
            let bitmap_info = unsafe { CGImageGetBitmapInfo(cg_img) };
            let provider = unsafe { CGImageGetDataProvider(cg_img) };
            if provider.is_null() {
                unsafe { CGImageRelease(cg_img) };
                return Err("CGImageGetDataProvider returned null".to_string());
            }
            let data_ref = unsafe { CGDataProviderCopyData(provider) };
            if data_ref.is_null() {
                unsafe { CGImageRelease(cg_img) };
                return Err("CGDataProviderCopyData returned null".to_string());
            }
            let ptr = unsafe { CFDataGetBytePtr(data_ref) };
            let len = unsafe { CFDataGetLength(data_ref) } as usize;
            let raw_bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
            let converted = convert_cg_rgba_rows_to_rgb555(raw_bytes, w, h, bpr, bitmap_info);
            unsafe {
                CFRelease(data_ref);
                CGImageRelease(cg_img);
            }
            let pixels = converted?;
            *self.shared_size.lock().unwrap() = (w as u32, h as u32);
            Ok(crop_or_resize_rgb555(&pixels, w, h, rect))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = rect;
            Err("MacOsCapturer is not supported on this platform".to_string())
        }
    }
}

pub struct MacOsCommandInjector {
    #[cfg(test)]
    pub injected_packets: Arc<Mutex<Vec<Vec<u8>>>>,
    pub use_commands: bool,
}

impl MacOsCommandInjector {
    pub fn new(use_commands: bool) -> Self {
        Self {
            #[cfg(test)]
            injected_packets: Arc::new(Mutex::new(Vec::new())),
            use_commands,
        }
    }
}

impl Injector for MacOsCommandInjector {
    fn inject_event(&mut self, packet: &[u8]) -> Result<(), String> {
        #[cfg(test)]
        self.injected_packets.lock().unwrap().push(packet.to_vec());
        let _ = packet;
        if !self.use_commands {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        if let Some(evt) = decode_host_input(packet) {
            unsafe {
                match evt {
                    HostInputEvent::Mouse {
                        msg, x, y, wheel, ..
                    } => {
                        let pt = MacCgPoint {
                            x: x.max(0) as f64,
                            y: y.max(0) as f64,
                        };
                        match msg {
                            0x0200 => {
                                let ev = CGEventCreateMouseEvent(std::ptr::null(), 5, pt, 0);
                                if !ev.is_null() {
                                    CGEventPost(0, ev);
                                    CFRelease(ev);
                                }
                            }
                            0x0201 | 0x0203 => {
                                let ev = CGEventCreateMouseEvent(std::ptr::null(), 1, pt, 0);
                                if !ev.is_null() {
                                    CGEventPost(0, ev);
                                    CFRelease(ev);
                                }
                            }
                            0x0202 => {
                                let ev = CGEventCreateMouseEvent(std::ptr::null(), 2, pt, 0);
                                if !ev.is_null() {
                                    CGEventPost(0, ev);
                                    CFRelease(ev);
                                }
                            }
                            0x0204 | 0x0206 => {
                                let ev = CGEventCreateMouseEvent(std::ptr::null(), 3, pt, 1);
                                if !ev.is_null() {
                                    CGEventPost(0, ev);
                                    CFRelease(ev);
                                }
                            }
                            0x0205 => {
                                let ev = CGEventCreateMouseEvent(std::ptr::null(), 4, pt, 1);
                                if !ev.is_null() {
                                    CGEventPost(0, ev);
                                    CFRelease(ev);
                                }
                            }
                            0x0207 | 0x0209 => {
                                let ev = CGEventCreateMouseEvent(std::ptr::null(), 25, pt, 2);
                                if !ev.is_null() {
                                    CGEventPost(0, ev);
                                    CFRelease(ev);
                                }
                            }
                            0x0208 => {
                                let ev = CGEventCreateMouseEvent(std::ptr::null(), 26, pt, 2);
                                if !ev.is_null() {
                                    CGEventPost(0, ev);
                                    CFRelease(ev);
                                }
                            }
                            0x020A => {
                                let steps = (wheel as i32 / 120).clamp(-10, 10);
                                if steps != 0 {
                                    let ev = CGEventCreateScrollWheelEvent2(
                                        std::ptr::null(),
                                        1,
                                        1,
                                        steps,
                                        0,
                                        0,
                                    );
                                    if !ev.is_null() {
                                        CGEventPost(0, ev);
                                        CFRelease(ev);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    HostInputEvent::Key { kind, vk, ch, .. } => {
                        let down = kind == KEY_DOWN || kind == SYS_KEY_DOWN;
                        if vk == VK_PACKET {
                            if down && ch >= 0x20 {
                                let down_ev = CGEventCreateKeyboardEvent(std::ptr::null(), 0, true);
                                if !down_ev.is_null() {
                                    let unit = [ch];
                                    CGEventKeyboardSetUnicodeString(down_ev, 1, unit.as_ptr());
                                    CGEventPost(0, down_ev);
                                    CFRelease(down_ev);
                                }
                                let up_ev = CGEventCreateKeyboardEvent(std::ptr::null(), 0, false);
                                if !up_ev.is_null() {
                                    let unit = [ch];
                                    CGEventKeyboardSetUnicodeString(up_ev, 1, unit.as_ptr());
                                    CGEventPost(0, up_ev);
                                    CFRelease(up_ev);
                                }
                            }
                        } else if let Some(kc) = vk_to_macos_keycode(vk) {
                            let ev = CGEventCreateKeyboardEvent(std::ptr::null(), kc, down);
                            if !ev.is_null() {
                                CGEventPost(0, ev);
                                CFRelease(ev);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn release_all(&mut self) {
        #[cfg(test)]
        self.injected_packets
            .lock()
            .unwrap()
            .push(b"RELEASE_ALL".to_vec());
    }
}

pub fn vk_to_evdev(vk: u8) -> Option<u16> {
    Some(match vk {
        0x08 => 14,
        0x09 => 15,
        0x0d => 28,
        0x10 | 0xa0 => 42,
        0xa1 => 54,
        0x11 | 0xa2 => 29,
        0xa3 => 97,
        0x12 | 0xa4 => 56,
        0xa5 => 100,
        0x14 => 58,
        0x1b => 1,
        0x20 => 57,
        0x21 => 104,
        0x22 => 109,
        0x23 => 107,
        0x24 => 102,
        0x25 => 105,
        0x26 => 103,
        0x27 => 106,
        0x28 => 108,
        0x2d => 110,
        0x2e => 111,
        0x30 => 11,
        0x31..=0x39 => (vk - 0x31 + 2) as u16,
        0x41 => 30,
        0x42 => 48,
        0x43 => 46,
        0x44 => 32,
        0x45 => 18,
        0x46 => 33,
        0x47 => 34,
        0x48 => 35,
        0x49 => 23,
        0x4a => 36,
        0x4b => 37,
        0x4c => 38,
        0x4d => 50,
        0x4e => 49,
        0x4f => 24,
        0x50 => 25,
        0x51 => 16,
        0x52 => 19,
        0x53 => 31,
        0x54 => 20,
        0x55 => 22,
        0x56 => 47,
        0x57 => 17,
        0x58 => 45,
        0x59 => 21,
        0x5a => 44,
        0x5b => 125,
        0x5c => 126,
        0x70..=0x79 => (vk - 0x70 + 59) as u16,
        0x7a => 87,
        0x7b => 88,
        0xba => 39,
        0xbb => 13,
        0xbc => 51,
        0xbd => 12,
        0xbe => 52,
        0xbf => 53,
        0xc0 => 41,
        0xdb => 26,
        0xdc => 43,
        0xdd => 27,
        0xde => 40,
        _ => return None,
    })
}

pub fn ascii_to_evdev(ch: u16) -> Option<(u16, bool)> {
    let b = u8::try_from(ch).ok()?;
    Some(match b {
        b'a'..=b'z' => (vk_to_evdev(b - 32)?, false),
        b'A'..=b'Z' => (vk_to_evdev(b)?, true),
        b'0'..=b'9' => (vk_to_evdev(b)?, false),
        b'!' => (2, true),
        b'@' => (3, true),
        b'#' => (4, true),
        b'$' => (5, true),
        b'%' => (6, true),
        b'^' => (7, true),
        b'&' => (8, true),
        b'*' => (9, true),
        b'(' => (10, true),
        b')' => (11, true),
        b'-' => (12, false),
        b'_' => (12, true),
        b'=' => (13, false),
        b'+' => (13, true),
        b'[' => (26, false),
        b'{' => (26, true),
        b']' => (27, false),
        b'}' => (27, true),
        b'\\' => (43, false),
        b'|' => (43, true),
        b';' => (39, false),
        b':' => (39, true),
        b'\'' => (40, false),
        b'"' => (40, true),
        b'`' => (41, false),
        b'~' => (41, true),
        b',' => (51, false),
        b'<' => (51, true),
        b'.' => (52, false),
        b'>' => (52, true),
        b'/' => (53, false),
        b'?' => (53, true),
        b' ' => (57, false),
        b'\n' | b'\r' => (28, false),
        b'\t' => (15, false),
        _ => return None,
    })
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct UinputInputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct UinputSetup {
    id: UinputInputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct UinputAbsinfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct UinputAbsSetup {
    code: u16,
    absinfo: UinputAbsinfo,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct RawLinuxInputEvent {
    tv_sec: libc::c_long,
    tv_usec: libc::c_long,
    type_: u16,
    code: u16,
    value: i32,
}

#[cfg(target_os = "linux")]
const UI_SET_EVBIT: libc::Ioctl = 0x4004_5564 as libc::Ioctl;
#[cfg(target_os = "linux")]
const UI_SET_KEYBIT: libc::Ioctl = 0x4004_5565 as libc::Ioctl;
#[cfg(target_os = "linux")]
const UI_SET_RELBIT: libc::Ioctl = 0x4004_5566 as libc::Ioctl;
#[cfg(target_os = "linux")]
const UI_SET_ABSBIT: libc::Ioctl = 0x4004_5567 as libc::Ioctl;
#[cfg(target_os = "linux")]
const UI_DEV_SETUP: libc::Ioctl = 0x405c_5503 as libc::Ioctl;
#[cfg(target_os = "linux")]
const UI_ABS_SETUP: libc::Ioctl = 0x401c_5504 as libc::Ioctl;
#[cfg(target_os = "linux")]
const UI_DEV_CREATE: libc::Ioctl = 0x5501 as libc::Ioctl;
#[cfg(target_os = "linux")]
const UI_DEV_DESTROY: libc::Ioctl = 0x5502 as libc::Ioctl;
#[cfg(target_os = "linux")]
const UINPUT_ABS_MAX: i32 = 65535;

#[cfg(target_os = "linux")]
pub struct UinputInjector {
    pointer_fd: i32,
    keyboard_fd: i32,
    shared_size: Arc<Mutex<(u32, u32)>>,
    held_keys: Vec<u16>,
    held_buttons: Vec<u16>,
    wheel_remainder: i32,
}

#[cfg(target_os = "linux")]
impl UinputInjector {
    pub fn connect(shared_size: Arc<Mutex<(u32, u32)>>) -> Result<Self, String> {
        unsafe {
            let pointer_fd = libc::open(c"/dev/uinput".as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK);
            if pointer_fd < 0 {
                return Err("cannot open /dev/uinput for pointer".to_string());
            }
            let keyboard_fd =
                libc::open(c"/dev/uinput".as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK);
            if keyboard_fd < 0 {
                libc::close(pointer_fd);
                return Err("cannot open /dev/uinput for keyboard".to_string());
            }

            for ev in [0x00i32, 0x01, 0x02, 0x03] {
                libc::ioctl(pointer_fd, UI_SET_EVBIT, ev);
            }
            for btn in [0x110i32, 0x111, 0x112] {
                libc::ioctl(pointer_fd, UI_SET_KEYBIT, btn);
            }
            libc::ioctl(pointer_fd, UI_SET_RELBIT, 0x08i32);
            libc::ioctl(pointer_fd, UI_SET_ABSBIT, 0x00i32);
            libc::ioctl(pointer_fd, UI_SET_ABSBIT, 0x01i32);

            for axis in [0u16, 1u16] {
                let abs_setup = UinputAbsSetup {
                    code: axis,
                    absinfo: UinputAbsinfo {
                        value: 0,
                        minimum: 0,
                        maximum: UINPUT_ABS_MAX,
                        fuzz: 0,
                        flat: 0,
                        resolution: 0,
                    },
                };
                libc::ioctl(
                    pointer_fd,
                    UI_ABS_SETUP,
                    &abs_setup as *const UinputAbsSetup,
                );
            }

            let mut p_name = [0u8; 80];
            let p_label = b"UltraDesk Virtual Pointer";
            p_name[..p_label.len()].copy_from_slice(p_label);
            let p_setup = UinputSetup {
                id: UinputInputId {
                    bustype: 0x03,
                    vendor: 0x1d6b,
                    product: 0x0104,
                    version: 1,
                },
                name: p_name,
                ff_effects_max: 0,
            };
            if libc::ioctl(pointer_fd, UI_DEV_SETUP, &p_setup as *const UinputSetup) < 0
                || libc::ioctl(pointer_fd, UI_DEV_CREATE) < 0
            {
                libc::close(pointer_fd);
                libc::close(keyboard_fd);
                return Err("failed to create uinput pointer device".to_string());
            }

            for ev in [0x00i32, 0x01] {
                libc::ioctl(keyboard_fd, UI_SET_EVBIT, ev);
            }
            for k in 1i32..249 {
                libc::ioctl(keyboard_fd, UI_SET_KEYBIT, k);
            }
            let mut k_name = [0u8; 80];
            let k_label = b"UltraDesk Virtual Keyboard";
            k_name[..k_label.len()].copy_from_slice(k_label);
            let k_setup = UinputSetup {
                id: UinputInputId {
                    bustype: 0x03,
                    vendor: 0x1d6b,
                    product: 0x0105,
                    version: 1,
                },
                name: k_name,
                ff_effects_max: 0,
            };
            if libc::ioctl(keyboard_fd, UI_DEV_SETUP, &k_setup as *const UinputSetup) < 0
                || libc::ioctl(keyboard_fd, UI_DEV_CREATE) < 0
            {
                libc::ioctl(pointer_fd, UI_DEV_DESTROY);
                libc::close(pointer_fd);
                libc::close(keyboard_fd);
                return Err("failed to create uinput keyboard device".to_string());
            }

            Ok(Self {
                pointer_fd,
                keyboard_fd,
                shared_size,
                held_keys: Vec::new(),
                held_buttons: Vec::new(),
                wheel_remainder: 0,
            })
        }
    }

    fn emit(fd: i32, type_: u16, code: u16, value: i32) {
        let ev = RawLinuxInputEvent {
            tv_sec: 0,
            tv_usec: 0,
            type_,
            code,
            value,
        };
        unsafe {
            let _ = libc::write(
                fd,
                &ev as *const RawLinuxInputEvent as *const libc::c_void,
                std::mem::size_of::<RawLinuxInputEvent>(),
            );
        }
    }

    fn syn(fd: i32) {
        Self::emit(fd, 0x00, 0x00, 0);
    }

    fn set_button(&mut self, btn_code: u16, pressed: bool) {
        if pressed {
            if !self.held_buttons.contains(&btn_code) {
                self.held_buttons.push(btn_code);
            }
        } else if let Some(idx) = self.held_buttons.iter().position(|&b| b == btn_code) {
            self.held_buttons.remove(idx);
        } else {
            return;
        }
        Self::emit(self.pointer_fd, 0x01, btn_code, if pressed { 1 } else { 0 });
        Self::syn(self.pointer_fd);
    }

    fn set_key(&mut self, keycode: u16, pressed: bool) {
        if pressed {
            if !self.held_keys.contains(&keycode) {
                self.held_keys.push(keycode);
            }
        } else if let Some(idx) = self.held_keys.iter().position(|&k| k == keycode) {
            self.held_keys.remove(idx);
        } else {
            return;
        }
        Self::emit(self.keyboard_fd, 0x01, keycode, if pressed { 1 } else { 0 });
        Self::syn(self.keyboard_fd);
    }
}

#[cfg(target_os = "linux")]
impl Drop for UinputInjector {
    fn drop(&mut self) {
        self.release_all();
        unsafe {
            libc::ioctl(self.pointer_fd, UI_DEV_DESTROY);
            libc::close(self.pointer_fd);
            libc::ioctl(self.keyboard_fd, UI_DEV_DESTROY);
            libc::close(self.keyboard_fd);
        }
    }
}

#[cfg(target_os = "linux")]
impl Injector for UinputInjector {
    fn inject_event(&mut self, packet: &[u8]) -> Result<(), String> {
        let Some(event) = decode_host_input(packet) else {
            return Ok(());
        };
        match pointer_action(&event, &mut self.wheel_remainder) {
            PointerAction::Move(x, y) => {
                let (w, h) = {
                    let g = *self.shared_size.lock().unwrap();
                    (g.0.max(1) as i64, g.1.max(1) as i64)
                };
                let cx = (x as i64).clamp(0, (w - 1).max(0));
                let cy = (y as i64).clamp(0, (h - 1).max(0));
                let abs_x = ((cx * UINPUT_ABS_MAX as i64) / (w - 1).max(1)) as i32;
                let abs_y = ((cy * UINPUT_ABS_MAX as i64) / (h - 1).max(1)) as i32;
                Self::emit(self.pointer_fd, 0x03, 0x00, abs_x);
                Self::emit(self.pointer_fd, 0x03, 0x01, abs_y);
                Self::syn(self.pointer_fd);
            }
            PointerAction::Button(b, pressed) => {
                let code = match b {
                    1 => 0x110,
                    2 => 0x112,
                    _ => 0x111,
                };
                self.set_button(code, pressed);
            }
            PointerAction::Wheel(steps) => {
                Self::emit(self.pointer_fd, 0x02, 0x08, steps);
                Self::syn(self.pointer_fd);
            }
            PointerAction::None => {}
        }
        if let HostInputEvent::Key {
            kind, vk, scan, ch, ..
        } = event
        {
            let down = kind == KEY_DOWN || kind == SYS_KEY_DOWN;
            if vk == VK_PACKET {
                if down {
                    if let Some((code, shift)) = ascii_to_evdev(ch) {
                        if shift {
                            Self::emit(self.keyboard_fd, 0x01, 42, 1);
                            Self::syn(self.keyboard_fd);
                        }
                        Self::emit(self.keyboard_fd, 0x01, code, 1);
                        Self::syn(self.keyboard_fd);
                        Self::emit(self.keyboard_fd, 0x01, code, 0);
                        Self::syn(self.keyboard_fd);
                        if shift {
                            Self::emit(self.keyboard_fd, 0x01, 42, 0);
                            Self::syn(self.keyboard_fd);
                        }
                    }
                }
            } else if let Some(code) = evdev_keycode(scan)
                .map(|c| c as u16)
                .or_else(|| vk_to_evdev(vk))
            {
                if down && self.held_keys.contains(&code) {
                    Self::emit(self.keyboard_fd, 0x01, code, 2);
                    Self::syn(self.keyboard_fd);
                } else {
                    self.set_key(code, down);
                }
            }
        }
        Ok(())
    }

    fn release_all(&mut self) {
        for code in std::mem::take(&mut self.held_keys).into_iter().rev() {
            Self::emit(self.keyboard_fd, 0x01, code, 0);
        }
        Self::syn(self.keyboard_fd);
        for btn in std::mem::take(&mut self.held_buttons) {
            Self::emit(self.pointer_fd, 0x01, btn, 0);
        }
        Self::syn(self.pointer_fd);
        self.wheel_remainder = 0;
    }
}

#[cfg(not(target_os = "linux"))]
pub struct UinputInjector;

#[cfg(not(target_os = "linux"))]
impl UinputInjector {
    pub fn connect(_shared_size: Arc<Mutex<(u32, u32)>>) -> Result<Self, String> {
        Err("uinput is only supported on Linux".to_string())
    }
}

#[cfg(not(target_os = "linux"))]
impl Injector for UinputInjector {
    fn inject_event(&mut self, _packet: &[u8]) -> Result<(), String> {
        Ok(())
    }

    fn release_all(&mut self) {}
}

pub struct DesktopBackend {
    pub name: &'static str,
    pub capturer: Box<dyn Capturer>,
    pub injector: Box<dyn Injector>,
}

impl DesktopBackend {
    pub fn headless(width: u32, height: u32) -> Self {
        Self {
            name: "HeadlessBackend",
            capturer: Box::new(HeadlessCapturer::new(width, height)),
            injector: Box::new(HeadlessInjector::new()),
        }
    }

    pub fn auto_detect() -> Self {
        let shared_size = Arc::new(Mutex::new((1920u32, 1080u32)));
        if cfg!(target_os = "macos") {
            if let Ok(capturer) = MacOsCapturer::probe(shared_size.clone()) {
                return Self {
                    name: "MacOsCoreGraphicsBackend",
                    capturer: Box::new(capturer),
                    injector: Box::new(MacOsCommandInjector::new(true)),
                };
            }
        }
        if is_wayland_session() {
            if let Ok(capturer) = WaylandGrimCapturer::probe(shared_size.clone()) {
                if let Ok(injector) = UinputInjector::connect(shared_size.clone()) {
                    return Self {
                        name: "WaylandGrimUinputBackend",
                        capturer: Box::new(capturer),
                        injector: Box::new(injector),
                    };
                }
                if let Ok(injector) = X11Injector::connect() {
                    return Self {
                        name: "WaylandGrimXtestBackend",
                        capturer: Box::new(capturer),
                        injector: Box::new(injector),
                    };
                }
                return Self {
                    name: "WaylandGrimCommandBackend",
                    capturer: Box::new(capturer),
                    injector: Box::new(X11CommandInjector::new(true)),
                };
            }
        }
        if let Ok(mut capturer) = X11Capturer::connect() {
            if let Ok(mons) = capturer.enumerate_displays() {
                let w = mons.iter().map(|m| m.right).max().unwrap_or(1920) as u32;
                let h = mons.iter().map(|m| m.bottom).max().unwrap_or(1080) as u32;
                *shared_size.lock().unwrap() = (w.max(1), h.max(1));
            }
            if let Ok(injector) = X11Injector::connect() {
                let name = if capturer.shm_supported {
                    "X11ShmXtestBackend"
                } else {
                    "X11GetImageXtestBackend"
                };
                return Self {
                    name,
                    capturer: Box::new(capturer),
                    injector: Box::new(injector),
                };
            }
            if let Ok(injector) = UinputInjector::connect(shared_size) {
                return Self {
                    name: "X11UinputBackend",
                    capturer: Box::new(capturer),
                    injector: Box::new(injector),
                };
            }
        }
        Self::headless(1280, 720)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostStatus {
    pub running: bool,
    pub host_id: String,
    pub temp_password: String,
    pub hardware_mac: String,
    pub has_master_password: bool,
    pub auto_approve: bool,
    pub phase: HostPhase,
    pub viewer_id: Option<String>,
    pub viewer_name: Option<String>,
    pub control_allowed: bool,
    pub waiting_ack: bool,
    pub frame_seq: i32,
    pub is_locked: bool,
    pub backend_name: String,
    #[serde(default)]
    pub monitors: Vec<(i32, i32, i32, i32)>,
    #[serde(default)]
    pub selected_monitor: Option<usize>,
    #[serde(default)]
    pub selected_monitors: Vec<usize>,
    #[serde(default)]
    pub lan_port: u16,
    #[serde(default)]
    pub e2e_enabled: bool,
    #[serde(default)]
    pub e2e_cipher: String,
    #[serde(default)]
    pub e2e_fingerprint: String,
    #[serde(default = "MacOsPermissionState::preflight")]
    pub macos_permissions: MacOsPermissionState,
}

struct HostSessionInner {
    running: bool,
    generation: u64,
    lan_port: u16,
    lan_viewer_active: bool,
    device_id: String,
    display_name: String,
    host_id: String,
    hardware_mac: String,
    core: HostCore,
    backend: DesktopBackend,
    full_monitors: Vec<MonitorRect>,
    monitor_selection: MonitorSelection,
    outbound_peer_packets: Vec<Vec<u8>>,
    pending_chats: Vec<String>,
    pending_clipboard: Option<String>,
    capture_needed: bool,
    force_keyframe: bool,
    last_frame: Vec<u8>,
    peer_throttles: std::collections::HashMap<String, LoginThrottle>,
}

pub fn query_live_host_id(
    device_id: &str,
    hardware_mac: &str,
    temp_password: &str,
    master_password_proof: Option<&str>,
    display_name: &str,
) -> Result<String, String> {
    query_live_host_id_at(
        DEFAULT_REGISTRATION_HOST,
        443,
        true,
        device_id,
        hardware_mac,
        temp_password,
        master_password_proof,
        display_name,
    )
}

pub fn query_live_host_id_at(
    reg_host: &str,
    reg_port: u16,
    use_tls: bool,
    device_id: &str,
    hardware_mac: &str,
    temp_password: &str,
    master_password_proof: Option<&str>,
    display_name: &str,
) -> Result<String, String> {
    let local_ip = detect_local_ip();
    let computer_b64 = encode_base64(display_name.as_bytes());
    let user_raw = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "linux".to_string());
    let user_clean: String = user_raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(24)
        .collect();
    let user_name = if user_clean.is_empty() {
        "linux"
    } else {
        &user_clean
    };
    let candidates = candidate_registration_macs(hardware_mac);
    for active_mac in &candidates {
        let cmd = build_host_registration_command(
            device_id,
            temp_password,
            &local_ip,
            &computer_b64,
            active_mac,
            master_password_proof,
            user_name,
        )?;
        let mut reg = RegistrationTransport::new(5.0, 1024 * 1024, true)?;
        reg.connect_with_tls(reg_host, reg_port, use_tls)?;
        reg.send_command(&cmd)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut try_next_mac = false;
        while Instant::now() < deadline {
            match reg.receive_mode1_payload() {
                Ok(payload) if payload.starts_with(b"3:") => {
                    let reply = parse_host_registration_reply(&payload)?;
                    reg.close();
                    return Ok(reply.id);
                }
                Ok(payload) if payload.starts_with(b"60:") => {
                    try_next_mac = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        reg.close();
        if !try_next_mac {
            break;
        }
    }
    Err("registration server did not return command 3 reply".to_string())
}

#[derive(Clone)]
pub struct HostSession {
    inner: Arc<Mutex<HostSessionInner>>,
}

impl HostSession {
    pub fn new(
        temp_password: &str,
        master_password_proof: Option<String>,
        layout: DisplayLayout,
        backend: DesktopBackend,
    ) -> Self {
        Self::with_id(
            "",
            "",
            temp_password,
            master_password_proof,
            layout,
            backend,
        )
    }

    pub fn with_id(
        host_id: &str,
        hardware_mac: &str,
        temp_password: &str,
        master_password_proof: Option<String>,
        layout: DisplayLayout,
        backend: DesktopBackend,
    ) -> Self {
        Self::with_profile(
            "FEDCBA9876543210FEDCBA9876543210",
            host_id,
            hardware_mac,
            temp_password,
            master_password_proof,
            "UltraDesk",
            layout,
            backend,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_profile(
        device_id: &str,
        host_id: &str,
        hardware_mac: &str,
        temp_password: &str,
        master_password_proof: Option<String>,
        display_name: &str,
        layout: DisplayLayout,
        backend: DesktopBackend,
    ) -> Self {
        let initial_monitors = layout.monitors.clone();
        let core = HostCore::new(temp_password, master_password_proof, layout);
        let mut inner = HostSessionInner {
            running: false,
            generation: 0,
            lan_port: 0,
            lan_viewer_active: false,
            device_id: if device_id.len() == 32 {
                device_id.to_string()
            } else {
                "FEDCBA9876543210FEDCBA9876543210".to_string()
            },
            display_name: if display_name.is_empty() {
                "UltraDesk".to_string()
            } else {
                display_name.to_string()
            },
            host_id: host_id.to_string(),
            hardware_mac: if hardware_mac.len() == 12 {
                hardware_mac.to_string()
            } else {
                "001122334455".to_string()
            },
            core,
            backend,
            full_monitors: initial_monitors,
            monitor_selection: MonitorSelection::All,
            outbound_peer_packets: Vec::new(),
            pending_chats: Vec::new(),
            pending_clipboard: None,
            capture_needed: false,
            force_keyframe: true,
            last_frame: Vec::new(),
            peer_throttles: std::collections::HashMap::new(),
        };
        Self::refresh_display_layout(&mut inner);
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    fn compute_shared_layout(
        full_monitors: &[MonitorRect],
        selection: &MonitorSelection,
        fallback: &DisplayLayout,
    ) -> DisplayLayout {
        if full_monitors.is_empty() {
            return fallback.clone();
        }
        let min_left = full_monitors.iter().map(|m| m.left).min().unwrap_or(0);
        let min_top = full_monitors.iter().map(|m| m.top).min().unwrap_or(0);
        let max_right = full_monitors
            .iter()
            .map(|m| m.right)
            .max()
            .unwrap_or(fallback.width as i32);
        let max_bottom = full_monitors
            .iter()
            .map(|m| m.bottom)
            .max()
            .unwrap_or(fallback.height as i32);
        let full_layout = DisplayLayout {
            width: (max_right - min_left).max(1) as u32,
            height: (max_bottom - min_top).max(1) as u32,
            virtual_left: min_left,
            virtual_top: min_top,
            monitors: full_monitors.to_vec(),
        };
        let indices = full_layout.selected_indices(selection);
        let selected_rects: Vec<MonitorRect> = indices
            .iter()
            .filter_map(|&idx| full_monitors.get(idx).copied())
            .collect();
        let bounds = full_layout.active_bounds(selection);
        DisplayLayout {
            width: bounds.width().max(1),
            height: bounds.height().max(1),
            virtual_left: bounds.left,
            virtual_top: bounds.top,
            monitors: if selected_rects.is_empty() {
                full_monitors.to_vec()
            } else {
                selected_rects
            },
        }
    }

    fn refresh_display_layout(s: &mut HostSessionInner) {
        if let Ok(monitors) = s.backend.capturer.enumerate_displays() {
            if !monitors.is_empty()
                && (s.backend.name != "HeadlessBackend" || s.full_monitors.is_empty())
            {
                s.full_monitors = monitors;
            }
        }
        if s.full_monitors.is_empty() {
            s.full_monitors = s.core.layout.monitors.clone();
        }
        let new_layout =
            Self::compute_shared_layout(&s.full_monitors, &s.monitor_selection, &s.core.layout);
        if matches!(s.core.phase, HostPhase::Starting | HostPhase::Streaming) {
            s.force_keyframe = true;
            let actions = s.core.update_layout(new_layout);
            Self::record_actions(s, &actions);
        } else {
            s.core.layout = new_layout;
        }
    }

    fn record_actions(s: &mut HostSessionInner, actions: &[HostAction]) {
        let ox = s.core.layout.virtual_left;
        let oy = s.core.layout.virtual_top;
        for act in actions {
            match act {
                HostAction::SendPeer(bytes) => {
                    s.outbound_peer_packets.push(bytes.clone());
                }
                HostAction::SessionStarted(_) => {
                    s.force_keyframe = true;
                }
                HostAction::RequestCapture => {
                    s.capture_needed = true;
                }
                HostAction::InjectInput(pkt) => {
                    let translated = offset_host_input_packet(pkt, ox, oy);
                    let _ = s.backend.injector.inject_event(&translated);
                }
                HostAction::ReleaseAllInput => {
                    s.backend.injector.release_all();
                }
                HostAction::ChatReceived(text) => {
                    s.pending_chats.push(format!("Partner: {}", text));
                }
                HostAction::ClipboardReceived(text) => {
                    let _ = crate::clipboard::set_linux_system_clipboard(text);
                    s.pending_clipboard = Some(text.clone());
                }
                _ => {}
            }
        }
    }

    pub fn send_chat(&self, text: &str) -> bool {
        let mut s = self.inner.lock().unwrap();
        if matches!(s.core.phase, HostPhase::Starting | HostPhase::Streaming) {
            let pkt = s.core.seal_peer_packet(&encode_host_chat(text));
            s.outbound_peer_packets.push(pkt);
            true
        } else {
            false
        }
    }

    pub fn send_clipboard(&self, text: &str) -> bool {
        let mut s = self.inner.lock().unwrap();
        if matches!(s.core.phase, HostPhase::Starting | HostPhase::Streaming) {
            let pkt = s.core.seal_peer_packet(&encode_host_clipboard(text));
            s.outbound_peer_packets.push(pkt);
            true
        } else {
            false
        }
    }

    pub fn set_download_dir(&self, download_dir: std::path::PathBuf) {
        let mut s = self.inner.lock().unwrap();
        s.core.files.download_dir = download_dir;
    }

    pub fn file_transfers(&self) -> Vec<crate::file_transfer::FileTransferEntry> {
        let s = self.inner.lock().unwrap();
        s.core.files.transfers.values().cloned().collect()
    }

    pub fn downloaded_file_bytes(&self, file_id: &str) -> Option<(String, Vec<u8>)> {
        let s = self.inner.lock().unwrap();
        if let Some(res) = s.core.files.read_completed_download(file_id) {
            return Some(res);
        }
        let entry = s.core.files.transfers.get(file_id)?;
        if entry.direction == crate::file_transfer::TransferDirection::Send
            && entry.saved_path.is_none()
            && !entry.buffer.is_empty()
        {
            return Some((entry.file_name.clone(), entry.buffer.clone()));
        }
        None
    }

    pub fn offer_file_bytes(&self, file_name: &str, data: Vec<u8>) -> Result<String, String> {
        let mut s = self.inner.lock().unwrap();
        if !matches!(s.core.phase, HostPhase::Starting | HostPhase::Streaming) {
            return Err("No active viewer connected to host".to_string());
        }
        let (file_id, pkt) = s.core.files.offer_file_bytes(file_name, data)?;
        let sealed = s.core.seal_peer_packet(&pkt);
        s.outbound_peer_packets.push(sealed);
        Ok(file_id)
    }

    pub fn offer_file_from_path(&self, path: &std::path::Path) -> Result<String, String> {
        let mut s = self.inner.lock().unwrap();
        if !matches!(s.core.phase, HostPhase::Starting | HostPhase::Streaming) {
            return Err("No active viewer connected to host".to_string());
        }
        let (file_id, pkt) = s.core.files.offer_file_from_path(path)?;
        let sealed = s.core.seal_peer_packet(&pkt);
        s.outbound_peer_packets.push(sealed);
        Ok(file_id)
    }

    pub fn accept_file(&self, file_id: &str) -> Result<Vec<u8>, String> {
        let mut s = self.inner.lock().unwrap();
        let pkt = s.core.files.accept_incoming_offer(file_id)?;
        if matches!(s.core.phase, HostPhase::Starting | HostPhase::Streaming) {
            let sealed = s.core.seal_peer_packet(&pkt);
            s.outbound_peer_packets.push(sealed);
        }
        Ok(pkt)
    }

    pub fn cancel_file(&self, file_id: &str) -> Result<Vec<u8>, String> {
        let mut s = self.inner.lock().unwrap();
        let pkt = s.core.files.cancel_transfer(file_id)?;
        if matches!(s.core.phase, HostPhase::Starting | HostPhase::Streaming) {
            let sealed = s.core.seal_peer_packet(&pkt);
            s.outbound_peer_packets.push(sealed);
        }
        Ok(pkt)
    }

    pub fn drain_auxiliary_events(&self) -> (Vec<String>, Option<String>) {
        let mut s = self.inner.lock().unwrap();
        let chats = std::mem::take(&mut s.pending_chats);
        let clip = s.pending_clipboard.take();
        (chats, clip)
    }

    pub fn start(&self) -> HostStatus {
        self.start_with_endpoint(DEFAULT_REGISTRATION_HOST, 443, true)
    }

    pub fn start_with_endpoint(&self, reg_host: &str, reg_port: u16, use_tls: bool) -> HostStatus {
        let (gen, lan_listener) = {
            let mut s = self.inner.lock().unwrap();
            if s.running && !s.host_id.is_empty() && s.host_id.bytes().all(|b| b.is_ascii_digit()) {
                drop(s);
                return self.status();
            }
            let listener = if use_tls {
                TcpListener::bind(("0.0.0.0", DEFAULT_LAN_PORT))
                    .or_else(|_| TcpListener::bind(("0.0.0.0", 0)))
                    .ok()
            } else {
                TcpListener::bind(("0.0.0.0", 0)).ok()
            };
            if let Some(ref l) = listener {
                let _ = l.set_nonblocking(true);
            }
            let lan_port = listener
                .as_ref()
                .and_then(|l| l.local_addr().ok())
                .map(|a| a.port())
                .unwrap_or(0);
            s.running = true;
            s.generation = s.generation.wrapping_add(1);
            s.lan_port = lan_port;
            s.lan_viewer_active = false;
            s.outbound_peer_packets.clear();
            s.capture_needed = false;
            s.last_frame.clear();
            Self::refresh_display_layout(&mut s);
            (s.generation, listener)
        };
        if let Some(listener) = lan_listener {
            let lan_clone = self.clone();
            thread::spawn(move || {
                lan_clone.run_lan_worker(gen, listener);
            });
        }
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let host_clone = self.clone();
        let host_str = reg_host.to_string();
        thread::spawn(move || {
            host_clone.run_host_worker(gen, &host_str, reg_port, use_tls, Some(ready_tx));
        });
        let _ = ready_rx.recv_timeout(Duration::from_secs(10));
        self.status()
    }

    fn is_worker_active(&self, gen: u64) -> bool {
        let s = self.inner.lock().unwrap();
        s.running && s.generation == gen
    }

    fn run_lan_worker(&self, gen: u64, listener: TcpListener) {
        let _ = listener.set_nonblocking(true);
        let udp_disc = if cfg!(target_os = "macos") {
            if std::env::var_os("ULTRADESK_MACOS_LAN_UDP").is_some() {
                UdpSocket::bind(("0.0.0.0", DEFAULT_LAN_PORT)).ok()
            } else {
                None
            }
        } else {
            UdpSocket::bind(("0.0.0.0", DEFAULT_LAN_PORT)).ok()
        };
        if let Some(ref u) = udp_disc {
            let _ = u.set_nonblocking(true);
        }

        struct PendingLanPeer {
            stream: TcpStream,
            throttle_key: String,
            wire_ip: String,
            peer_port: u16,
            accepted_at: Instant,
            rx_frames: FrameStream,
        }

        struct ActiveLanPeer {
            stream: TcpStream,
            throttle_key: String,
            wire_ip: String,
            peer_port: u16,
            rx_frames: FrameStream,
            next_capture_at: Instant,
        }

        const MAX_PENDING_LAN_PEERS: usize = 16;
        const PREAUTH_TIMEOUT: Duration = Duration::from_millis(1000);

        let mut pending: Vec<PendingLanPeer> = Vec::new();
        let mut active_peer: Option<ActiveLanPeer> = None;
        let mut buf = [0u8; 65536];

        while self.is_worker_active(gen) {
            let mut had_activity = false;

            if let Some(ref u) = udp_disc {
                let mut disc_buf = [0u8; 256];
                while let Ok((n, src)) = u.recv_from(&mut disc_buf) {
                    had_activity = true;
                    if let Ok(txt) = std::str::from_utf8(&disc_buf[..n]) {
                        if let Some(queried) = txt.trim().strip_prefix("ULTRADESK_DISCOVER:") {
                            let (hid, did, lport) = {
                                let s = self.inner.lock().unwrap();
                                (s.host_id.clone(), s.device_id.clone(), s.lan_port)
                            };
                            let q = queried.trim();
                            if lport > 0 && (q.is_empty() || q == hid || q == did) {
                                let reply_id = if !hid.is_empty() { hid } else { did };
                                let reply = format!("ULTRADESK_HERE:{}:{}", reply_id, lport);
                                let _ = u.send_to(reply.as_bytes(), src);
                            }
                        }
                    }
                }
            }

            while let Ok((stream, peer_addr)) = listener.accept() {
                had_activity = true;
                let _ = stream.set_nonblocking(true);
                let _ = stream.set_nodelay(true);
                let throttle_key = peer_throttle_key(peer_addr.ip());
                let wire_ip = match peer_addr.ip() {
                    std::net::IpAddr::V4(v4) => v4.to_string(),
                    std::net::IpAddr::V6(v6) => v6
                        .to_ipv4_mapped()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "127.0.0.1".to_string()),
                };
                let peer_port = peer_addr.port().max(1);
                if pending.len() >= MAX_PENDING_LAN_PEERS {
                    pending.remove(0);
                }
                pending.push(PendingLanPeer {
                    stream,
                    throttle_key,
                    wire_ip,
                    peer_port,
                    accepted_at: Instant::now(),
                    rx_frames: FrameStream::new(false),
                });
            }

            let mut i = 0;
            while i < pending.len() {
                if pending[i].accepted_at.elapsed() >= PREAUTH_TIMEOUT {
                    pending.swap_remove(i);
                    continue;
                }

                let mut drop_peer = false;
                let mut peer_authenticated = false;

                match pending[i].stream.read(&mut buf) {
                    Ok(0) => {
                        drop_peer = true;
                    }
                    Ok(n) => {
                        had_activity = true;
                        let frames = pending[i].rx_frames.feed(&buf[..n]);
                        for frame in frames {
                            if let Ok(Some(pkt)) = decode_transport_body(&frame) {
                                if pkt.kind == "relay" || pkt.kind == "direct" {
                                    if let Some(decoded) = decode_peer_payload(&pkt.data, pkt.flag)
                                    {
                                        if active_peer.is_some() {
                                            if decoded.starts_with(b"login:") {
                                                if let Ok(auth_fail_frame) = encode_relay_frame(
                                                    &encode_host_auth_failed(),
                                                    &pending[i].wire_ip,
                                                    pending[i].peer_port,
                                                    0,
                                                ) {
                                                    let _ = pending[i]
                                                        .stream
                                                        .write_all(&auth_fail_frame);
                                                }
                                                drop_peer = true;
                                                break;
                                            }
                                        } else {
                                            let can_process = {
                                                let s = self.inner.lock().unwrap();
                                                !s.lan_viewer_active
                                                    && s.core.phase == HostPhase::None
                                                    && decoded.starts_with(b"login:")
                                            };
                                            if can_process {
                                                let actions = self
                                                    .handle_peer_payload_from_peer(
                                                        &pending[i].throttle_key,
                                                        &decoded,
                                                    )
                                                    .unwrap_or_default();
                                                let authenticated = actions.iter().any(|a| {
                                                    matches!(
                                                        a,
                                                        HostAction::PromptApproval(_)
                                                            | HostAction::SessionStarted(_)
                                                    )
                                                });
                                                if authenticated {
                                                    peer_authenticated = true;
                                                    break;
                                                } else {
                                                    let rejected: Vec<Vec<u8>> = {
                                                        let mut s = self.inner.lock().unwrap();
                                                        if !s.lan_viewer_active
                                                            && s.core.phase == HostPhase::None
                                                        {
                                                            std::mem::take(
                                                                &mut s.outbound_peer_packets,
                                                            )
                                                        } else {
                                                            Vec::new()
                                                        }
                                                    };
                                                    for p in rejected {
                                                        if let Ok(f) = encode_relay_frame(
                                                            &p,
                                                            &pending[i].wire_ip,
                                                            pending[i].peer_port,
                                                            0,
                                                        ) {
                                                            let _ = pending[i].stream.write_all(&f);
                                                        }
                                                    }
                                                    drop_peer = true;
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(ref e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(_) => {
                        drop_peer = true;
                    }
                }

                if peer_authenticated {
                    let mut peer = pending.swap_remove(i);
                    let _ = peer.stream.set_nonblocking(false);
                    let _ = peer.stream.set_read_timeout(Some(Duration::from_millis(5)));
                    let _ = peer.stream.set_write_timeout(Some(Duration::from_secs(2)));
                    let outbound: Vec<Vec<u8>> = {
                        let mut s = self.inner.lock().unwrap();
                        s.lan_viewer_active = true;
                        std::mem::take(&mut s.outbound_peer_packets)
                    };
                    for pkt in outbound {
                        if let Ok(frame) =
                            encode_relay_frame(&pkt, &peer.wire_ip, peer.peer_port, 0)
                        {
                            let _ = peer.stream.write_all(&frame);
                        }
                    }
                    active_peer = Some(ActiveLanPeer {
                        stream: peer.stream,
                        throttle_key: peer.throttle_key,
                        wire_ip: peer.wire_ip,
                        peer_port: peer.peer_port,
                        rx_frames: peer.rx_frames,
                        next_capture_at: Instant::now(),
                    });
                    break;
                } else if drop_peer {
                    pending.swap_remove(i);
                } else {
                    i += 1;
                }
            }

            if let Some(ref mut active) = active_peer {
                let mut disconnected = false;
                match active.stream.read(&mut buf) {
                    Ok(0) => {
                        disconnected = true;
                    }
                    Ok(n) => {
                        had_activity = true;
                        for frame in active.rx_frames.feed(&buf[..n]) {
                            if let Ok(Some(pkt)) = decode_transport_body(&frame) {
                                if pkt.kind == "relay" || pkt.kind == "direct" {
                                    if let Some(decoded) = decode_peer_payload(&pkt.data, pkt.flag)
                                    {
                                        let _ = self.handle_peer_payload_from_peer(
                                            &active.throttle_key,
                                            &decoded,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Err(ref e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(_) => {
                        disconnected = true;
                    }
                }

                let should_capture = {
                    let s = self.inner.lock().unwrap();
                    s.core.phase == HostPhase::Streaming
                        && !s.core.waiting_ack
                        && (s.capture_needed || Instant::now() >= active.next_capture_at)
                };
                if should_capture {
                    had_activity = true;
                    let capture_start = Instant::now();
                    let _ = self.step_capture();
                    let interval = {
                        let s = self.inner.lock().unwrap();
                        s.core.qos.target_interval()
                    };
                    active.next_capture_at =
                        Instant::now() + interval.saturating_sub(capture_start.elapsed());
                }

                let outbound: Vec<Vec<u8>> = {
                    let mut s = self.inner.lock().unwrap();
                    std::mem::take(&mut s.outbound_peer_packets)
                };
                for pkt in outbound {
                    had_activity = true;
                    if let Ok(frame) =
                        encode_relay_frame(&pkt, &active.wire_ip, active.peer_port, 0)
                    {
                        if active.stream.write_all(&frame).is_err() {
                            disconnected = true;
                            break;
                        }
                    }
                }

                if disconnected {
                    let mut s = self.inner.lock().unwrap();
                    s.lan_viewer_active = false;
                    let actions = s.core.end_session("LAN viewer disconnected");
                    Self::record_actions(&mut s, &actions);
                    s.outbound_peer_packets.clear();
                    active_peer = None;
                }
            }

            if active_peer.is_none() && !had_activity {
                thread::sleep(Duration::from_millis(10));
            }
        }

        if active_peer.is_some() {
            let mut s = self.inner.lock().unwrap();
            s.lan_viewer_active = false;
            let actions = s.core.end_session("LAN viewer disconnected");
            Self::record_actions(&mut s, &actions);
            s.outbound_peer_packets.clear();
        }
    }

    fn run_host_worker(
        &self,
        gen: u64,
        reg_host: &str,
        reg_port: u16,
        use_tls: bool,
        mut ready_tx: Option<std::sync::mpsc::Sender<()>>,
    ) {
        let (device_id, temp_pw, mac, master_proof, display_name) = {
            let s = self.inner.lock().unwrap();
            (
                s.device_id.clone(),
                s.core.temp_password.clone(),
                s.hardware_mac.clone(),
                s.core.master_password_proof.clone(),
                s.display_name.clone(),
            )
        };
        let local_ip = detect_local_ip();
        let computer_b64 = encode_base64(display_name.as_bytes());
        let user_raw = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "linux".to_string());
        let user_clean: String = user_raw
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(24)
            .collect();
        let user_name = if user_clean.is_empty() {
            "linux"
        } else {
            &user_clean
        };
        let candidates = candidate_registration_macs(&mac);
        let mut mac_idx = 0usize;

        while self.is_worker_active(gen) {
            let active_mac = candidates
                .get(mac_idx)
                .map(String::as_str)
                .unwrap_or(DEFAULT_FALLBACK_HOST_MAC);
            let Ok(reg_cmd) = build_host_registration_command(
                &device_id,
                &temp_pw,
                &local_ip,
                &computer_b64,
                active_mac,
                master_proof.as_deref(),
                user_name,
            ) else {
                if let Some(tx) = ready_tx.take() {
                    let _ = tx.send(());
                }
                return;
            };

            let Ok(mut reg) = RegistrationTransport::new(15.0, 16 * 1024 * 1024, true) else {
                if let Some(tx) = ready_tx.take() {
                    let _ = tx.send(());
                }
                return;
            };
            if reg.connect_with_tls(reg_host, reg_port, use_tls).is_err()
                || reg.send_command(&reg_cmd).is_err()
            {
                if let Some(tx) = ready_tx.take() {
                    let _ = tx.send(());
                }
                reg.close();
                if !use_tls {
                    return;
                }
                thread::sleep(Duration::from_secs(2));
                continue;
            };

            let mut reg_reply: Option<HostRegistrationReply> = None;
            let mut unregistered_mac = false;
            let reg_deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < reg_deadline && self.is_worker_active(gen) {
                match reg.receive_mode1_payload() {
                    Ok(payload) if payload.starts_with(b"3:") => {
                        if let Ok(parsed) = parse_host_registration_reply(&payload) {
                            reg_reply = Some(parsed);
                            break;
                        }
                    }
                    Ok(payload) if payload.starts_with(b"60:") => {
                        unregistered_mac = true;
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            let Some(reply) = reg_reply else {
                reg.close();
                if unregistered_mac && mac_idx + 1 < candidates.len() {
                    mac_idx += 1;
                    continue;
                }
                if let Some(tx) = ready_tx.take() {
                    let _ = tx.send(());
                }
                if !use_tls {
                    return;
                }
                thread::sleep(Duration::from_secs(2));
                continue;
            };

            {
                let mut s = self.inner.lock().unwrap();
                if s.running && s.generation == gen {
                    s.host_id = reply.id.clone();
                    s.hardware_mac = active_mac.to_string();
                }
            }
            if let Some(tx) = ready_tx.take() {
                let _ = tx.send(());
            }

            reg.set_timeout(0.03);
            let mut active_relay: Option<(
                SessionTransport,
                RelayNotification,
                Option<(u16, String)>,
            )> = None;
            let mut viewer_route: Option<(String, u16)> = None;
            let mut last_relay_rx = Instant::now();
            let mut last_keepalive = Instant::now();
            let mut next_capture_at = Instant::now();

            while self.is_worker_active(gen) {
                let is_streaming = {
                    let s = self.inner.lock().unwrap();
                    s.core.phase == HostPhase::Streaming
                };
                if is_streaming {
                    reg.set_timeout(0.005);
                    if let Some((ref mut relay, _, _)) = active_relay {
                        relay.set_timeout(0.005);
                    }
                } else {
                    reg.set_timeout(0.03);
                    if let Some((ref mut relay, _, _)) = active_relay {
                        relay.set_timeout(0.02);
                    }
                }

                if last_keepalive.elapsed() >= Duration::from_secs(50) {
                    last_keepalive = Instant::now();
                    let _ = reg.send_command(b"2");
                    if let Some((ref mut relay, _, _)) = active_relay {
                        let _ = relay.send_system(b"ping", false);
                    }
                }

                match reg.receive_mode1_payload() {
                    Ok(payload) => {
                        let text = String::from_utf8_lossy(&payload);
                        let allow_loopback =
                            !use_tls && (reg_host == "127.0.0.1" || reg_host == "localhost");
                        if let Some(notif) = parse_relay_notification_with_policy(
                            &text,
                            &reply.ping_list,
                            &reply.ping_host_name_list,
                            allow_loopback,
                        ) {
                            if let Some((_, ref cur_notif, Some((pub_port, ref pub_ip)))) =
                                active_relay
                            {
                                if *cur_notif == notif {
                                    let avail = encode_availability_reply(&notif, pub_port, pub_ip);
                                    let _ = reg.send_command(&avail);
                                    continue;
                                }
                            }
                            let can_replace_relay = {
                                let mut s = self.inner.lock().unwrap();
                                if s.core.phase == HostPhase::None {
                                    true
                                } else if !s.lan_viewer_active
                                    && last_relay_rx.elapsed() >= Duration::from_secs(2)
                                {
                                    let actions = s.core.end_session("Relay viewer timed out");
                                    Self::record_actions(&mut s, &actions);
                                    s.outbound_peer_packets.clear();
                                    true
                                } else {
                                    false
                                }
                            };
                            if can_replace_relay {
                                let relay_port = if use_tls { 443 } else { reg_port };
                                if let Ok(mut new_relay) =
                                    SessionTransport::new(5.0, 16 * 1024 * 1024, true)
                                {
                                    if new_relay
                                        .connect(
                                            &notif.relay_ip,
                                            relay_port,
                                            &notif.relay_host,
                                            use_tls,
                                        )
                                        .is_ok()
                                        && new_relay.send_system(b"0", false).is_ok()
                                    {
                                        new_relay.set_timeout(0.02);
                                        active_relay = Some((new_relay, notif, None));
                                        viewer_route = None;
                                        last_relay_rx = Instant::now();
                                    }
                                }
                            }
                        }
                    }
                    Err(e) if e.starts_with("TimeoutError") => {}
                    Err(_) => break,
                }

                if let Some((ref mut relay, ref notif, ref mut assigned_addr)) = active_relay {
                    match relay.receive_body() {
                        Ok(body) => match decode_transport_body(&body) {
                            Ok(Some(pkt))
                                if pkt.kind == "direct" && pkt.data.starts_with(b"0:") =>
                            {
                                if let Some((pub_port, pub_ip)) = parse_relay_address(&pkt.data) {
                                    *assigned_addr = Some((pub_port, pub_ip.clone()));
                                    let avail = encode_availability_reply(notif, pub_port, &pub_ip);
                                    let _ = reg.send_command(&avail);
                                }
                            }
                            Ok(Some(pkt)) if pkt.kind == "relay" => {
                                let peer_ip =
                                    std::net::Ipv4Addr::from(pkt.address_value).to_string();
                                let candidate_route = (peer_ip, pkt.port);
                                if let Some(decoded) = decode_peer_payload(&pkt.data, pkt.flag) {
                                    let is_login = decoded.starts_with(b"login:");
                                    let allow_packet = viewer_route.as_ref()
                                        == Some(&candidate_route)
                                        || (is_login
                                            && (viewer_route.is_none()
                                                || last_relay_rx.elapsed()
                                                    >= Duration::from_secs(2)));
                                    if allow_packet {
                                        let actions = self
                                            .handle_peer_payload_from_peer(
                                                &candidate_route.0,
                                                &decoded,
                                            )
                                            .unwrap_or_default();
                                        let started = actions.iter().any(|a| {
                                            matches!(
                                                a,
                                                HostAction::PromptApproval(_)
                                                    | HostAction::SessionStarted(_)
                                            )
                                        });
                                        let ended = actions
                                            .iter()
                                            .any(|a| matches!(a, HostAction::SessionEnded(_)));
                                        if started {
                                            viewer_route = Some(candidate_route);
                                            last_relay_rx = Instant::now();
                                        } else if ended
                                            && viewer_route.as_ref() == Some(&candidate_route)
                                        {
                                            let outbound: Vec<Vec<u8>> = {
                                                let mut s = self.inner.lock().unwrap();
                                                if s.lan_viewer_active {
                                                    Vec::new()
                                                } else {
                                                    std::mem::take(&mut s.outbound_peer_packets)
                                                }
                                            };
                                            for out_pkt in outbound {
                                                let _ = relay.send_relay(
                                                    &out_pkt,
                                                    &candidate_route.0,
                                                    candidate_route.1,
                                                    false,
                                                );
                                            }
                                            viewer_route = None;
                                        } else if viewer_route.as_ref() == Some(&candidate_route) {
                                            last_relay_rx = Instant::now();
                                        } else if viewer_route.is_none() {
                                            let rejected: Vec<Vec<u8>> = {
                                                let mut s = self.inner.lock().unwrap();
                                                if !s.lan_viewer_active
                                                    && s.core.phase == HostPhase::None
                                                {
                                                    std::mem::take(&mut s.outbound_peer_packets)
                                                } else {
                                                    Vec::new()
                                                }
                                            };
                                            for out_pkt in rejected {
                                                let _ = relay.send_relay(
                                                    &out_pkt,
                                                    &candidate_route.0,
                                                    candidate_route.1,
                                                    false,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        },
                        Err(e) if e.starts_with("TimeoutError") => {}
                        Err(_) => {
                            active_relay = None;
                            viewer_route = None;
                            let mut s = self.inner.lock().unwrap();
                            let actions = s.core.end_session("Relay disconnected");
                            Self::record_actions(&mut s, &actions);
                            s.outbound_peer_packets.clear();
                        }
                    }
                }

                let should_capture = {
                    let s = self.inner.lock().unwrap();
                    !s.lan_viewer_active
                        && s.core.phase == HostPhase::Streaming
                        && !s.core.waiting_ack
                        && (s.capture_needed || Instant::now() >= next_capture_at)
                };
                if should_capture {
                    let capture_start = Instant::now();
                    let _ = self.step_capture();
                    let interval = {
                        let s = self.inner.lock().unwrap();
                        s.core.qos.target_interval()
                    };
                    next_capture_at =
                        Instant::now() + interval.saturating_sub(capture_start.elapsed());
                }

                if let (Some((ref mut relay, _, _)), Some((ref peer_ip, peer_port))) =
                    (&mut active_relay, &viewer_route)
                {
                    let outbound: Vec<Vec<u8>> = {
                        let mut s = self.inner.lock().unwrap();
                        if s.lan_viewer_active {
                            Vec::new()
                        } else {
                            std::mem::take(&mut s.outbound_peer_packets)
                        }
                    };
                    for pkt in outbound {
                        let _ = relay.send_relay(&pkt, peer_ip, *peer_port, false);
                    }
                }
            }

            if let Some((mut relay, _, _)) = active_relay {
                if let Some((ref peer_ip, peer_port)) = viewer_route {
                    let outbound: Vec<Vec<u8>> = {
                        let mut s = self.inner.lock().unwrap();
                        if s.lan_viewer_active {
                            Vec::new()
                        } else {
                            std::mem::take(&mut s.outbound_peer_packets)
                        }
                    };
                    for pkt in outbound {
                        let _ = relay.send_relay(&pkt, peer_ip, peer_port, false);
                    }
                }
                relay.close();
            }
            reg.close();
            if !use_tls || !self.is_worker_active(gen) {
                break;
            }
            thread::sleep(Duration::from_secs(2));
        }
    }

    pub fn stop(&self) -> Vec<HostAction> {
        let mut s = self.inner.lock().unwrap();
        s.running = false;
        s.lan_port = 0;
        s.lan_viewer_active = false;
        s.generation = s.generation.wrapping_add(1);
        let actions = s.core.end_session("Host stopped");
        Self::record_actions(&mut s, &actions);
        actions
    }

    pub fn set_host_id(&self, id: &str) {
        let mut s = self.inner.lock().unwrap();
        s.host_id = id.to_string();
    }

    pub fn set_auto_approve(&self, auto_approve: bool) {
        let mut s = self.inner.lock().unwrap();
        s.core.auto_approve = auto_approve;
    }

    pub fn select_monitors(&self, selection: MonitorSelection) -> HostStatus {
        {
            let mut s = self.inner.lock().unwrap();
            s.monitor_selection = selection;
            Self::refresh_display_layout(&mut s);
        }
        self.status()
    }

    pub fn status(&self) -> HostStatus {
        let s = self.inner.lock().unwrap();
        let now = Instant::now();
        let total = s.full_monitors.len();
        let selected_monitors = match &s.monitor_selection {
            MonitorSelection::All => (0..total).collect(),
            MonitorSelection::Index(idx) => {
                if *idx < total {
                    vec![*idx]
                } else {
                    (0..total).collect()
                }
            }
            MonitorSelection::Subset(v) => {
                let mut valid: Vec<usize> = v.iter().copied().filter(|&i| i < total).collect();
                valid.sort_unstable();
                valid.dedup();
                if valid.is_empty() {
                    (0..total).collect()
                } else {
                    valid
                }
            }
        };
        let selected_monitor = match &s.monitor_selection {
            MonitorSelection::Index(idx) if *idx < total => Some(*idx),
            _ if selected_monitors.len() == 1 && total > 1 => Some(selected_monitors[0]),
            _ => None,
        };
        HostStatus {
            running: s.running,
            host_id: s.host_id.clone(),
            temp_password: s.core.temp_password.clone(),
            hardware_mac: s.hardware_mac.clone(),
            has_master_password: s.core.master_password_proof.is_some(),
            auto_approve: s.core.auto_approve,
            phase: s.core.phase,
            viewer_id: s.core.active_viewer.as_ref().map(|v| v.claimed_id.clone()),
            viewer_name: s
                .core
                .active_viewer
                .as_ref()
                .map(|v| v.display_name.clone()),
            control_allowed: s.core.control_allowed,
            waiting_ack: s.core.waiting_ack,
            frame_seq: s.core.next_frame_seq,
            is_locked: s.core.throttle.is_locked(now),
            backend_name: s.backend.name.to_string(),
            monitors: s
                .full_monitors
                .iter()
                .map(|m| (m.left, m.top, m.right, m.bottom))
                .collect(),
            selected_monitor,
            selected_monitors,
            lan_port: s.lan_port,
            e2e_enabled: s.core.e2e.is_enabled(),
            e2e_cipher: s.core.e2e.cipher_name().to_string(),
            e2e_fingerprint: s.core.e2e.fingerprint().to_string(),
            macos_permissions: MacOsPermissionState::preflight(),
        }
    }

    pub fn set_master_password(&self, pw: Option<&str>) {
        let mut s = self.inner.lock().unwrap();
        s.core.master_password_proof = pw.map(compute_master_password_proof);
    }

    pub fn set_master_password_proof(&self, proof: Option<String>) {
        let mut s = self.inner.lock().unwrap();
        s.core.master_password_proof = proof;
    }

    pub fn approve(&self, allow_control: bool) -> Result<Vec<HostAction>, String> {
        let _ = MacOsPermissionState::request(MacOsPermissionKind::ScreenRecording);
        if allow_control {
            let _ = MacOsPermissionState::request(MacOsPermissionKind::Accessibility);
        }
        let mut s = self.inner.lock().unwrap();
        Self::refresh_display_layout(&mut s);
        let actions = s.core.approve(allow_control);
        Self::record_actions(&mut s, &actions);
        Ok(actions)
    }

    pub fn decline(&self) -> Result<Vec<HostAction>, String> {
        let mut s = self.inner.lock().unwrap();
        let actions = s.core.decline();
        Self::record_actions(&mut s, &actions);
        Ok(actions)
    }

    pub fn set_control_allowed(&self, allowed: bool) -> Result<Vec<HostAction>, String> {
        if allowed {
            let _ = MacOsPermissionState::request(MacOsPermissionKind::Accessibility);
        }
        let mut s = self.inner.lock().unwrap();
        let actions = s.core.set_control_allowed(allowed);
        Self::record_actions(&mut s, &actions);
        Ok(actions)
    }

    pub fn handle_peer_payload(&self, bytes: &[u8]) -> Result<Vec<HostAction>, String> {
        let mut s = self.inner.lock().unwrap();
        if bytes.starts_with(b"login:") {
            Self::refresh_display_layout(&mut s);
        }
        let actions = s.core.handle_packet(bytes, Instant::now());
        Self::record_actions(&mut s, &actions);
        Ok(actions)
    }

    pub fn handle_peer_payload_from_peer(
        &self,
        peer_key: &str,
        bytes: &[u8],
    ) -> Result<Vec<HostAction>, String> {
        let mut s = self.inner.lock().unwrap();
        let now = Instant::now();
        if bytes.starts_with(b"login:") {
            Self::refresh_display_layout(&mut s);
            s.peer_throttles.retain(|_, t| !t.is_expired(now));
            if s.peer_throttles.len() >= 256 && !s.peer_throttles.contains_key(peer_key) {
                let evict_candidate = s
                    .peer_throttles
                    .iter()
                    .filter(|(_, t)| !t.is_locked(now))
                    .min_by_key(|(_, t)| t.last_activity())
                    .map(|(k, _)| k.clone());
                if let Some(k) = evict_candidate {
                    s.peer_throttles.remove(&k);
                } else {
                    return Ok(vec![HostAction::SendPeer(encode_host_auth_failed())]);
                }
            }
            let peer_throttle = s
                .peer_throttles
                .entry(peer_key.to_string())
                .or_default()
                .clone();
            let saved_global = std::mem::replace(&mut s.core.throttle, peer_throttle);
            let actions = s.core.handle_packet(bytes, now);
            let updated_peer = std::mem::replace(&mut s.core.throttle, saved_global);
            s.peer_throttles.insert(peer_key.to_string(), updated_peer);
            Self::record_actions(&mut s, &actions);
            return Ok(actions);
        }
        let actions = s.core.handle_packet(bytes, now);
        Self::record_actions(&mut s, &actions);
        Ok(actions)
    }

    pub fn step_capture(&self) -> Result<Vec<HostAction>, String> {
        let mut s = self.inner.lock().unwrap();
        if s.core.phase != HostPhase::Streaming || s.core.waiting_ack {
            return Ok(Vec::new());
        }
        s.capture_needed = false;
        let rect = MonitorRect {
            left: s.core.layout.virtual_left,
            top: s.core.layout.virtual_top,
            right: s.core.layout.virtual_left + s.core.layout.width as i32,
            bottom: s.core.layout.virtual_top + s.core.layout.height as i32,
        };
        let raw = s.backend.capturer.capture_screen(rect)?;

        if s.force_keyframe || s.last_frame.is_empty() || s.last_frame.len() != raw.len() {
            let actions = s.core.send_frame(&raw)?;
            s.last_frame = raw;
            s.force_keyframe = false;
            Self::record_actions(&mut s, &actions);
            return Ok(actions);
        }

        match compute_dirty_rect(
            &s.last_frame,
            &raw,
            s.core.layout.width,
            s.core.layout.height,
        ) {
            None => Ok(Vec::new()),
            Some((dx, dy, dw, dh)) => {
                let total_pixels = (s.core.layout.width as u64) * (s.core.layout.height as u64);
                let dirty_pixels = (dw as u64) * (dh as u64);
                let actions = if dirty_pixels * 4 < total_pixels * 3 {
                    let mut sub_raw = Vec::with_capacity((dw as usize) * (dh as usize) * 2);
                    let full_row_bytes = (s.core.layout.width as usize) * 2;
                    let sub_row_bytes = (dw as usize) * 2;
                    for r in 0..dh {
                        let src_start = ((dy + r) as usize) * full_row_bytes + (dx as usize) * 2;
                        sub_raw.extend_from_slice(&raw[src_start..src_start + sub_row_bytes]);
                    }
                    s.core.send_frame_rect((dx, dy, dw, dh), &sub_raw)?
                } else {
                    s.core.send_frame(&raw)?
                };
                s.last_frame = raw;
                Self::record_actions(&mut s, &actions);
                Ok(actions)
            }
        }
    }

    pub fn capture_rgb888_snapshot(&self) -> Result<(u32, u32, Vec<u8>), String> {
        let mut s = self.inner.lock().unwrap();
        Self::refresh_display_layout(&mut s);
        let w = s.core.layout.width.max(1);
        let h = s.core.layout.height.max(1);
        let rect = MonitorRect {
            left: s.core.layout.virtual_left,
            top: s.core.layout.virtual_top,
            right: s.core.layout.virtual_left + w as i32,
            bottom: s.core.layout.virtual_top + h as i32,
        };
        let raw = s.backend.capturer.capture_screen(rect)?;
        let pixel_count = (w as usize) * (h as usize);
        if raw.len() < pixel_count * 2 {
            return Err("captured host frame buffer shorter than expected".to_string());
        }
        let mut rgb = Vec::with_capacity(pixel_count * 3);
        for i in 0..pixel_count {
            let v = u16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]);
            let r5 = ((v >> 10) & 0x1F) as u8;
            let g5 = ((v >> 5) & 0x1F) as u8;
            let b5 = (v & 0x1F) as u8;
            rgb.push((r5 << 3) | (r5 >> 2));
            rgb.push((g5 << 3) | (g5 >> 2));
            rgb.push((b5 << 3) | (b5 >> 2));
        }
        Ok((w, h, rgb))
    }

    pub fn inject_local_wire_input(&self, pkt: &[u8]) -> Result<(), String> {
        let mut s = self.inner.lock().unwrap();
        let ox = s.core.layout.virtual_left;
        let oy = s.core.layout.virtual_top;
        let translated = offset_host_input_packet(pkt, ox, oy);
        s.backend.injector.inject_event(&translated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::decode_byte_array_fields_checked;
    use crate::screen::{RGB555Screen, ZstdDictionaryDecoder};

    fn sample_login_payload(password: &str, colors: &str, control: i16) -> Vec<u8> {
        let nonce = "12345678";
        let proof = md5_hex_lower(format!("{}{}", nonce, password).as_bytes());
        format!(
            "login:{nonce}:{proof}:1920:1080:7:15:15:30:16:111222333:Windows Viewer:{control}:{}:x:conn:{colors}:45:0:0::6.6.133:0",
            "0".repeat(32)
        )
        .into_bytes()
    }

    #[test]
    fn test_relay_notification_parsing() {
        let legacy = "11:192.0.2.10:unused:relay.ultraviewer.net";
        let notif = parse_relay_notification(legacy).unwrap();
        assert_eq!(notif.relay_ip, "192.0.2.10");
        assert_eq!(notif.relay_host, "relay.ultraviewer.net");
        assert!(notif.tag.is_none());

        let modern = "17:request-tag:request-hash:192.0.2.20;192.0.2.21:relay2.ultraviewer.net;relay3.ultraviewer.net:0:-1:60:0:";
        let notif_m = parse_relay_notification(modern).unwrap();
        assert_eq!(notif_m.tag.as_deref(), Some("request-tag"));
        assert_eq!(notif_m.hash.as_deref(), Some("request-hash"));
        assert_eq!(notif_m.relay_ip, "192.0.2.20");
        assert_eq!(notif_m.relay_host, "relay2.ultraviewer.net");
    }

    #[test]
    fn test_login_parsing_and_credentials() {
        let payload = sample_login_payload("mypassword", "2", 1);
        let login = parse_viewer_login(&String::from_utf8_lossy(&payload)).unwrap();
        assert_eq!(login.claimed_id, "111222333");
        assert_eq!(login.display_name, "Windows Viewer");
        assert!(login.wants_control);
        assert_eq!(login.color_mode, 2);

        let (valid, is_master) = verify_login_credentials(&login, "mypassword", None);
        assert!(valid);
        assert!(!is_master);

        let (wrong, _) = verify_login_credentials(&login, "wrongpass", None);
        assert!(!wrong);

        let master_proof = compute_master_password_proof("mastersecret");
        let master_login_payload = format!(
            "login:12345678:wrongrandomproof:1920:1080:7:15:15:30:16:111222333:Master Viewer:1:{}:x:conn:2:45:0:0::6.6.133:0",
            master_proof
        );
        let m_login = parse_viewer_login(&master_login_payload).unwrap();
        let (m_valid, m_is_master) =
            verify_login_credentials(&m_login, "mypassword", Some(&master_proof));
        assert!(m_valid);
        assert!(m_is_master);
    }

    #[test]
    fn test_five_failure_lockout() {
        let mut throttle = LoginThrottle::default();
        let now = Instant::now();
        for _ in 0..4 {
            assert!(!throttle.record_failure(now));
            assert!(!throttle.is_locked(now));
        }
        assert!(throttle.record_failure(now));
        assert!(throttle.is_locked(now));
        assert!(throttle.is_locked(now + Duration::from_secs(299)));
        assert!(!throttle.is_locked(now + Duration::from_secs(300)));
    }

    #[test]
    fn test_full_host_handshake_flow_and_screen_decode() {
        let layout = DisplayLayout {
            width: 4,
            height: 4,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![MonitorRect {
                left: 0,
                top: 0,
                right: 4,
                bottom: 4,
            }],
        };
        let mut core = HostCore::new("temp_pass", None, layout.clone());
        let now = Instant::now();

        let wrong_payload = sample_login_payload("wrong_pass", "2", 1);
        let actions = core.handle_packet(&wrong_payload, now);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0], HostAction::SendPeer(encode_host_auth_failed()));

        let valid_payload = sample_login_payload("temp_pass", "2", 1);
        let actions = core.handle_packet(&valid_payload, now);
        assert_eq!(core.phase, HostPhase::Approval);
        assert!(
            matches!(&actions[0], HostAction::PromptApproval(v) if v.claimed_id == "111222333")
        );

        let approve_actions = core.approve(true);
        assert_eq!(core.phase, HostPhase::Starting);
        assert_eq!(approve_actions.len(), 3);
        let init_cmd = match &approve_actions[0] {
            HostAction::SendPeer(p) => p.clone(),
            _ => panic!("expected SendPeer"),
        };
        let fields = decode_byte_array_fields_checked(&init_cmd, Some(11)).unwrap();
        assert_eq!(fields[0], vec![0]);
        assert_eq!(fields[6], HOST_VERSION);

        let start_actions = core.handle_packet(b"17", now);
        assert_eq!(core.phase, HostPhase::Streaming);
        assert_eq!(start_actions, vec![HostAction::RequestCapture]);

        let mut raw_pixels = Vec::new();
        for _ in 0..16 {
            raw_pixels.extend_from_slice(&0x7C00u16.to_le_bytes());
        }
        let frame_actions = core.send_frame(&raw_pixels).unwrap();
        assert_eq!(frame_actions.len(), 1);
        assert!(core.waiting_ack);

        let frame_cmd = match &frame_actions[0] {
            HostAction::SendPeer(p) => p.clone(),
            _ => panic!("expected SendPeer"),
        };

        let frame_fields = decode_byte_array_fields_checked(&frame_cmd, Some(11)).unwrap();
        assert_eq!(frame_fields[0], vec![1]);
        assert_eq!(frame_fields[4], FIRST_FRAME_NUMBER.to_le_bytes());

        let mut screen = RGB555Screen::new((4, 4), ZstdDictionaryDecoder::default(), 100, 32);
        let applied = screen.apply(&frame_fields).unwrap();
        assert_eq!(applied, 1);
        assert_eq!(screen.canvas.get_pixel(0, 0), (255, 0, 0));
        assert_eq!(screen.canvas.get_pixel(3, 3), (255, 0, 0));

        let ack_actions = core.handle_packet(b"18", now);
        assert!(!core.waiting_ack);
        assert_eq!(ack_actions, vec![HostAction::RequestCapture]);

        let mouse_pkt = b"8|||512|||0|||2|||2";
        let input_actions = core.handle_packet(mouse_pkt, now);
        assert_eq!(
            input_actions,
            vec![HostAction::InjectInput(mouse_pkt.to_vec())]
        );

        let backend = DesktopBackend::headless(4, 4);
        let session = HostSession::new("temp_pass", None, layout, backend);
        assert_eq!(session.status().phase, HostPhase::None);
    }

    const BGRX_FORMAT: X11PixelFormat = X11PixelFormat {
        bits_per_pixel: 32,
        scanline_pad: 32,
        msb_first: false,
        red_mask: 0xff0000,
        green_mask: 0xff00,
        blue_mask: 0xff,
    };

    #[test]
    fn test_x11_zpixmap_conversions_and_keymap() {
        let data = [0, 0, 255, 0, 255, 0, 0, 0];
        let mut out = Vec::new();
        zpixmap_to_rgb555(&data, 1, 2, &BGRX_FORMAT, &mut out).unwrap();
        assert_eq!(out, [0x1f, 0, 0, 0x7c]);

        let mut be = Vec::new();
        zpixmap_to_rgb555(
            &[0, 255, 0, 0],
            1,
            1,
            &X11PixelFormat {
                msb_first: true,
                ..BGRX_FORMAT
            },
            &mut be,
        )
        .unwrap();
        assert_eq!(be, [0, 0x7c]);

        let f24 = X11PixelFormat {
            bits_per_pixel: 24,
            ..BGRX_FORMAT
        };
        let mut out24 = Vec::new();
        zpixmap_to_rgb555(&[0, 255, 0, 9], 1, 1, &f24, &mut out24).unwrap();
        assert_eq!(out24, 0x03e0u16.to_le_bytes());

        let f30 = X11PixelFormat {
            red_mask: 0x3ff0_0000,
            green_mask: 0x000f_fc00,
            blue_mask: 0x0000_03ff,
            ..BGRX_FORMAT
        };
        let mut out30 = Vec::new();
        zpixmap_to_rgb555(&0x3ff0_0000u32.to_le_bytes(), 1, 1, &f30, &mut out30).unwrap();
        assert_eq!(out30, 0x7c00u16.to_le_bytes());

        assert_eq!(vk_keysym(0x41, 0x1e), Some(0x61));
        assert_eq!(vk_keysym(0x0d, 0xe01c), Some(0xff8d));
        assert_eq!(char_keysym(0x20ac), Some(0x10020ac));
        assert_eq!(char_keysym(0xd800), None);
        assert_eq!(
            (
                evdev_keycode(0x1e),
                evdev_keycode(0xe048),
                evdev_keycode(0xe0ff)
            ),
            (Some(0x1e), Some(103), None)
        );
    }

    #[test]
    fn test_official_windows_viewer_wire_requirements() {
        let layout = DisplayLayout {
            width: 1920,
            height: 1080,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![
                MonitorRect {
                    left: 0,
                    top: 0,
                    right: 960,
                    bottom: 1080,
                },
                MonitorRect {
                    left: 960,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
            ],
        };
        let init_bytes = encode_host_screen_init(&layout);
        let init_fields = decode_byte_array_fields_checked(&init_bytes, Some(11)).unwrap();
        assert_eq!(init_fields[7], vec![1]);
        assert_eq!(init_fields[8], b"0:0:960:1080|960:0:1920:1080|");

        let raw = vec![0x1fu8; 4 * 4 * 2];
        let frame_bytes = encode_host_frame((0, 0, 4, 4), &raw, FIRST_FRAME_NUMBER).unwrap();
        let frame_fields = decode_byte_array_fields_checked(&frame_bytes, Some(11)).unwrap();
        assert_eq!(frame_fields[1], vec![0, 0, 0, 0, 4, 0, 4, 0]);
    }

    #[test]
    fn test_live_host_worker_registration_relay_and_windows_viewer_session() {
        use crate::protocol::{
            decrypt_registration_bytes, encode_relay_frame, encode_typed_transport_frame,
            FrameStream, REGISTRATION_PASSWORD,
        };
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let temp_pw = "12345678";

        let server_handle = std::thread::spawn(move || {
            let (mut reg_stream, _) = listener.accept().unwrap();
            reg_stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            reg_stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
            reg_stream.flush().unwrap();
            let mut reg_rx = FrameStream::new(true);
            let mut buf = [0u8; 4096];

            let reg_cmd = loop {
                let n = reg_stream.read(&mut buf).unwrap();
                assert!(n > 0);
                let frames = reg_rx.feed(&buf[..n]);
                if let Some(frame) = frames.into_iter().next() {
                    if let Ok(Some(body)) = decode_transport_body(&frame) {
                        if body.kind == "direct" && body.data.first() == Some(&0) {
                            let plain =
                                decrypt_registration_bytes(&body.data[1..], REGISTRATION_PASSWORD)
                                    .unwrap();
                            break String::from_utf8(plain).unwrap();
                        }
                    }
                }
            };
            assert!(reg_cmd.starts_with("3:"));

            let reply = format!(
                "3:{}:00987654321:203.0.113.7::0:127.0.0.1:-1:17/10/2026:localhost:server-tok",
                port
            );
            let reply_wire = encode_typed_transport_frame(reply.as_bytes(), 1);
            reg_stream.write_all(&reply_wire).unwrap();
            reg_stream.flush().unwrap();

            let notify = "17:req-tag-1:req-hash-1::localhost:0:-1:60:0:";
            let notify_wire = encode_typed_transport_frame(notify.as_bytes(), 1);
            reg_stream.write_all(&notify_wire).unwrap();
            reg_stream.flush().unwrap();

            let (mut relay_stream, _) = listener.accept().unwrap();
            relay_stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            relay_stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
            relay_stream.flush().unwrap();
            let mut relay_rx = FrameStream::new(true);
            let relay_hello = loop {
                let n = relay_stream.read(&mut buf).unwrap();
                assert!(n > 0);
                let frames = relay_rx.feed(&buf[..n]);
                if let Some(frame) = frames.into_iter().next() {
                    if let Ok(Some(body)) = decode_transport_body(&frame) {
                        if body.kind == "direct" {
                            break body.data;
                        }
                    }
                }
            };
            assert_eq!(relay_hello, b"0");

            let relay_assigned = encode_typed_transport_frame(b"0:54321:198.51.100.9:6.6.133", 1);
            relay_stream.write_all(&relay_assigned).unwrap();
            relay_stream.flush().unwrap();

            let avail_reply = loop {
                let n = reg_stream.read(&mut buf).unwrap();
                assert!(n > 0);
                let frames = reg_rx.feed(&buf[..n]);
                if let Some(frame) = frames.into_iter().next() {
                    if let Ok(Some(body)) = decode_transport_body(&frame) {
                        if body.kind == "direct" && body.data.first() == Some(&0) {
                            let plain =
                                decrypt_registration_bytes(&body.data[1..], REGISTRATION_PASSWORD)
                                    .unwrap();
                            break String::from_utf8(plain).unwrap();
                        }
                    }
                }
            };
            assert_eq!(
                avail_reply,
                "17:req-tag-1:req-hash-1:127.0.0.1:54321:198.51.100.9:0:|0"
            );

            let utf16le = |s: &str| -> Vec<u8> {
                s.encode_utf16()
                    .flat_map(|u| u.to_le_bytes())
                    .collect::<Vec<u8>>()
            };

            let login_str = String::from_utf8(sample_login_payload(temp_pw, "2", 1)).unwrap();
            let login_frame =
                encode_relay_frame(&utf16le(&login_str), "198.51.100.9", 54321, 1).unwrap();
            relay_stream.write_all(&login_frame).unwrap();
            relay_stream.flush().unwrap();

            let mut received_peer = Vec::new();
            while received_peer.len() < 2 {
                let n = relay_stream.read(&mut buf).unwrap();
                assert!(n > 0);
                for f in relay_rx.feed(&buf[..n]) {
                    if let Ok(Some(body)) = decode_transport_body(&f) {
                        if body.kind == "relay" {
                            received_peer.push(body.data);
                        }
                    }
                }
            }
            let init_fields =
                decode_byte_array_fields_checked(&received_peer[0], Some(11)).unwrap();
            assert_eq!(init_fields[0], vec![0]);
            assert_eq!(init_fields[7], vec![1]);

            let start_frame = encode_relay_frame(&utf16le("17"), "198.51.100.9", 54321, 1).unwrap();
            relay_stream.write_all(&start_frame).unwrap();
            relay_stream.flush().unwrap();

            let screen_pkt = loop {
                let n = relay_stream.read(&mut buf).unwrap();
                assert!(n > 0);
                let mut found = None;
                for f in relay_rx.feed(&buf[..n]) {
                    if let Ok(Some(body)) = decode_transport_body(&f) {
                        if body.kind == "relay" {
                            if let Ok(fields) = decode_byte_array_fields_checked(&body.data, None) {
                                if !fields.is_empty() && fields[0] == vec![1] {
                                    found = Some(fields);
                                    break;
                                }
                            }
                        }
                    }
                }
                if let Some(fields) = found {
                    break fields;
                }
            };
            assert_eq!(screen_pkt[1].len(), 8);
        });

        let layout = DisplayLayout {
            width: 8,
            height: 8,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![MonitorRect {
                left: 0,
                top: 0,
                right: 8,
                bottom: 8,
            }],
        };
        let backend = DesktopBackend::headless(8, 8);
        let session = HostSession::with_profile(
            "FEDCBA9876543210FEDCBA9876543210",
            "",
            "001122334455",
            temp_pw,
            None,
            "UltraDeskHost",
            layout,
            backend,
        );
        session.set_auto_approve(true);
        let st = session.start_with_endpoint("127.0.0.1", port, false);
        assert!(st.running);
        assert_eq!(st.host_id, "987654321");
        let st2 = session.start_with_endpoint("127.0.0.1", port, false);
        assert!(st2.running);
        assert_eq!(st2.host_id, "987654321");

        server_handle.join().unwrap();
        let _ = session.stop();
        assert!(!session.status().running);
    }

    #[test]
    fn test_wayland_ppm_parser_and_windows_control_mode_zero() {
        let mut ppm = b"P6\n2 1\n255\n".to_vec();
        ppm.extend_from_slice(&[255, 0, 0, 0, 0, 255]);
        let (w, h, rgb555) = parse_ppm_to_rgb555_top_down(&ppm).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgb555, [0x00, 0x7c, 0x1f, 0x00]);

        let login_zero = sample_login_payload("mypassword", "2", 0);
        let parsed_zero = parse_viewer_login(&String::from_utf8_lossy(&login_zero)).unwrap();
        assert!(parsed_zero.wants_control);

        let login_re = sample_login_payload("mypassword", "2", -1);
        let parsed_re = parse_viewer_login(&String::from_utf8_lossy(&login_re)).unwrap();
        assert!(parsed_re.wants_control);

        let login_view = sample_login_payload("mypassword", "2", 2);
        let parsed_view = parse_viewer_login(&String::from_utf8_lossy(&login_view)).unwrap();
        assert!(!parsed_view.wants_control);

        assert_eq!(vk_to_evdev(0x41), Some(30));
        assert_eq!(ascii_to_evdev(b'A' as u16), Some((30, true)));
    }

    #[test]
    fn test_host_multi_monitor_selection_and_input_offset() {
        let niri_json = r#"{
            "eDP-1": {"logical": {"x": 3720, "y": 0, "width": 1920, "height": 1200}},
            "DP-2": {"logical": {"x": 1800, "y": 0, "width": 1920, "height": 1080}}
        }"#;
        let mons = parse_niri_outputs_json(niri_json);
        assert_eq!(
            mons,
            vec![
                MonitorRect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                MonitorRect {
                    left: 1920,
                    top: 0,
                    right: 3840,
                    bottom: 1200,
                },
            ]
        );

        let move_pkt = crate::input::mouse_move(100, 250).unwrap();
        let shifted = offset_host_input_packet(&move_pkt, 1920, 0);
        assert_eq!(
            decode_host_input(&shifted),
            Some(HostInputEvent::Mouse {
                msg: 0x0200,
                wparam: 0,
                x: 2020,
                y: 250,
                wheel: 0,
            })
        );

        let layout = DisplayLayout {
            width: 3840,
            height: 1200,
            virtual_left: 0,
            virtual_top: 0,
            monitors: mons.clone(),
        };
        let backend = DesktopBackend::headless(3840, 1200);
        let session = HostSession::with_profile(
            "FEDCBA9876543210FEDCBA9876543210",
            "",
            "001122334455",
            "81617177",
            None,
            "UltraDeskHost",
            layout,
            backend,
        );
        let st_all = session.status();
        assert_eq!(st_all.monitors.len(), 2);
        assert_eq!(st_all.selected_monitor, None);
        assert_eq!(st_all.selected_monitors, vec![0, 1]);

        let st_m1 = session.select_monitors(MonitorSelection::Index(1));
        assert_eq!(st_m1.selected_monitor, Some(1));
        assert_eq!(st_m1.selected_monitors, vec![1]);

        session.set_auto_approve(true);
        let login = sample_login_payload("81617177", "2", 0);
        session.handle_peer_payload(&login).unwrap();
        let actions_chat = session.handle_peer_payload(b"12|||hello cc").unwrap();
        assert_eq!(
            actions_chat,
            vec![HostAction::ChatReceived("hello cc".to_string())]
        );
        let (chats, clip) = session.drain_auxiliary_events();
        assert_eq!(chats, vec!["Partner: hello cc".to_string()]);
        assert_eq!(clip, None);
        assert!(session.send_chat("xin chào"));
    }

    #[test]
    fn test_video_qos_and_compute_dirty_rect() {
        let mut qos = VideoQos::new();
        assert_eq!(qos.target_fps, 30);
        assert_eq!(qos.target_interval(), Duration::from_millis(33));

        let t0 = Instant::now();
        qos.on_frame_sent(t0);
        qos.on_frame_ack(t0 + Duration::from_millis(10));
        assert!(qos.rtt_ema_ms < 25.0);
        assert_eq!(qos.target_fps, 60);
        assert_eq!(qos.target_interval(), Duration::from_millis(16));

        let w = 10u32;
        let h = 10u32;
        let prev = vec![0u8; (w * h * 2) as usize];
        let curr = prev.clone();
        assert_eq!(compute_dirty_rect(&prev, &curr, w, h), None);

        let mut changed = prev.clone();
        let idx = (2 * w * 2 + 3 * 2) as usize;
        changed[idx] = 1;
        let rect = compute_dirty_rect(&prev, &changed, w, h);
        assert_eq!(rect, Some((3, 2, 1, 1)));

        let mut changed_box = prev.clone();
        for y in 2..=5 {
            for x in 3..=7 {
                let i = ((y * w + x) * 2) as usize;
                changed_box[i] = 0xAA;
            }
        }
        assert_eq!(
            compute_dirty_rect(&prev, &changed_box, w, h),
            Some((3, 2, 5, 4))
        );
    }

    #[test]
    fn test_host_core_e2e_handshake_auto_and_manual_approve_and_session_packets() {
        let layout = DisplayLayout {
            width: 4,
            height: 4,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![MonitorRect {
                left: 0,
                top: 0,
                right: 4,
                bottom: 4,
            }],
        };
        let now = Instant::now();
        let nonce = "12345678";
        let temp_pw = "temp_pass";
        let proof = md5_hex_lower(format!("{}{}", nonce, temp_pw).as_bytes());
        let (offer_wire, viewer_offer_state) =
            crate::e2e::create_viewer_offer_for_password(nonce, temp_pw, "");

        let login_pkt = format!(
            "login:{nonce}:{proof}:1920:1080:7:15:15:30:16:111222333:E2E Viewer:1:{}:x:conn:2:45:0:0:{offer_wire}:6.6.133:0",
            "0".repeat(32)
        )
        .into_bytes();

        let mut manual_core = HostCore::new(temp_pw, None, layout.clone());
        let prompt_actions = manual_core.handle_packet(&login_pkt, now);
        assert_eq!(manual_core.phase, HostPhase::Approval);
        assert!(!manual_core.e2e.is_enabled());
        assert!(!manual_core.e2e.fingerprint().is_empty());
        assert!(matches!(
            prompt_actions.as_slice(),
            [HostAction::PromptApproval(_)]
        ));

        let approve_actions = manual_core.approve(true);
        assert_eq!(manual_core.phase, HostPhase::Starting);
        assert!(manual_core.e2e.is_enabled());
        assert_eq!(approve_actions.len(), 3);

        let init_plain = match &approve_actions[0] {
            HostAction::SendPeer(p) => p.clone(),
            _ => panic!("expected plaintext screen_init"),
        };
        assert!(!crate::e2e::is_e2e_frame(&init_plain));
        let init_fields = decode_byte_array_fields_checked(&init_plain, Some(11)).unwrap();
        assert_eq!(init_fields[0], vec![0]);
        assert!(init_fields[3].starts_with(crate::e2e::E2E_HANDSHAKE_PREFIX.as_bytes()));

        let mut viewer_e2e =
            crate::e2e::finalize_viewer_handshake(&viewer_offer_state, &init_fields[3]).unwrap();
        assert!(viewer_e2e.is_enabled());
        assert_eq!(viewer_e2e.fingerprint(), manual_core.e2e.fingerprint());

        let ctrl_sealed = match &approve_actions[1] {
            HostAction::SendPeer(p) => p.clone(),
            _ => panic!("expected sealed control_permission"),
        };
        assert!(crate::e2e::is_e2e_frame(&ctrl_sealed));
        assert_eq!(manual_core.e2e.cipher_mut().unwrap().tx_seqnum, 1);

        let (ctrl_plain, _) = viewer_e2e.cipher_mut().unwrap().open(&ctrl_sealed).unwrap();
        assert_eq!(ctrl_plain, encode_host_control_permission(true));
        assert_eq!(viewer_e2e.cipher_mut().unwrap().rx_seqnum, 1);

        let mut auto_core = HostCore::new(temp_pw, None, layout.clone());
        auto_core.auto_approve = true;
        let (offer_wire_2, viewer_offer_2) =
            crate::e2e::create_viewer_offer_for_password(nonce, temp_pw, "");
        let login_pkt_2 = format!(
            "login:{nonce}:{proof}:1920:1080:7:15:15:30:16:111222333:E2E Viewer:1:{}:x:conn:2:45:0:0:{offer_wire_2}:6.6.133:0",
            "0".repeat(32)
        )
        .into_bytes();

        let auto_actions = auto_core.handle_packet(&login_pkt_2, now);
        assert_eq!(auto_core.phase, HostPhase::Starting);
        assert!(auto_core.e2e.is_enabled());
        assert_eq!(auto_actions.len(), 3);

        let auto_init = match &auto_actions[0] {
            HostAction::SendPeer(p) => p.clone(),
            _ => panic!("expected screen_init"),
        };
        let auto_init_fields = decode_byte_array_fields_checked(&auto_init, Some(11)).unwrap();
        let mut viewer_e2e_2 =
            crate::e2e::finalize_viewer_handshake(&viewer_offer_2, &auto_init_fields[3]).unwrap();
        assert_eq!(viewer_e2e_2.fingerprint(), auto_core.e2e.fingerprint());

        let auto_ctrl = match &auto_actions[1] {
            HostAction::SendPeer(p) => p.clone(),
            _ => panic!("expected sealed control_permission"),
        };
        let (auto_ctrl_plain, _) = viewer_e2e_2.cipher_mut().unwrap().open(&auto_ctrl).unwrap();
        assert_eq!(auto_ctrl_plain, encode_host_control_permission(true));

        let early_plaintext_acts = auto_core.handle_packet(b"8|||512|||0|||10|||20", now);
        assert!(early_plaintext_acts.is_empty());
        assert_eq!(auto_core.phase, HostPhase::Starting);

        let sealed_17 = viewer_e2e_2.cipher_mut().unwrap().seal(b"17", false);
        let start_acts = auto_core.handle_packet(&sealed_17, now);
        assert_eq!(auto_core.phase, HostPhase::Streaming);
        assert_eq!(start_acts, vec![HostAction::RequestCapture]);

        let raw_pixels = vec![0x1fu8; 4 * 4 * 2];
        let frame_acts = auto_core.send_frame(&raw_pixels).unwrap();
        let sealed_frame = match &frame_acts[0] {
            HostAction::SendPeer(p) => p.clone(),
            _ => panic!("expected sealed frame"),
        };
        assert!(crate::e2e::is_e2e_frame(&sealed_frame));
        let (plain_frame, _) = viewer_e2e_2
            .cipher_mut()
            .unwrap()
            .open(&sealed_frame)
            .unwrap();
        let frame_fields = decode_byte_array_fields_checked(&plain_frame, Some(11)).unwrap();
        assert_eq!(frame_fields[0], vec![1]);

        let sealed_18 = viewer_e2e_2.cipher_mut().unwrap().seal(b"18", false);
        let ack_acts = auto_core.handle_packet(&sealed_18, now);
        assert_eq!(ack_acts, vec![HostAction::RequestCapture]);

        let mouse_raw = b"8|||512|||0|||2|||2";
        let sealed_mouse = viewer_e2e_2.cipher_mut().unwrap().seal(mouse_raw, false);
        let mouse_acts = auto_core.handle_packet(&sealed_mouse, now);
        assert_eq!(
            mouse_acts,
            vec![HostAction::InjectInput(mouse_raw.to_vec())]
        );

        let sealed_chat = viewer_e2e_2
            .cipher_mut()
            .unwrap()
            .seal(b"12|||encrypted hello", false);
        let chat_acts = auto_core.handle_packet(&sealed_chat, now);
        assert_eq!(
            chat_acts,
            vec![HostAction::ChatReceived("encrypted hello".to_string())]
        );

        let (clip_pkt, clip_unicode) =
            crate::clipboard::encode_clipboard_for_remote("secret clip").unwrap();
        let sealed_clip = viewer_e2e_2
            .cipher_mut()
            .unwrap()
            .seal(&clip_pkt, clip_unicode);
        let clip_acts = auto_core.handle_packet(&sealed_clip, now);
        assert_eq!(
            clip_acts,
            vec![HostAction::ClipboardReceived("secret clip".to_string())]
        );

        let legacy_nonce = "99998888";
        let legacy_proof = md5_hex_lower(format!("{legacy_nonce}{temp_pw}").as_bytes());
        let (legacy_offer, _) = crate::e2e::create_viewer_offer(legacy_nonce, &legacy_proof, "");
        let legacy_login = format!(
            "login:{legacy_nonce}:{legacy_proof}:1920:1080:7:15:15:30:16:111222333:Legacy:1:{}:x:conn:2:45:0:0:{legacy_offer}:6.6.133:0",
            "0".repeat(32)
        )
        .into_bytes();
        let mut legacy_core = HostCore::new(temp_pw, None, layout);
        legacy_core.auto_approve = true;
        let legacy_actions = legacy_core.handle_packet(&legacy_login, now);
        assert_eq!(legacy_core.phase, HostPhase::Starting);
        assert!(!legacy_core.e2e.is_enabled());
        let screen_init = match &legacy_actions[0] {
            HostAction::SendPeer(p) => p.clone(),
            _ => panic!("expected screen_init"),
        };
        let legacy_init_fields = decode_byte_array_fields_checked(&screen_init, Some(11)).unwrap();
        assert!(legacy_init_fields[3].is_empty());
    }

    #[test]
    fn test_macos_png_to_rgb555_and_command_injector() {
        let png_bytes = crate::server::DEFAULT_APP_ICON_PNG;
        let (w, h, rgb555) = parse_png_to_rgb555_top_down(png_bytes).unwrap();
        assert!(w > 0 && h > 0);
        assert_eq!(rgb555.len(), (w as usize) * (h as usize) * 2);

        let sub = MonitorRect {
            left: 0,
            top: 0,
            right: 4,
            bottom: 4,
        };
        let cropped = crop_or_resize_rgb555(&rgb555, w as usize, h as usize, sub);
        assert_eq!(cropped.len(), 4 * 4 * 2);

        let mut inj = MacOsCommandInjector::new(false);
        inj.inject_event(b"8|||512|||0|||10|||20").unwrap();
        inj.release_all();
        let recorded = inj.injected_packets.lock().unwrap().clone();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0], b"8|||512|||0|||10|||20".to_vec());
        assert_eq!(recorded[1], b"RELEASE_ALL".to_vec());

        assert_eq!(vk_to_macos_keycode(0x41), Some(0x00));
        assert_eq!(vk_to_macos_keycode(0x0d), Some(0x24));
        assert_eq!(vk_to_macos_keycode(0x1b), Some(0x35));
        assert_eq!(vk_to_macos_keycode(0x5b), Some(0x37));
        let _ = ensure_macos_host_permissions(false);

        let bgra_row = [
            0u8, 128u8, 255u8, 255u8, 255u8, 0u8, 0u8, 255u8, 99u8, 99u8, 99u8, 99u8,
        ];
        let rgb555_bgra = convert_cg_rgba_rows_to_rgb555(&bgra_row, 2, 1, 12, 0x2002).unwrap();
        assert_eq!(rgb555_bgra.len(), 4);
        let p0 = u16::from_le_bytes([rgb555_bgra[0], rgb555_bgra[1]]);
        let p1 = u16::from_le_bytes([rgb555_bgra[2], rgb555_bgra[3]]);
        assert_eq!(p0, ((31u16) << 10) | ((16u16) << 5));
        assert_eq!(p1, 31u16);

        let argb_row = [255u8, 255u8, 128u8, 0u8, 255u8, 0u8, 0u8, 255u8];
        let rgb555_argb = convert_cg_rgba_rows_to_rgb555(&argb_row, 2, 1, 8, 0x4002).unwrap();
        assert_eq!(rgb555_argb, rgb555_bgra);

        let perms = MacOsPermissionState::preflight();
        let req_screen = MacOsPermissionState::request(MacOsPermissionKind::ScreenRecording);
        assert_eq!(perms.supported, req_screen.supported);
        assert_eq!(
            MacOsPermissionState::open_settings(MacOsPermissionKind::ScreenRecording).unwrap(),
            "Privacy_ScreenCapture"
        );
        assert_eq!(
            MacOsPermissionState::open_settings(MacOsPermissionKind::Accessibility).unwrap(),
            "Privacy_Accessibility"
        );
    }

    #[test]
    fn test_host_registration_command60_fallback_mac() {
        use crate::protocol::{
            decrypt_registration_bytes, encode_typed_transport_frame, FrameStream,
            REGISTRATION_PASSWORD,
        };
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let temp_pw = "87654321";

        let server_handle = std::thread::spawn(move || {
            for attempt in 0..2 {
                let (mut reg_stream, _) = listener.accept().unwrap();
                reg_stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                reg_stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
                reg_stream.flush().unwrap();
                let mut reg_rx = FrameStream::new(true);
                let mut buf = [0u8; 4096];

                let reg_cmd = loop {
                    let n = reg_stream.read(&mut buf).unwrap();
                    assert!(n > 0);
                    let frames = reg_rx.feed(&buf[..n]);
                    if let Some(frame) = frames.into_iter().next() {
                        if let Ok(Some(body)) = decode_transport_body(&frame) {
                            if body.kind == "direct" && body.data.first() == Some(&0) {
                                let plain = decrypt_registration_bytes(
                                    &body.data[1..],
                                    REGISTRATION_PASSWORD,
                                )
                                .unwrap();
                                break String::from_utf8(plain).unwrap();
                            }
                        }
                    }
                };
                let parts: Vec<&str> = reg_cmd.split(':').collect();
                assert_eq!(parts[0], "3");
                if attempt == 0 {
                    assert_eq!(parts[8], "021122334455");
                    let ch = encode_typed_transport_frame(b"60:challenge_token:", 1);
                    reg_stream.write_all(&ch).unwrap();
                    reg_stream.flush().unwrap();
                } else {
                    assert_eq!(parts[8], DEFAULT_FALLBACK_HOST_MAC);
                    let reply = format!(
                        "3:{}:134462379:203.0.113.7::0:127.0.0.1:-1:01/01/0001::",
                        port
                    );
                    let reply_wire = encode_typed_transport_frame(reply.as_bytes(), 1);
                    reg_stream.write_all(&reply_wire).unwrap();
                    reg_stream.flush().unwrap();
                }
            }
        });

        let layout = DisplayLayout {
            width: 8,
            height: 8,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![MonitorRect {
                left: 0,
                top: 0,
                right: 8,
                bottom: 8,
            }],
        };
        let backend = DesktopBackend::headless(8, 8);
        let session = HostSession::with_profile(
            "FEDCBA9876543210FEDCBA9876543210",
            "",
            "021122334455",
            temp_pw,
            None,
            "MacHost",
            layout,
            backend,
        );
        let st = session.start_with_endpoint("127.0.0.1", port, false);
        assert!(st.running);
        assert_eq!(st.host_id, "134462379");
        assert_eq!(st.hardware_mac, DEFAULT_FALLBACK_HOST_MAC);

        server_handle.join().unwrap();
        let _ = session.stop();
    }

    #[test]
    fn test_host_core_remote_file_offer_and_streaming() {
        let layout = DisplayLayout {
            width: 4,
            height: 4,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![MonitorRect {
                left: 0,
                top: 0,
                right: 4,
                bottom: 4,
            }],
        };
        let mut core = HostCore::new("temp_pass", None, layout);
        let temp_dir =
            std::env::temp_dir().join(format!("ultradesk_host_ft_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        core.files.download_dir = temp_dir.clone();
        let now = Instant::now();

        let valid_payload = sample_login_payload("temp_pass", "2", 1);
        let _ = core.handle_packet(&valid_payload, now);
        let _ = core.approve(true);
        let _ = core.handle_packet(b"17", now);
        assert_eq!(core.phase, HostPhase::Streaming);

        let mut sender = crate::file_transfer::FileTransferManager::new(temp_dir.clone());
        let payload_bytes = b"hello from remote controller to host".to_vec();
        let (file_id, offer_pkt) = sender
            .offer_file_bytes("remote_note.txt", payload_bytes.clone())
            .unwrap();

        let actions = core.handle_packet(&offer_pkt, now);
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            HostAction::FileTransferEvent(crate::file_transfer::FileTransferPacket::Offer { .. })
        ));
        let accept_pkt = core.files.accept_incoming_offer(&file_id).unwrap();

        let (_, chunk_replies) = sender.handle_packet(&accept_pkt).unwrap().unwrap();
        assert!(!chunk_replies.is_empty());
        for chunk_pkt in chunk_replies {
            let chunk_actions = core.handle_packet(&chunk_pkt, now);
            assert!(!chunk_actions.is_empty());
        }

        let entry = core.files.transfers.get(&file_id).unwrap();
        assert_eq!(
            entry.status,
            crate::file_transfer::TransferStatus::Completed
        );
        assert_eq!(
            entry.direction,
            crate::file_transfer::TransferDirection::Receive
        );
        assert!(entry.buffer.is_empty());
        let saved_path = entry.saved_path.as_ref().unwrap();
        assert_eq!(std::fs::read(saved_path).unwrap(), payload_bytes);
        assert_eq!(
            core.files.read_completed_download(&file_id).unwrap().1,
            payload_bytes
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_host_core_rejects_file_offer_when_control_disallowed() {
        let layout = DisplayLayout {
            width: 4,
            height: 4,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![MonitorRect {
                left: 0,
                top: 0,
                right: 4,
                bottom: 4,
            }],
        };
        let mut core = HostCore::new("temp_pass", None, layout);
        let now = Instant::now();
        let valid_payload = sample_login_payload("temp_pass", "2", 1);
        let _ = core.handle_packet(&valid_payload, now);
        let _ = core.approve(false);
        let _ = core.handle_packet(b"17", now);
        assert_eq!(core.phase, HostPhase::Streaming);
        assert!(!core.control_allowed);

        let offer_pkt =
            crate::file_transfer::encode_file_offer("uvf_99", "blocked.txt", 5, true).unwrap();
        let actions = core.handle_packet(&offer_pkt, now);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            HostAction::SendPeer(pkt) => {
                let decoded = crate::file_transfer::decode_file_transfer_packet(pkt).unwrap();
                assert_eq!(
                    decoded,
                    crate::file_transfer::FileTransferPacket::Cancel {
                        file_id: "uvf_99".to_string()
                    }
                );
            }
            other => panic!("expected SendPeer cancel, got {:?}", other),
        }
        assert!(core.files.transfers.is_empty());
    }

    #[test]
    fn test_bidirectional_remote_file_request_and_download() {
        let layout = DisplayLayout {
            width: 4,
            height: 4,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![MonitorRect {
                left: 0,
                top: 0,
                right: 4,
                bottom: 4,
            }],
        };
        let mut core = HostCore::new("temp_pass", None, layout);
        let temp_dir =
            std::env::temp_dir().join(format!("ultradesk_host_pull_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        core.files.download_dir = temp_dir.clone();
        core.files.allow_remote_requests = true;
        let remote_src = temp_dir.join("remote_export.txt");
        let expected_bytes = b"pulled file content from remote host".to_vec();
        std::fs::write(&remote_src, &expected_bytes).unwrap();

        let now = Instant::now();
        let valid_payload = sample_login_payload("temp_pass", "2", 1);
        let _ = core.handle_packet(&valid_payload, now);
        let _ = core.approve(true);
        let _ = core.handle_packet(b"17", now);

        let mut controller_files =
            crate::file_transfer::FileTransferManager::new(temp_dir.join("controller_dl"));
        assert!(!controller_files.auto_accept);
        controller_files.record_requested_remote_path(&remote_src.to_string_lossy());

        let req_pkt =
            crate::file_transfer::encode_file_request(&remote_src.to_string_lossy(), true).unwrap();
        let host_actions = core.handle_packet(&req_pkt, now);
        assert_eq!(host_actions.len(), 2);
        let offer_pkt = match &host_actions[0] {
            HostAction::SendPeer(p) => p.clone(),
            other => panic!("expected SendPeer offer, got {:?}", other),
        };

        let (_, ctrl_replies) = controller_files.handle_packet(&offer_pkt).unwrap().unwrap();
        assert_eq!(ctrl_replies.len(), 1);
        let accept_pkt = &ctrl_replies[0];

        let chunk_actions = core.handle_packet(accept_pkt, now);
        for act in chunk_actions {
            if let HostAction::SendPeer(chunk_pkt) = act {
                let _ = controller_files.handle_packet(&chunk_pkt).unwrap();
            }
        }

        let completed_id = controller_files
            .transfers
            .values()
            .find(|e| e.file_name == "remote_export.txt")
            .unwrap()
            .file_id
            .clone();
        let completed = controller_files.transfers.get(&completed_id).unwrap();
        assert_eq!(
            completed.status,
            crate::file_transfer::TransferStatus::Completed
        );
        assert!(completed.buffer.is_empty());
        assert!(completed.saved_path.is_some());
        assert_eq!(
            controller_files
                .read_completed_download(&completed_id)
                .unwrap()
                .1,
            expected_bytes
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_peer_throttles_saturation_preserves_locked_entries_and_ipv6_bucket() {
        assert_eq!(peer_throttle_key("192.0.2.1".parse().unwrap()), "192.0.2.1");
        assert_eq!(
            peer_throttle_key("::ffff:192.0.2.1".parse().unwrap()),
            "192.0.2.1"
        );
        assert_eq!(peer_throttle_key("::1".parse().unwrap()), "::1");
        assert_eq!(
            peer_throttle_key("2001:db8:abcd:1234::1".parse().unwrap()),
            "v6:2001:db8:abcd:1234::/64"
        );
        assert_eq!(
            peer_throttle_key("2001:db8:abcd:1234::ffff".parse().unwrap()),
            "v6:2001:db8:abcd:1234::/64"
        );

        let layout = DisplayLayout {
            width: 4,
            height: 4,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![MonitorRect {
                left: 0,
                top: 0,
                right: 4,
                bottom: 4,
            }],
        };
        let session =
            HostSession::new("goodpassword", None, layout, DesktopBackend::headless(4, 4));
        let attacker_ip = "203.0.113.66";
        let bad_login = b"login:00001111:00000000000000000000000000000000:1920:1080:7:15:15:30:16:999:Attacker:1:00000000000000000000000000000000:x:conn:2:45:0:0::6.6.133:0";

        for _ in 0..5 {
            let _ = session.handle_peer_payload_from_peer(attacker_ip, bad_login);
        }
        {
            let s = session.inner.lock().unwrap();
            assert!(s
                .peer_throttles
                .get(attacker_ip)
                .unwrap()
                .is_locked(Instant::now()));
        }

        for i in 0..300 {
            let key = format!("198.51.100.{}", i);
            let _ = session.handle_peer_payload_from_peer(&key, bad_login);
        }

        {
            let s = session.inner.lock().unwrap();
            let throttle = s.peer_throttles.get(attacker_ip);
            assert!(
                throttle.is_some(),
                "attacker throttle should not be evicted"
            );
            assert!(
                throttle.unwrap().is_locked(Instant::now()),
                "attacker must remain locked"
            );
        }
    }

    #[test]
    fn test_idle_preauth_lan_sockets_do_not_block_legitimate_viewer() {
        let layout = DisplayLayout {
            width: 4,
            height: 4,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![MonitorRect {
                left: 0,
                top: 0,
                right: 4,
                bottom: 4,
            }],
        };
        let host = HostSession::new("pass1234", None, layout, DesktopBackend::headless(4, 4));
        host.set_auto_approve(true);
        let status = host.start_with_endpoint("127.0.0.1", 1, false);
        let port = status.lan_port;
        assert!(port > 0);

        let mut idle_sockets = Vec::new();
        for _ in 0..4 {
            let s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.set_nonblocking(true).unwrap();
            idle_sockets.push(s);
        }

        let start = Instant::now();
        let mut viewer = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        viewer
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let login_payload = sample_login_payload("pass1234", "2", 1);
        let utf16le =
            |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect() };
        let frame = encode_relay_frame(
            &utf16le(&String::from_utf8(login_payload).unwrap()),
            "127.0.0.1",
            12345,
            1,
        )
        .unwrap();
        viewer.write_all(&frame).unwrap();

        let mut rx = FrameStream::new(false);
        let mut buf = [0u8; 4096];
        let mut authenticated = false;
        while start.elapsed() < Duration::from_millis(500) {
            match viewer.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    for f in rx.feed(&buf[..n]) {
                        if let Ok(Some(body)) = decode_transport_body(&f) {
                            if body.kind == "relay" {
                                authenticated = true;
                                break;
                            }
                        }
                    }
                    if authenticated {
                        break;
                    }
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break,
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let elapsed = start.elapsed();
        host.stop();
        drop(idle_sockets);
        drop(viewer);

        assert!(authenticated, "legitimate viewer should have authenticated");
        assert!(
            elapsed < Duration::from_millis(500),
            "authentication took {:?}, expected < 500ms",
            elapsed
        );
    }

    #[test]
    fn test_auto_approve_master_false_by_default_prompts_approval() {
        let layout = DisplayLayout {
            width: 4,
            height: 4,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![MonitorRect {
                left: 0,
                top: 0,
                right: 4,
                bottom: 4,
            }],
        };
        let master_proof = compute_master_password_proof("mastersecret");
        let mut core = HostCore::new("temppass", Some(master_proof.clone()), layout);
        assert!(!core.auto_approve_master);

        let nonce = "12345678";
        let master_login_payload = format!(
            "login:{nonce}:wrongproof:1920:1080:7:15:15:30:16:111222333:Master Viewer:1:{}:x:conn:2:45:0:0::6.6.133:0",
            master_proof
        );
        let actions = core.handle_packet(master_login_payload.as_bytes(), Instant::now());
        assert_eq!(core.phase, HostPhase::Approval);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], HostAction::PromptApproval(_)));
    }

    #[test]
    fn test_relay_ip_and_host_validation() {
        let good_legacy = "11:103.56.158.44:unused:relay1.ultraviewer.net";
        assert!(parse_relay_notification(good_legacy).is_some());

        let bad_ips = ["127.0.0.1", "10.0.0.1", "169.254.169.254", "100.64.0.1"];
        for bad_ip in bad_ips {
            let legacy = format!("11:{}:unused:relay1.ultraviewer.net", bad_ip);
            assert!(
                parse_relay_notification(&legacy).is_none(),
                "expected {} to be rejected in legacy",
                bad_ip
            );

            let modern = format!("17:tag:hash:{}:relay1.ultraviewer.net:0:-1:60:0:", bad_ip);
            assert!(
                parse_relay_notification(&modern).is_none(),
                "expected {} to be rejected in modern",
                bad_ip
            );
        }

        assert!(parse_relay_notification_with_policy(
            "11:127.0.0.1:unused:localhost",
            "",
            "",
            true
        )
        .is_some());
        assert!(parse_relay_notification_with_policy(
            "11:127.0.0.1:unused:localhost",
            "",
            "",
            false
        )
        .is_none());
        assert!(parse_relay_notification("11:103.56.158.44:unused:localhost").is_none());

        let bad_host = "11:103.56.158.44:unused:evil..com";
        assert!(parse_relay_notification(bad_host).is_none());

        let modern_bad_host = "17:tag:hash:103.56.158.44:evil_host/path:0:-1:60:0:";
        assert!(parse_relay_notification(modern_bad_host).is_none());

        assert!(parse_relay_address(b"0:12345:0.0.0.0").is_none());
        assert!(parse_relay_address(b"0:12345:255.255.255.255").is_none());
        assert!(parse_relay_address(b"0:12345:224.0.0.1").is_none());
        let cgnat_addr = b"0:12345:100.64.1.2";
        assert_eq!(
            parse_relay_address(cgnat_addr),
            Some((12345, "100.64.1.2".to_string()))
        );
    }
}

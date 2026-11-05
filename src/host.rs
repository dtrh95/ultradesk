use crate::input::{decode_host_input, HostInputEvent, KEY_DOWN, SYS_KEY_DOWN, VK_PACKET};
use crate::protocol::{encode_byte_array_fields, md5_hex_lower};
use crate::screen::{compress_rgb555_with_dict, DisplayLayout, MonitorRect};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;
use std::sync::{Arc, Mutex};
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
pub const MAX_FAILURES_BEFORE_LOCKOUT: u32 = 5;
pub const LOCKOUT_DURATION: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayNotification {
    pub tag: Option<String>,
    pub hash: Option<String>,
    pub relay_ip: String,
    pub relay_host: String,
}

pub fn parse_relay_notification(text: &str) -> Option<RelayNotification> {
    let parts: Vec<&str> = text.split(':').collect();
    if parts.is_empty() {
        return None;
    }
    if parts[0] == "11" && parts.len() >= 4 {
        return Some(RelayNotification {
            tag: None,
            hash: None,
            relay_ip: parts[1].to_string(),
            relay_host: parts[3].to_string(),
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
        let relay_ip = parts[3].split(';').next().unwrap_or("").to_string();
        let relay_host = parts[4].split(';').next().unwrap_or("").to_string();
        if relay_ip.is_empty() {
            return None;
        }
        return Some(RelayNotification {
            tag,
            hash,
            relay_ip,
            relay_host: if relay_host.is_empty() {
                "functions3.ultraviewer.net".to_string()
            } else {
                relay_host
            },
        });
    }
    None
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
    let client_version = fields[21].to_string();

    Some(ViewerLogin {
        nonce,
        password_proof,
        viewer_width,
        viewer_height,
        claimed_id,
        display_name,
        wants_control: control_mode == 1,
        custom_password_proof,
        color_mode,
        client_version,
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

#[derive(Debug, Clone)]
pub struct LoginThrottle {
    failures: u32,
    locked_until: Option<Instant>,
}

impl Default for LoginThrottle {
    fn default() -> Self {
        Self {
            failures: 0,
            locked_until: None,
        }
    }
}

impl LoginThrottle {
    pub fn is_locked(&self, now: Instant) -> bool {
        self.locked_until.is_some_and(|until| now < until)
    }

    pub fn record_failure(&mut self, now: Instant) -> bool {
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
    }

    pub fn remaining_lockout(&self, now: Instant) -> Option<Duration> {
        self.locked_until
            .and_then(|until| until.checked_duration_since(now))
    }
}

pub fn encode_host_screen_init(layout: &DisplayLayout) -> Vec<u8> {
    let width_bytes = (layout.width as i32).to_le_bytes();
    let height_bytes = (layout.height as i32).to_le_bytes();
    let num_monitors = [layout.monitors.len().max(1) as u8];
    let monitor_bounds = if layout.monitors.is_empty() {
        format!(
            "{}:{}:{}:{}|\0",
            layout.virtual_left,
            layout.virtual_top,
            layout.virtual_left + layout.width as i32,
            layout.virtual_top + layout.height as i32
        )
    } else {
        let mut s = String::new();
        for m in &layout.monitors {
            s.push_str(&format!("{}:{}:{}:{}|", m.left, m.top, m.right, m.bottom));
        }
        s.push('\0');
        s
    };
    let vl_bytes = layout.virtual_left.to_le_bytes();
    let vt_bytes = layout.virtual_top.to_le_bytes();
    let fields: [&[u8]; 11] = [
        &[0],
        &width_bytes,
        &height_bytes,
        &[],
        &[],
        &[0],
        HOST_VERSION,
        &num_monitors,
        monitor_bounds.as_bytes(),
        &vl_bytes,
        &vt_bytes,
    ];
    encode_byte_array_fields(&fields).unwrap()
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

pub fn encode_host_frame(
    rect: (u32, u32, u32, u32),
    rgb555_top_down: &[u8],
    seq: i32,
) -> Result<Vec<u8>, String> {
    let compressed = compress_rgb555_with_dict(rgb555_top_down, 1)?;
    let mut rect_bytes = Vec::with_capacity(16);
    rect_bytes.extend_from_slice(&(rect.0 as i32).to_le_bytes());
    rect_bytes.extend_from_slice(&(rect.1 as i32).to_le_bytes());
    rect_bytes.extend_from_slice(&(rect.2 as i32).to_le_bytes());
    rect_bytes.extend_from_slice(&(rect.3 as i32).to_le_bytes());

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
    SessionEnded(String),
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
}

impl HostCore {
    pub fn new(
        temp_password: &str,
        master_password_proof: Option<String>,
        layout: DisplayLayout,
    ) -> Self {
        Self {
            phase: HostPhase::None,
            temp_password: temp_password.to_string(),
            master_password_proof,
            auto_approve: false,
            auto_approve_master: true,
            throttle: LoginThrottle::default(),
            active_viewer: None,
            control_allowed: false,
            waiting_ack: false,
            next_frame_seq: FIRST_FRAME_NUMBER,
            layout,
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

            if self.auto_approve || (is_master && self.auto_approve_master) {
                self.active_viewer = Some(login.clone());
                self.phase = HostPhase::Starting;
                self.control_allowed = login.wants_control;
                self.next_frame_seq = FIRST_FRAME_NUMBER;
                self.waiting_ack = false;
                actions.push(HostAction::SendPeer(encode_host_screen_init(&self.layout)));
                actions.push(HostAction::SendPeer(encode_host_control_permission(
                    self.control_allowed,
                )));
                actions.push(HostAction::SessionStarted(login));
            } else {
                self.active_viewer = Some(login.clone());
                self.phase = HostPhase::Approval;
                actions.push(HostAction::PromptApproval(login));
            }
            return actions;
        }

        if payload == b"17" && self.phase == HostPhase::Starting {
            self.phase = HostPhase::Streaming;
            self.waiting_ack = false;
            actions.push(HostAction::RequestCapture);
            return actions;
        }

        if payload == b"18" && self.phase == HostPhase::Streaming && self.waiting_ack {
            self.waiting_ack = false;
            actions.push(HostAction::RequestCapture);
            return actions;
        }

        if payload == b"19" {
            self.phase = HostPhase::None;
            self.active_viewer = None;
            self.waiting_ack = false;
            actions.push(HostAction::ReleaseAllInput);
            actions.push(HostAction::SessionEnded(
                "Viewer disconnected".to_string(),
            ));
            return actions;
        }

        if payload == b"20" {
            actions.push(HostAction::SendPeer(encode_host_keepalive_reply()));
            return actions;
        }

        if payload.starts_with(b"8|||") || payload.starts_with(&[30, b'|', b'|', b'|']) {
            if self.phase == HostPhase::Streaming && self.control_allowed {
                actions.push(HostAction::InjectInput(payload.to_vec()));
            }
            return actions;
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
        actions.push(HostAction::SendPeer(encode_host_screen_init(&self.layout)));
        actions.push(HostAction::SendPeer(encode_host_control_permission(
            allow_control,
        )));
        if let Some(ref v) = self.active_viewer {
            actions.push(HostAction::SessionStarted(v.clone()));
        }
        actions
    }

    pub fn decline(&mut self) -> Vec<HostAction> {
        let mut actions = Vec::new();
        self.phase = HostPhase::None;
        self.active_viewer = None;
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
        actions.push(HostAction::SendPeer(encode_host_control_permission(allowed)));
        actions
    }

    pub fn on_frame_acknowledged(&mut self) -> Vec<HostAction> {
        let mut actions = Vec::new();
        if self.phase == HostPhase::Streaming && self.waiting_ack {
            self.waiting_ack = false;
            actions.push(HostAction::RequestCapture);
        }
        actions
    }

    pub fn send_frame(&mut self, rgb555_top_down: &[u8]) -> Result<Vec<HostAction>, String> {
        if self.phase != HostPhase::Streaming || self.waiting_ack {
            return Ok(Vec::new());
        }
        let seq = self.next_frame_seq;
        self.next_frame_seq = self.next_frame_seq.wrapping_add(1);
        self.waiting_ack = true;
        let packet = encode_host_frame(
            (0, 0, self.layout.width, self.layout.height),
            rgb555_top_down,
            seq,
        )?;
        Ok(vec![HostAction::SendPeer(packet)])
    }

    pub fn end_session(&mut self, reason: &str) -> Vec<HostAction> {
        let mut actions = Vec::new();
        if self.active_viewer.is_some() {
            actions.push(HostAction::SendPeer(encode_host_reject()));
        }
        self.phase = HostPhase::None;
        self.active_viewer = None;
        self.waiting_ack = false;
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
}

impl HeadlessCapturer {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
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
        let mut raw = Vec::with_capacity(w * h * 2);
        for y in 0..h {
            for x in 0..w {
                let px = (((x % 31) << 10) | (((y / 2) % 31) << 5) | (x % 31)) as u16;
                raw.extend_from_slice(&px.to_le_bytes());
            }
        }
        Ok(raw)
    }
}

pub struct HeadlessInjector {
    pub injected_packets: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Default for HeadlessInjector {
    fn default() -> Self {
        Self {
            injected_packets: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl HeadlessInjector {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Injector for HeadlessInjector {
    fn inject_event(&mut self, packet: &[u8]) -> Result<(), String> {
        self.injected_packets.lock().unwrap().push(packet.to_vec());
        Ok(())
    }

    fn release_all(&mut self) {
        self.injected_packets
            .lock()
            .unwrap()
            .push(b"RELEASE_ALL".to_vec());
    }
}

pub struct X11CommandInjector {
    pub injected_packets: Arc<Mutex<Vec<Vec<u8>>>>,
    pub use_xdotool: bool,
}

impl X11CommandInjector {
    pub fn new(use_xdotool: bool) -> Self {
        Self {
            injected_packets: Arc::new(Mutex::new(Vec::new())),
            use_xdotool,
        }
    }
}

impl Injector for X11CommandInjector {
    fn inject_event(&mut self, packet: &[u8]) -> Result<(), String> {
        self.injected_packets.lock().unwrap().push(packet.to_vec());
        if !self.use_xdotool {
            return Ok(());
        }
        if let Some(evt) = decode_host_input(packet) {
            match evt {
                HostInputEvent::Mouse { msg, x, y, wheel, .. } => match msg {
                    0x0200 => {
                        let _ = Command::new("xdotool")
                            .args(["mousemove", &x.to_string(), &y.to_string()])
                            .status();
                    }
                    0x0201 => {
                        let _ = Command::new("xdotool").args(["mousedown", "1"]).status();
                    }
                    0x0202 => {
                        let _ = Command::new("xdotool").args(["mouseup", "1"]).status();
                    }
                    0x0204 => {
                        let _ = Command::new("xdotool").args(["mousedown", "3"]).status();
                    }
                    0x0205 => {
                        let _ = Command::new("xdotool").args(["mouseup", "3"]).status();
                    }
                    0x0207 => {
                        let _ = Command::new("xdotool").args(["mousedown", "2"]).status();
                    }
                    0x0208 => {
                        let _ = Command::new("xdotool").args(["mouseup", "2"]).status();
                    }
                    0x020A => {
                        let btn = if wheel >= 0 { "4" } else { "5" };
                        let _ = Command::new("xdotool").args(["click", btn]).status();
                    }
                    _ => {}
                },
                HostInputEvent::Key { kind, vk, ch, .. } => {
                    if vk == VK_PACKET && (kind == KEY_DOWN || kind == SYS_KEY_DOWN) && ch >= 0x20 {
                        if let Some(c) = char::from_u32(ch as u32) {
                            let s = c.to_string();
                            let _ = Command::new("xdotool").args(["type", "--", &s]).status();
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn release_all(&mut self) {
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
        if !std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty()) {
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
        if self.shm_supported {
            if let Some(pixels) = self.capture_shm(rect) {
                return Ok(pixels);
            }
            self.shm_supported = false;
        }
        self.capture_get_image(rect)
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
        if !std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty()) {
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
        if let (Ok(capturer), Ok(injector)) = (X11Capturer::connect(), X11Injector::connect()) {
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
}

struct HostSessionInner {
    running: bool,
    host_id: String,
    hardware_mac: String,
    core: HostCore,
    backend: DesktopBackend,
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
        Self::with_id("", "", temp_password, master_password_proof, layout, backend)
    }

    pub fn with_id(
        host_id: &str,
        hardware_mac: &str,
        temp_password: &str,
        master_password_proof: Option<String>,
        layout: DisplayLayout,
        backend: DesktopBackend,
    ) -> Self {
        let core = HostCore::new(temp_password, master_password_proof, layout);
        Self {
            inner: Arc::new(Mutex::new(HostSessionInner {
                running: false,
                host_id: host_id.to_string(),
                hardware_mac: hardware_mac.to_string(),
                core,
                backend,
            })),
        }
    }

    pub fn start(&self) -> HostStatus {
        {
            let mut s = self.inner.lock().unwrap();
            s.running = true;
        }
        self.status()
    }

    pub fn stop(&self) -> Vec<HostAction> {
        let mut s = self.inner.lock().unwrap();
        s.running = false;
        let actions = s.core.end_session("Host stopped");
        for act in &actions {
            if matches!(act, HostAction::ReleaseAllInput) {
                s.backend.injector.release_all();
            }
        }
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

    pub fn status(&self) -> HostStatus {
        let s = self.inner.lock().unwrap();
        let now = Instant::now();
        HostStatus {
            running: s.running,
            host_id: s.host_id.clone(),
            temp_password: s.core.temp_password.clone(),
            hardware_mac: s.hardware_mac.clone(),
            has_master_password: s.core.master_password_proof.is_some(),
            auto_approve: s.core.auto_approve,
            phase: s.core.phase,
            viewer_id: s.core.active_viewer.as_ref().map(|v| v.claimed_id.clone()),
            viewer_name: s.core.active_viewer.as_ref().map(|v| v.display_name.clone()),
            control_allowed: s.core.control_allowed,
            waiting_ack: s.core.waiting_ack,
            frame_seq: s.core.next_frame_seq,
            is_locked: s.core.throttle.is_locked(now),
            backend_name: s.backend.name.to_string(),
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
        let mut s = self.inner.lock().unwrap();
        Ok(s.core.approve(allow_control))
    }

    pub fn decline(&self) -> Result<Vec<HostAction>, String> {
        let mut s = self.inner.lock().unwrap();
        Ok(s.core.decline())
    }

    pub fn set_control_allowed(&self, allowed: bool) -> Result<Vec<HostAction>, String> {
        let mut s = self.inner.lock().unwrap();
        let actions = s.core.set_control_allowed(allowed);
        for act in &actions {
            if matches!(act, HostAction::ReleaseAllInput) {
                s.backend.injector.release_all();
            }
        }
        Ok(actions)
    }

    pub fn handle_peer_payload(&self, bytes: &[u8]) -> Result<Vec<HostAction>, String> {
        let mut s = self.inner.lock().unwrap();
        let actions = s.core.handle_packet(bytes, Instant::now());
        for act in &actions {
            match act {
                HostAction::InjectInput(pkt) => {
                    let _ = s.backend.injector.inject_event(pkt);
                }
                HostAction::ReleaseAllInput => {
                    s.backend.injector.release_all();
                }
                _ => {}
            }
        }
        Ok(actions)
    }

    pub fn step_capture(&self) -> Result<Vec<HostAction>, String> {
        let mut s = self.inner.lock().unwrap();
        if s.core.phase != HostPhase::Streaming || s.core.waiting_ack {
            return Ok(Vec::new());
        }
        let rect = MonitorRect {
            left: 0,
            top: 0,
            right: s.core.layout.width as i32,
            bottom: s.core.layout.height as i32,
        };
        let raw = s.backend.capturer.capture_screen(rect)?;
        s.core.send_frame(&raw)
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
        assert!(matches!(&actions[0], HostAction::PromptApproval(v) if v.claimed_id == "111222333"));

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
            (evdev_keycode(0x1e), evdev_keycode(0xe048), evdev_keycode(0xe0ff)),
            (Some(0x1e), Some(103), None)
        );
    }
}

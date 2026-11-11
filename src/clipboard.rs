use crate::protocol::{
    decode_byte_array_fields, decode_utf16le_lossy, encode_byte_array_fields,
    encode_senddata_string, parse_command_fields,
};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::process::{Command, Stdio};

pub const PACKET_CLIPBOARD_SHORT: u8 = 5;
pub const PACKET_CLIPBOARD_SYNC: u8 = 33;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipboardSyncMode {
    Bidirectional,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardMessage {
    pub client_id: i32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardState {
    pub local_text: String,
    pub remote_text: String,
    pub auto_sync_system: bool,
    pub updates_sent: u64,
    pub updates_received: u64,
}

impl ClipboardState {
    pub fn new(sync_mode: ClipboardSyncMode) -> Self {
        Self {
            local_text: String::new(),
            remote_text: String::new(),
            auto_sync_system: matches!(sync_mode, ClipboardSyncMode::Bidirectional),
            updates_sent: 0,
            updates_received: 0,
        }
    }

    pub fn sync_mode(&self) -> ClipboardSyncMode {
        if self.auto_sync_system {
            ClipboardSyncMode::Bidirectional
        } else {
            ClipboardSyncMode::Off
        }
    }

    pub fn set_sync_mode(&mut self, mode: ClipboardSyncMode) {
        self.auto_sync_system = matches!(mode, ClipboardSyncMode::Bidirectional);
    }

    pub fn apply_remote_clipboard_update(&mut self, remote_text: &str, os_write_succeeded: bool) {
        self.remote_text = remote_text.to_string();
        self.updates_received += 1;
        if !remote_text.is_empty() && os_write_succeeded {
            self.local_text = remote_text.to_string();
        }
    }
}

impl Default for ClipboardState {
    fn default() -> Self {
        Self::new(ClipboardSyncMode::Bidirectional)
    }
}

pub fn encode_clipboard_for_remote(text: &str) -> Result<(Vec<u8>, bool), String> {
    let cmd = format!("5|||{}", text);
    Ok((encode_senddata_string(&cmd, true), true))
}

pub fn encode_clipboard_packet(client_id: i32, text: &str) -> Result<Vec<u8>, String> {
    let header = encode_senddata_string(&format!("|||33|||{}|||", client_id), false);
    let body = encode_senddata_string(text, true);
    encode_byte_array_fields(&[&header, &body])
}

pub fn encode_clipboard_modern_packet(client_id: i32, text: &str) -> Result<Vec<u8>, String> {
    let id_bytes = client_id.to_le_bytes();
    let body = encode_senddata_string(text, true);
    encode_byte_array_fields(&[&[PACKET_CLIPBOARD_SYNC], &id_bytes, &body])
}

pub fn decode_clipboard_packet(payload: &[u8]) -> Option<ClipboardMessage> {
    if let Ok(fields) = decode_byte_array_fields(payload) {
        let kind_opt = match fields.first().map(|f| f.as_slice()) {
            Some([b]) | Some([b, 0]) | Some([b, 0, 0, 0]) => Some(*b),
            _ => None,
        };
        if fields.len() >= 2 && kind_opt == Some(PACKET_CLIPBOARD_SHORT) {
            let text = decode_clipboard_text_bytes(&fields[1], 0);
            return Some(ClipboardMessage { client_id: 0, text });
        }
        if fields.len() >= 3 && kind_opt == Some(PACKET_CLIPBOARD_SYNC) {
            let meta = if fields[1].len() >= 4 {
                i32::from_le_bytes([fields[1][0], fields[1][1], fields[1][2], fields[1][3]])
            } else {
                0
            };
            let text = decode_clipboard_text_bytes(&fields[2], meta.max(0) as usize);
            return Some(ClipboardMessage {
                client_id: meta,
                text,
            });
        }
        if fields.len() == 2 && kind_opt == Some(PACKET_CLIPBOARD_SYNC) {
            let text = decode_clipboard_text_bytes(&fields[1], 0);
            return Some(ClipboardMessage { client_id: 0, text });
        }
        if fields.len() >= 2
            && (fields[0].starts_with(b"|||33|||")
                || fields[0].starts_with(b"33|||")
                || fields[0].starts_with(b"|||11|||")
                || fields[0].starts_with(b"11|||"))
        {
            let header_str = String::from_utf8_lossy(&fields[0]);
            let parts = parse_command_fields(&header_str);
            let meta = parts
                .get(1)
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(0);
            let text = decode_clipboard_text_bytes(&fields[1], meta.max(0) as usize);
            return Some(ClipboardMessage {
                client_id: meta,
                text,
            });
        }
        if fields.len() == 1 {
            let s = decode_clipboard_text_bytes(&fields[0], 0);
            if let Some(msg) = parse_legacy_clipboard_str(&s) {
                return Some(msg);
            }
        }
    }
    if payload.starts_with(b"33|||") || payload.starts_with(b"|||33|||") {
        let stripped = payload.strip_prefix(b"|||").unwrap_or(payload);
        if let Some(rest) = stripped.strip_prefix(b"33|||") {
            if let Some(pos) = rest.windows(3).position(|w| w == b"|||") {
                if let Ok(len_str) = std::str::from_utf8(&rest[..pos]) {
                    if let Ok(meta) = len_str.parse::<i32>() {
                        let body = &rest[pos + 3..];
                        if body.starts_with(b"\x28\xb5\x2f\xfd") {
                            let text = decode_clipboard_text_bytes(body, meta.max(0) as usize);
                            return Some(ClipboardMessage {
                                client_id: meta,
                                text,
                            });
                        }
                    }
                }
            }
        }
    }
    let s = decode_clipboard_text_bytes(payload, 0);
    if s.starts_with("|||33")
        || s.starts_with("|||5")
        || s.starts_with("33|||")
        || s.starts_with("5|||")
    {
        return parse_legacy_clipboard_str(&s);
    }
    None
}

fn decode_clipboard_text_bytes(raw: &[u8], expected_uncompressed: usize) -> String {
    if raw.is_empty() {
        return String::new();
    }
    if raw.starts_with(b"\x28\xb5\x2f\xfd") {
        let cap = expected_uncompressed
            .max(raw.len() * 8)
            .clamp(4096, 16 * 1024 * 1024);
        if let Ok(decompressed) = zstd::bulk::decompress(raw, cap) {
            return decode_utf16le_lossy(&decompressed);
        }
    }
    if raw.len().is_multiple_of(2) && (raw.contains(&0) || std::str::from_utf8(raw).is_err()) {
        return decode_utf16le_lossy(raw);
    }
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(_) => decode_utf16le_lossy(raw),
    }
}

fn parse_legacy_clipboard_str(text: &str) -> Option<ClipboardMessage> {
    if let Some(rest) = text.strip_prefix("5|||") {
        return Some(ClipboardMessage {
            client_id: 0,
            text: rest.to_string(),
        });
    }
    if let Some(rest) = text.strip_prefix("|||5") {
        let clean = rest.strip_prefix("|||").unwrap_or(rest);
        return Some(ClipboardMessage {
            client_id: 0,
            text: clean.to_string(),
        });
    }
    let parts = parse_command_fields(text);
    if parts.first().map(|s| s.as_str()) != Some("33") {
        return None;
    }
    if parts.len() >= 3 {
        if let Ok(client_id) = parts[1].parse::<i32>() {
            return Some(ClipboardMessage {
                client_id,
                text: parts[2..].join("|||"),
            });
        }
    }
    if parts.len() >= 2 {
        return Some(ClipboardMessage {
            client_id: 0,
            text: parts[1..].join("|||"),
        });
    }
    None
}

fn run_clipboard_write(bin: &str, args: &[&str], text: &str) -> bool {
    if let Ok(mut child) = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        if let Ok(status) = child.wait() {
            return status.success();
        }
    }
    false
}

pub fn clipboard_text_eq(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let norm_a = a.replace("\r\n", "\n");
    let norm_b = b.replace("\r\n", "\n");
    norm_a.trim_end_matches(&['\r', '\n'][..]) == norm_b.trim_end_matches(&['\r', '\n'][..])
}

const GTK_CLIPBOARD_SET_PY: &str = "import sys, gi; gi.require_version('Gtk', '3.0'); gi.require_version('Gdk', '3.0'); from gi.repository import Gtk, Gdk; Gtk.init_check(); c = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD); t = sys.stdin.read(); c.set_text(t, -1); c.store()";
const GTK_CLIPBOARD_GET_PY: &str = "import sys, gi; gi.require_version('Gtk', '3.0'); gi.require_version('Gdk', '3.0'); from gi.repository import Gtk, Gdk; Gtk.init_check(); c = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD); t = c.wait_for_text(); sys.stdout.write(t if t is not None else '')";

pub fn is_system_clipboard_disabled() -> bool {
    cfg!(test)
        || std::env::var_os("ULTRADESK_DISABLE_SYSTEM_CLIPBOARD")
            .is_some_and(|v| v == "1" || v.to_str().is_some_and(|s| s.eq_ignore_ascii_case("true")))
}

pub fn select_wayland_paste_mime_type(types_output: &str) -> Option<&'static str> {
    const PREFERRED_PLAIN_TYPES: &[&str] = &[
        "text/plain;charset=utf-8",
        "text/plain; charset=utf-8",
        "text/plain;charset=UTF-8",
        "text/plain",
        "UTF8_STRING",
        "TEXT",
        "STRING",
    ];
    let offered: Vec<&str> = types_output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    PREFERRED_PLAIN_TYPES
        .iter()
        .copied()
        .find(|preferred| offered.contains(preferred))
}

pub fn select_linux_clipboard_write_backends(
    has_wayland: bool,
    has_x11: bool,
) -> Vec<(&'static str, &'static [&'static str])> {
    let mut out = Vec::new();
    if has_wayland {
        out.push(("wl-copy", &[][..]));
    }
    if has_x11 {
        out.push((
            "xclip",
            &["-selection", "clipboard", "-t", "UTF8_STRING"][..],
        ));
        out.push(("xsel", &["--clipboard", "--input"][..]));
        out.push(("python3", &["-c", GTK_CLIPBOARD_SET_PY][..]));
    }
    out
}

fn has_graphical_display() -> bool {
    cfg!(target_os = "macos")
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
}

pub fn set_linux_system_clipboard(text: &str) -> bool {
    if is_system_clipboard_disabled() || !has_graphical_display() {
        return false;
    }
    if cfg!(target_os = "macos") {
        return run_clipboard_write("pbcopy", &[], text);
    }
    let has_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    let has_x11 = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty());
    for (bin, args) in select_linux_clipboard_write_backends(has_wayland, has_x11) {
        if run_clipboard_write(bin, args, text) {
            return true;
        }
    }
    false
}

pub fn get_linux_system_clipboard() -> Option<String> {
    if is_system_clipboard_disabled() || !has_graphical_display() {
        return None;
    }
    if cfg!(target_os = "macos") {
        let output = Command::new("pbpaste").output().ok()?;
        if output.status.success() {
            return Some(String::from_utf8_lossy(&output.stdout).to_string());
        }
        return None;
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()) {
        if let Ok(list_out) = Command::new("wl-paste").arg("--list-types").output() {
            if list_out.status.success() {
                let types_str = String::from_utf8_lossy(&list_out.stdout);
                let Some(mime) = select_wayland_paste_mime_type(&types_str) else {
                    return Some(String::new());
                };
                if let Ok(paste_out) = Command::new("wl-paste")
                    .args(["--no-newline", "--type", mime])
                    .output()
                {
                    if paste_out.status.success() {
                        return Some(String::from_utf8_lossy(&paste_out.stdout).to_string());
                    }
                }
                return Some(String::new());
            }
            let stderr_str = String::from_utf8_lossy(&list_out.stderr);
            if stderr_str.contains("Nothing is copied") {
                return Some(String::new());
            }
        }
    }
    let candidates: &[(&str, &[&str])] = &[
        (
            "xclip",
            &["-selection", "clipboard", "-t", "UTF8_STRING", "-o"],
        ),
        ("xclip", &["-selection", "clipboard", "-o"]),
        ("xsel", &["--clipboard", "--output"]),
        ("python3", &["-c", GTK_CLIPBOARD_GET_PY]),
    ];
    let mut saw_empty_success = false;
    for (bin, args) in candidates {
        if let Ok(output) = Command::new(bin).args(*args).output() {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).to_string();
                if !s.is_empty() {
                    return Some(s);
                }
                saw_empty_success = true;
            }
        }
    }
    if saw_empty_success {
        return Some(String::new());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clipboard_sync_mode_toggle() {
        let mut state = ClipboardState::default();
        assert_eq!(state.sync_mode(), ClipboardSyncMode::Bidirectional);
        assert!(state.auto_sync_system);

        state.set_sync_mode(ClipboardSyncMode::Off);
        assert_eq!(state.sync_mode(), ClipboardSyncMode::Off);
        assert!(!state.auto_sync_system);

        state.set_sync_mode(ClipboardSyncMode::Bidirectional);
        assert_eq!(state.sync_mode(), ClipboardSyncMode::Bidirectional);
        assert!(state.auto_sync_system);
    }

    #[test]
    fn test_clipboard_apply_remote_update_baseline_protection() {
        let mut state = ClipboardState::default();
        state.local_text = "local_baseline".to_string();

        state.apply_remote_clipboard_update("", true);
        assert_eq!(state.local_text, "local_baseline");
        assert_eq!(state.remote_text, "");
        assert_eq!(state.updates_received, 1);

        state.apply_remote_clipboard_update("remote_attempt", false);
        assert_eq!(state.local_text, "local_baseline");
        assert_eq!(state.remote_text, "remote_attempt");
        assert_eq!(state.updates_received, 2);

        state.apply_remote_clipboard_update("remote_valid", true);
        assert_eq!(state.local_text, "remote_valid");
        assert_eq!(state.remote_text, "remote_valid");
        assert_eq!(state.updates_received, 3);
    }

    #[test]
    fn test_clipboard_sync_mode_serde() {
        let json_bi = serde_json::to_string(&ClipboardSyncMode::Bidirectional).unwrap();
        assert_eq!(json_bi, "\"bidirectional\"");
        let de_bi: ClipboardSyncMode = serde_json::from_str(&json_bi).unwrap();
        assert_eq!(de_bi, ClipboardSyncMode::Bidirectional);

        let json_off = serde_json::to_string(&ClipboardSyncMode::Off).unwrap();
        assert_eq!(json_off, "\"off\"");
        let de_off: ClipboardSyncMode = serde_json::from_str(&json_off).unwrap();
        assert_eq!(de_off, ClipboardSyncMode::Off);
    }
}

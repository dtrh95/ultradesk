use crate::protocol::{
    decode_byte_array_fields, decode_utf16le_lossy, encode_byte_array_fields,
    encode_senddata_string, parse_command_fields,
};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::process::{Command, Stdio};

pub const PACKET_CLIPBOARD_SHORT: u8 = 5;
pub const PACKET_CLIPBOARD_SYNC: u8 = 33;

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

impl Default for ClipboardState {
    fn default() -> Self {
        Self {
            local_text: String::new(),
            remote_text: String::new(),
            auto_sync_system: true,
            updates_sent: 0,
            updates_received: 0,
        }
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
            return Some(ClipboardMessage {
                client_id: 0,
                text,
            });
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
            return Some(ClipboardMessage {
                client_id: 0,
                text,
            });
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
        let cap = expected_uncompressed.max(raw.len() * 8).clamp(4096, 16 * 1024 * 1024);
        if let Ok(decompressed) = zstd::bulk::decompress(raw, cap) {
            return decode_utf16le_lossy(&decompressed);
        }
    }
    if raw.len() % 2 == 0 && (raw.contains(&0) || std::str::from_utf8(raw).is_err()) {
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

pub fn set_linux_system_clipboard(text: &str) -> bool {
    let mut any_ok = false;
    if run_clipboard_write("wl-copy", &[], text) {
        any_ok = true;
    }
    if run_clipboard_write("xclip", &["-selection", "clipboard"], text)
        || run_clipboard_write("xsel", &["--clipboard", "--input"], text)
    {
        any_ok = true;
    }
    if !any_ok && run_clipboard_write("python3", &["-c", GTK_CLIPBOARD_SET_PY], text) {
        any_ok = true;
    }
    any_ok
}

pub fn get_linux_system_clipboard() -> Option<String> {
    let candidates: &[(&str, &[&str])] = &[
        ("wl-paste", &["--no-newline"]),
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

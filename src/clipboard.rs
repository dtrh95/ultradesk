use crate::protocol::{
    decode_byte_array_fields, decode_utf16le_lossy, encode_byte_array_fields,
    encode_senddata_string, parse_command_fields,
};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::process::{Command, Stdio};

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

/// Encodes an outbound UltraViewer clipboard packet using the 2-field byte-array wire format
/// (`["|||33|||<client_id>|||", utf16le_text]`) from `uv_unpacked.exe` at `0x6c9be8`.
pub fn encode_clipboard_packet(client_id: i32, text: &str) -> Result<Vec<u8>, String> {
    let header = encode_senddata_string(&format!("|||33|||{}|||", client_id), false);
    let body = encode_senddata_string(text, true);
    encode_byte_array_fields(&[&header, &body])
}

/// Encodes an outbound UltraViewer clipboard packet using the 3-field `Form1.ManipulatePacket`
/// wire format (`[[33], client_id_i32_le, utf16le_text]`) from `uv_unpacked.exe` at `0x7047ce`.
pub fn encode_clipboard_modern_packet(client_id: i32, text: &str) -> Result<Vec<u8>, String> {
    let id_bytes = client_id.to_le_bytes();
    let body = encode_senddata_string(text, true);
    encode_byte_array_fields(&[&[PACKET_CLIPBOARD_SYNC], &id_bytes, &body])
}

/// Decodes an inbound UltraViewer clipboard packet from any of the three wire representations:
/// 1. 3-field byte-array (`[[33], client_id_i32_le, utf16le_text]`)
/// 2. 2-field byte-array (`[b"|||33|||<client_id>|||", utf16le_text]`)
/// 3. Direct or single-field command string (`"|||33|||<client_id>|||<text>"`)
pub fn decode_clipboard_packet(payload: &[u8]) -> Option<ClipboardMessage> {
    if let Ok(fields) = decode_byte_array_fields(payload) {
        if fields.len() >= 3 && fields[0].as_slice() == [PACKET_CLIPBOARD_SYNC] {
            let client_id = if fields[1].len() >= 4 {
                i32::from_le_bytes([fields[1][0], fields[1][1], fields[1][2], fields[1][3]])
            } else {
                0
            };
            let text = decode_clipboard_text_bytes(&fields[2]);
            return Some(ClipboardMessage { client_id, text });
        }
        if fields.len() == 2 && fields[0].as_slice() == [PACKET_CLIPBOARD_SYNC] {
            let text = decode_clipboard_text_bytes(&fields[1]);
            return Some(ClipboardMessage {
                client_id: 0,
                text,
            });
        }
        if fields.len() >= 2 && fields[0].starts_with(b"|||33|||") {
            let header_str = String::from_utf8_lossy(&fields[0]);
            let parts = parse_command_fields(&header_str);
            let client_id = parts
                .get(1)
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(0);
            let text = decode_clipboard_text_bytes(&fields[1]);
            return Some(ClipboardMessage { client_id, text });
        }
        if fields.len() == 1 && fields[0].starts_with(b"|||33|||") {
            return parse_legacy_clipboard_str(&String::from_utf8_lossy(&fields[0]));
        }
    }
    if payload.starts_with(b"|||33|||") {
        return parse_legacy_clipboard_str(&String::from_utf8_lossy(payload));
    }
    None
}

fn decode_clipboard_text_bytes(raw: &[u8]) -> String {
    if raw.is_empty() {
        return String::new();
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

pub fn set_linux_system_clipboard(text: &str) -> bool {
    let candidates: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (bin, args) in candidates {
        if let Ok(mut child) = Command::new(bin)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if let Ok(status) = child.wait() {
                if status.success() {
                    return true;
                }
            }
        }
    }
    false
}

pub fn get_linux_system_clipboard() -> Option<String> {
    let candidates: &[(&str, &[&str])] = &[
        ("wl-paste", &["--no-newline"]),
        ("xclip", &["-selection", "clipboard", "-o"]),
        ("xsel", &["--clipboard", "--output"]),
    ];
    for (bin, args) in candidates {
        if let Ok(output) = Command::new(bin).args(*args).output() {
            if output.status.success() {
                return Some(String::from_utf8_lossy(&output.stdout).to_string());
            }
        }
    }
    None
}

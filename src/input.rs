use serde::{Deserialize, Serialize};

pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_MBUTTONDOWN: u32 = 0x0207;
pub const WM_MBUTTONUP: u32 = 0x0208;
pub const WM_MOUSEWHEEL: u32 = 0x020A;

pub const KEY_DOWN: u8 = 1;
pub const KEY_UP: u8 = 2;
pub const SYS_KEY_DOWN: u8 = 3;
pub const SYS_KEY_UP: u8 = 4;
pub const VK_PACKET: u8 = 0xE7;

#[inline]
pub fn pack_lparam(x: i32, y: i32) -> i32 {
    ((x as i16 as u16 as u32) | ((y as i16 as u16 as u32) << 16)) as i32
}

#[inline]
pub fn unpack_lparam(lparam: i32) -> (i32, i32) {
    ((lparam as i16) as i32, ((lparam >> 16) as i16) as i32)
}

#[inline]
pub fn pack_wheel_wparam(delta: i16, key_flags: u16) -> i32 {
    ((key_flags as u32) | ((delta as u16 as u32) << 16)) as i32
}

pub fn mouse_move(x: i32, y: i32) -> Result<Vec<u8>, String> {
    if !(-32768..=32767).contains(&x) || !(-32768..=32767).contains(&y) {
        return Err("input value outside supported range".to_string());
    }
    Ok(format!("8|||512|||0|||{}|||{}", x, y).into_bytes())
}

pub fn mouse_button(button: &str, down: bool) -> Result<Vec<u8>, String> {
    let code = match (button, down) {
        ("left", true) => 513,
        ("left", false) => 514,
        ("right", true) => 516,
        ("right", false) => 517,
        ("middle", true) => 519,
        ("middle", false) => 520,
        _ => return Err("unsupported mouse button or state".to_string()),
    };
    Ok(format!("8|||{}", code).into_bytes())
}

pub fn mouse_button_at(
    button: &str,
    down: bool,
    wparam: i32,
    x: i32,
    y: i32,
) -> Result<Vec<u8>, String> {
    let code = match (button, down) {
        ("left", true) => WM_LBUTTONDOWN,
        ("left", false) => WM_LBUTTONUP,
        ("right", true) => WM_RBUTTONDOWN,
        ("right", false) => WM_RBUTTONUP,
        ("middle", true) => WM_MBUTTONDOWN,
        ("middle", false) => WM_MBUTTONUP,
        _ => return Err("unsupported mouse button or state".to_string()),
    };
    Ok(format!("8|||{}|||{}|||{}", code, wparam, pack_lparam(x, y)).into_bytes())
}

pub fn mouse_wheel(delta: i32) -> Result<Vec<u8>, String> {
    if !(-32768..=32767).contains(&delta) {
        return Err("input value outside supported range".to_string());
    }
    let number = (delta as i64) << 16;
    Ok(format!("8|||522|||{}", number).into_bytes())
}

pub fn mouse_wheel_at(delta: i32, key_flags: u16, x: i32, y: i32) -> Result<Vec<u8>, String> {
    if !(-32768..=32767).contains(&delta) {
        return Err("input value outside supported range".to_string());
    }
    let wp = pack_wheel_wparam(delta as i16, key_flags);
    let lp = pack_lparam(x, y);
    Ok(format!("8|||522|||{}|||{}", wp, lp).into_bytes())
}

pub fn normalize_vk(vk: i32) -> i32 {
    match vk {
        160 | 161 => 16,
        162 | 163 => 17,
        164 | 165 => 18,
        other => other,
    }
}

pub fn keyboard(vk: i32, down: bool, system: bool) -> Result<Vec<u8>, String> {
    if !(0..=255).contains(&vk) {
        return Err("input value outside supported range".to_string());
    }
    let vk = normalize_vk(vk);
    let event = if system {
        if down { 260 } else { 261 }
    } else if down {
        256
    } else {
        257
    };
    Ok(format!("8|||{}|||{}", event, vk).into_bytes())
}

pub fn keyboard_legacy_5field(vk: i32, down: bool, system: bool) -> Result<Vec<u8>, String> {
    if !(0..=255).contains(&vk) {
        return Err("input value outside supported range".to_string());
    }
    let vk = normalize_vk(vk);
    let event = if system {
        if down { 260 } else { 261 }
    } else if down {
        256
    } else {
        257
    };
    Ok(format!("8|||{}|||{}|||-1|||-1", event, vk).into_bytes())
}

pub fn with_held_key<S, F, R>(mut send: S, vk: i32, system: bool, action: F) -> Result<R, String>
where
    S: FnMut(Vec<u8>),
    F: FnOnce() -> Result<R, String>,
{
    send(keyboard(vk, true, system)?);
    let result = action();
    if let Ok(release) = keyboard(vk, false, system) {
        send(release);
    }
    result
}

pub fn supports_binary_keyboard(partner_version: &str) -> bool {
    let parts: Vec<u32> = partner_version
        .split('.')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let target = [6, 2, 13];
    for i in 0..3 {
        let v = parts.get(i).copied().unwrap_or(0);
        if v > target[i] {
            return true;
        }
        if v < target[i] {
            return false;
        }
    }
    true
}

pub fn vk_to_scancode(vk: u8) -> u16 {
    match vk {
        0x1B => 0x01,
        b'1' => 0x02,
        b'2' => 0x03,
        b'3' => 0x04,
        b'4' => 0x05,
        b'5' => 0x06,
        b'6' => 0x07,
        b'7' => 0x08,
        b'8' => 0x09,
        b'9' => 0x0A,
        b'0' => 0x0B,
        0xBD => 0x0C,
        0xBB => 0x0D,
        0x08 => 0x0E,
        0x09 => 0x0F,
        b'Q' => 0x10,
        b'W' => 0x11,
        b'E' => 0x12,
        b'R' => 0x13,
        b'T' => 0x14,
        b'Y' => 0x15,
        b'U' => 0x16,
        b'I' => 0x17,
        b'O' => 0x18,
        b'P' => 0x19,
        0xDB => 0x1A,
        0xDD => 0x1B,
        0x0D => 0x1C,
        0x11 | 0xA2 => 0x1D,
        b'A' => 0x1E,
        b'S' => 0x1F,
        b'D' => 0x20,
        b'F' => 0x21,
        b'G' => 0x22,
        b'H' => 0x23,
        b'J' => 0x24,
        b'K' => 0x25,
        b'L' => 0x26,
        0xBA => 0x27,
        0xDE => 0x28,
        0xC0 => 0x29,
        0x10 | 0xA0 => 0x2A,
        0xDC => 0x2B,
        b'Z' => 0x2C,
        b'X' => 0x2D,
        b'C' => 0x2E,
        b'V' => 0x2F,
        b'B' => 0x30,
        b'N' => 0x31,
        b'M' => 0x32,
        0xBC => 0x33,
        0xBE => 0x34,
        0xBF => 0x35,
        0xA1 => 0x36,
        0x6A => 0x37,
        0x12 | 0xA4 => 0x38,
        0x20 => 0x39,
        0x14 => 0x3A,
        0x70 => 0x3B,
        0x71 => 0x3C,
        0x72 => 0x3D,
        0x73 => 0x3E,
        0x74 => 0x3F,
        0x75 => 0x40,
        0x76 => 0x41,
        0x77 => 0x42,
        0x78 => 0x43,
        0x79 => 0x44,
        0x90 => 0x45,
        0x91 => 0x46,
        0x67 => 0x47,
        0x68 => 0x48,
        0x69 => 0x49,
        0x6D => 0x4A,
        0x64 => 0x4B,
        0x65 => 0x4C,
        0x66 => 0x4D,
        0x6B => 0x4E,
        0x61 => 0x4F,
        0x62 => 0x50,
        0x63 => 0x51,
        0x60 => 0x52,
        0x6E => 0x53,
        0x7A => 0x57,
        0x7B => 0x58,
        0xA3 => 0xE01D,
        0xA5 => 0xE038,
        0x5B => 0xE05B,
        0x5C => 0xE05C,
        0x5D => 0xE05D,
        0x2D => 0xE052,
        0x2E => 0xE053,
        0x24 => 0xE047,
        0x23 => 0xE04F,
        0x21 => 0xE049,
        0x22 => 0xE051,
        0x25 => 0xE04B,
        0x27 => 0xE04D,
        0x26 => 0xE048,
        0x28 => 0xE050,
        0x6F => 0xE035,
        _ => 0,
    }
}

pub fn encode_keyboard_binary(kind: u8, vk: u8, scan: u16, ch: u16, caps_lock: bool) -> [u8; 11] {
    [
        30,
        b'|',
        b'|',
        b'|',
        kind,
        vk,
        scan as u8,
        (scan >> 8) as u8,
        ch as u8,
        (ch >> 8) as u8,
        u8::from(caps_lock),
    ]
}

pub fn encode_unicode_text_packets(text: &str, caps_lock: bool) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for u in text.encode_utf16() {
        if u < 0x20 || (0xd800..=0xdfff).contains(&u) {
            continue;
        }
        let down = encode_keyboard_binary(KEY_DOWN, VK_PACKET, 0, u, caps_lock);
        let up = encode_keyboard_binary(KEY_UP, VK_PACKET, 0, u, caps_lock);
        out.push(down.to_vec());
        out.push(up.to_vec());
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostInputEvent {
    Mouse {
        msg: u32,
        wparam: i32,
        x: i32,
        y: i32,
        wheel: i16,
    },
    Key {
        kind: u8,
        vk: u8,
        scan: u16,
        ch: u16,
        caps_lock: bool,
    },
}

pub fn decode_host_input(payload: &[u8]) -> Option<HostInputEvent> {
    if payload.first() == Some(&30) {
        if payload.len() != 11
            || payload[1..4] != [b'|', b'|', b'|']
            || !(1..=4).contains(&payload[4])
            || payload[10] > 1
        {
            return None;
        }
        return Some(HostInputEvent::Key {
            kind: payload[4],
            vk: payload[5],
            scan: u16::from_le_bytes([payload[6], payload[7]]),
            ch: u16::from_le_bytes([payload[8], payload[9]]),
            caps_lock: payload[10] != 0,
        });
    }

    if !payload.starts_with(b"8|||")
        || payload.len() > 256
        || !payload.iter().all(|b| (32..=126).contains(b))
    {
        return None;
    }

    let text = std::str::from_utf8(payload).ok()?;
    let fields: Vec<&str> = text.split("|||").collect();
    if fields.len() < 2 {
        return None;
    }
    let msg_code: u32 = fields[1].parse().ok()?;

    if [256, 257, 260, 261].contains(&msg_code) {
        let vk_raw: i32 = fields.get(2).and_then(|s| s.parse().ok())?;
        if !(0..=255).contains(&vk_raw) {
            return None;
        }
        let vk = normalize_vk(vk_raw) as u8;
        let kind = match msg_code {
            256 => KEY_DOWN,
            257 => KEY_UP,
            260 => SYS_KEY_DOWN,
            261 => SYS_KEY_UP,
            _ => return None,
        };
        let scan = vk_to_scancode(vk);
        return Some(HostInputEvent::Key {
            kind,
            vk,
            scan,
            ch: 0,
            caps_lock: false,
        });
    }

    if msg_code == WM_MOUSEMOVE {
        if fields.len() == 5 {
            let wparam: i32 = fields[2].parse().ok()?;
            let x: i32 = fields[3].parse().ok()?;
            let y: i32 = fields[4].parse().ok()?;
            return Some(HostInputEvent::Mouse {
                msg: WM_MOUSEMOVE,
                wparam,
                x,
                y,
                wheel: 0,
            });
        }
    }

    if msg_code == WM_MOUSEWHEEL {
        if fields.len() == 3 {
            let wp: i64 = fields[2].parse().ok()?;
            let wheel = (wp >> 16) as i16;
            return Some(HostInputEvent::Mouse {
                msg: WM_MOUSEWHEEL,
                wparam: wp as i32,
                x: 0,
                y: 0,
                wheel,
            });
        } else if fields.len() == 4 {
            let wp: i32 = fields[2].parse().ok()?;
            let lp: i32 = fields[3].parse().ok()?;
            let (x, y) = unpack_lparam(lp);
            let wheel = (wp >> 16) as i16;
            return Some(HostInputEvent::Mouse {
                msg: WM_MOUSEWHEEL,
                wparam: wp,
                x,
                y,
                wheel,
            });
        }
    }

    if (513..=520).contains(&msg_code) {
        if fields.len() == 2 {
            return Some(HostInputEvent::Mouse {
                msg: msg_code,
                wparam: 0,
                x: 0,
                y: 0,
                wheel: 0,
            });
        } else if fields.len() == 4 {
            let wp: i32 = fields[2].parse().ok()?;
            let lp: i32 = fields[3].parse().ok()?;
            let (x, y) = unpack_lparam(lp);
            return Some(HostInputEvent::Mouse {
                msg: msg_code,
                wparam: wp,
                x,
                y,
                wheel: 0,
            });
        }
    }

    None
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputEvent {
    #[serde(rename = "type", default)]
    pub event_type: String,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default)]
    pub button: String,
    #[serde(default)]
    pub down: bool,
    #[serde(default)]
    pub delta: i32,
    #[serde(default)]
    pub vk: i32,
    #[serde(default)]
    pub system: bool,
    #[serde(default)]
    pub wparam: i32,
    #[serde(default)]
    pub scan: u16,
    #[serde(default)]
    pub ch: u16,
    #[serde(default)]
    pub caps_lock: bool,
    #[serde(default)]
    pub binary: bool,
    #[serde(default)]
    pub text: String,
}

impl InputEvent {
    pub fn to_wire(&self) -> Result<Vec<u8>, String> {
        if self.binary && self.event_type == "key" {
            let kind = if self.system {
                if self.down {
                    SYS_KEY_DOWN
                } else {
                    SYS_KEY_UP
                }
            } else if self.down {
                KEY_DOWN
            } else {
                KEY_UP
            };
            let scan = if self.scan != 0 {
                self.scan
            } else {
                vk_to_scancode(self.vk as u8)
            };
            return Ok(encode_keyboard_binary(
                kind,
                self.vk as u8,
                scan,
                self.ch,
                self.caps_lock,
            )
            .to_vec());
        }

        if self.event_type == "text" || !self.text.is_empty() {
            let packets = encode_unicode_text_packets(&self.text, self.caps_lock);
            let mut flattened = Vec::new();
            for p in packets {
                flattened.extend_from_slice(&p);
            }
            return Ok(flattened);
        }

        match self.event_type.as_str() {
            "move" => mouse_move(self.x, self.y),
            "button" => {
                if self.wparam != 0 || self.x != 0 || self.y != 0 {
                    mouse_button_at(&self.button, self.down, self.wparam, self.x, self.y)
                } else {
                    mouse_button(&self.button, self.down)
                }
            }
            "wheel" => {
                if self.wparam != 0 || self.x != 0 || self.y != 0 {
                    mouse_wheel_at(self.delta, self.wparam as u16, self.x, self.y)
                } else {
                    mouse_wheel(self.delta)
                }
            }
            "key" => keyboard(self.vk, self.down, self.system),
            _ => Err("unknown input event".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pack_and_unpack_lparam() {
        assert_eq!(pack_lparam(0, 0), 0);
        assert_eq!(pack_lparam(640, 360), 640 | (360 << 16));
        assert_eq!(unpack_lparam(pack_lparam(640, 360)), (640, 360));
        assert_eq!(unpack_lparam(pack_lparam(-100, -200)), (-100, -200));
    }

    #[test]
    fn test_wheel_wparam() {
        let wp = pack_wheel_wparam(120, 0x0004);
        assert_eq!(wp, 0x0004 | (120 << 16));
    }

    #[test]
    fn test_supports_binary_keyboard() {
        assert!(supports_binary_keyboard("6.2.13"));
        assert!(supports_binary_keyboard("6.2.14"));
        assert!(supports_binary_keyboard("6.6.133"));
        assert!(!supports_binary_keyboard("6.2.12"));
        assert!(!supports_binary_keyboard("5.9.1"));
    }

    #[test]
    fn test_encode_and_decode_keyboard_binary() {
        let packet = encode_keyboard_binary(KEY_DOWN, b'A', 0x1E, 0, false);
        assert_eq!(packet.len(), 11);
        assert_eq!(packet[0], 30);
        assert_eq!(&packet[1..4], b"|||");
        assert_eq!(packet[4], KEY_DOWN);
        assert_eq!(packet[5], b'A');
        assert_eq!(packet[6], 0x1E);
        assert_eq!(packet[7], 0);

        let decoded = decode_host_input(&packet).unwrap();
        assert_eq!(
            decoded,
            HostInputEvent::Key {
                kind: KEY_DOWN,
                vk: b'A',
                scan: 0x1E,
                ch: 0,
                caps_lock: false,
            }
        );
    }

    #[test]
    fn test_unicode_text_packets() {
        let packets = encode_unicode_text_packets("Xin chào", false);
        assert_eq!(packets.len(), 8 * 2);
        for pair in packets.chunks_exact(2) {
            let down = decode_host_input(&pair[0]).unwrap();
            let up = decode_host_input(&pair[1]).unwrap();
            let (down_kind, down_vk, down_ch) = match down {
                HostInputEvent::Key { kind, vk, ch, .. } => (kind, vk, ch),
                _ => panic!("Expected key event"),
            };
            let (up_kind, up_vk, up_ch) = match up {
                HostInputEvent::Key { kind, vk, ch, .. } => (kind, vk, ch),
                _ => panic!("Expected key event"),
            };
            assert_eq!(down_kind, KEY_DOWN);
            assert_eq!(up_kind, KEY_UP);
            assert_eq!(down_vk, VK_PACKET);
            assert_eq!(up_vk, VK_PACKET);
            assert_eq!(down_ch, up_ch);
        }
    }

    #[test]
    fn test_decode_host_mouse() {
        let move_pkt = b"8|||512|||0|||1920|||1080";
        assert_eq!(
            decode_host_input(move_pkt).unwrap(),
            HostInputEvent::Mouse {
                msg: WM_MOUSEMOVE,
                wparam: 0,
                x: 1920,
                y: 1080,
                wheel: 0,
            }
        );

        let btn_pkt = mouse_button_at("left", true, 1, 100, 200).unwrap();
        assert_eq!(
            decode_host_input(&btn_pkt).unwrap(),
            HostInputEvent::Mouse {
                msg: WM_LBUTTONDOWN,
                wparam: 1,
                x: 100,
                y: 200,
                wheel: 0,
            }
        );

        let legacy_wheel = mouse_wheel(120).unwrap();
        assert_eq!(
            decode_host_input(&legacy_wheel).unwrap(),
            HostInputEvent::Mouse {
                msg: WM_MOUSEWHEEL,
                wparam: 120 << 16,
                x: 0,
                y: 0,
                wheel: 120,
            }
        );
    }
}

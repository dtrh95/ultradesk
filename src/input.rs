use serde::{Deserialize, Serialize};

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

pub fn mouse_wheel(delta: i32) -> Result<Vec<u8>, String> {
    if !(-32768..=32767).contains(&delta) {
        return Err("input value outside supported range".to_string());
    }
    let number = (delta as i64) << 16;
    Ok(format!("8|||522|||{}", number).into_bytes())
}

pub fn keyboard(vk: i32, down: bool, system: bool) -> Result<Vec<u8>, String> {
    if !(0..=255).contains(&vk) {
        return Err("input value outside supported range".to_string());
    }
    let event = if system {
        if down { 260 } else { 261 }
    } else if down {
        256
    } else {
        257
    };
    Ok(format!("8|||{}|||{}", event, vk).into_bytes())
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
}

impl InputEvent {
    pub fn to_wire(&self) -> Result<Vec<u8>, String> {
        match self.event_type.as_str() {
            "move" => mouse_move(self.x, self.y),
            "button" => mouse_button(&self.button, self.down),
            "wheel" => mouse_wheel(self.delta),
            "key" => keyboard(self.vk, self.down, self.system),
            _ => Err("unknown input event".to_string()),
        }
    }
}

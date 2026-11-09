use crate::input::InputEvent;
use crate::server::{decode_base64, encode_base64};
use crate::session::ControlSession;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

pub const DEFAULT_MCP_PORT: u16 = 9333;
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransportOrigin {
    Stdio,
    Network,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTool {
    GetSessionStatus,
    ConnectRemote,
    DisconnectRemote,
    TakeScreenshot,
    ListMonitors,
    SelectMonitor,
    MouseMove,
    MouseClick,
    MouseDrag,
    MouseScroll,
    TypeText,
    KeyPress,
    GetClipboard,
    SetClipboard,
    SendChat,
    ListFiles,
    SendFile,
    ManageFileTransfer,
    ManageHost,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

impl McpTool {
    pub const ALL: [McpTool; 19] = [
        Self::GetSessionStatus,
        Self::ConnectRemote,
        Self::DisconnectRemote,
        Self::TakeScreenshot,
        Self::ListMonitors,
        Self::SelectMonitor,
        Self::MouseMove,
        Self::MouseClick,
        Self::MouseDrag,
        Self::MouseScroll,
        Self::TypeText,
        Self::KeyPress,
        Self::GetClipboard,
        Self::SetClipboard,
        Self::SendChat,
        Self::ListFiles,
        Self::SendFile,
        Self::ManageFileTransfer,
        Self::ManageHost,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::GetSessionStatus => "get_session_status",
            Self::ConnectRemote => "connect_remote",
            Self::DisconnectRemote => "disconnect_remote",
            Self::TakeScreenshot => "take_screenshot",
            Self::ListMonitors => "list_monitors",
            Self::SelectMonitor => "select_monitor",
            Self::MouseMove => "mouse_move",
            Self::MouseClick => "mouse_click",
            Self::MouseDrag => "mouse_drag",
            Self::MouseScroll => "mouse_scroll",
            Self::TypeText => "type_text",
            Self::KeyPress => "key_press",
            Self::GetClipboard => "get_clipboard",
            Self::SetClipboard => "set_clipboard",
            Self::SendChat => "send_chat",
            Self::ListFiles => "list_files",
            Self::SendFile => "send_file",
            Self::ManageFileTransfer => "manage_file_transfer",
            Self::ManageHost => "manage_host",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == name.trim())
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::GetSessionStatus => {
                "Return the full UltraDesk client and host session status, resolution, monitors, cursor coordinates, encryption state, clipboard counters, and active file transfers."
            }
            Self::ConnectRemote => {
                "Connect to a remote UltraDesk partner using their 9-digit partner ID (or LAN address) and password."
            }
            Self::DisconnectRemote => {
                "Disconnect the active remote desktop session."
            }
            Self::TakeScreenshot => {
                "Capture a screenshot of the active remote desktop or local host display and return it as a base64-encoded PNG or JPEG image."
            }
            Self::ListMonitors => {
                "List detected displays and active monitor selections for both the remote session and the local host."
            }
            Self::SelectMonitor => {
                "Select all monitors, a single monitor index, or a subset of monitor indices on the remote view or local host."
            }
            Self::MouseMove => {
                "Move the mouse pointer to the specified (x, y) coordinates on the remote desktop or local host."
            }
            Self::MouseClick => {
                "Perform a mouse button action (click, double_click, down, or up) with left, right, or middle button at optional (x, y) coordinates."
            }
            Self::MouseDrag => {
                "Drag the mouse pointer from (start_x, start_y) to (end_x, end_y) while holding a mouse button."
            }
            Self::MouseScroll => {
                "Scroll the mouse wheel vertically by delta units at optional (x, y) coordinates."
            }
            Self::TypeText => {
                "Type Unicode text (including international characters and symbols) into the remote desktop or local host."
            }
            Self::KeyPress => {
                "Press, release, or tap a keyboard key or shortcut combination (such as Enter, Tab, Escape, Backspace, arrows, function keys, or Ctrl/Alt/Shift/Meta combinations)."
            }
            Self::GetClipboard => {
                "Read the current local and remote clipboard text and synchronization counters."
            }
            Self::SetClipboard => {
                "Send clipboard text to the remote partner or synchronize from the local system clipboard."
            }
            Self::SendChat => {
                "Send a chat message to the connected remote partner and return recent chat history."
            }
            Self::ListFiles => {
                "List active and completed file transfers and chat messages in the current session."
            }
            Self::SendFile => {
                "Offer and send a file to the connected partner from base64 data or a local file path."
            }
            Self::ManageFileTransfer => {
                "Accept, cancel, or download a file transfer by file_id."
            }
            Self::ManageHost => {
                "Inspect or control the local UltraDesk Host service (status, start, stop, approve, decline, set_control, set_master_password, check_update, apply_update, dismiss_update)."
            }
        }
    }

    pub fn input_schema(self) -> Value {
        match self {
            Self::GetSessionStatus => json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            Self::ConnectRemote => json!({
                "type": "object",
                "properties": {
                    "partner_id": {
                        "type": "string",
                        "description": "9-digit partner ID or direct LAN address (e.g. '130173637' or 'lan://192.168.1.10:2112')."
                    },
                    "password": {
                        "type": "string",
                        "description": "Session or unattended master password for the remote partner."
                    },
                    "remember": {
                        "type": "boolean",
                        "description": "Whether to save credentials in the local profile.",
                        "default": true
                    }
                },
                "required": ["partner_id", "password"]
            }),
            Self::DisconnectRemote => json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            Self::TakeScreenshot => json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "enum": ["auto", "remote", "host"],
                        "description": "Screen source: 'remote' for connected partner, 'host' for local desktop, or 'auto' (remote if connected, otherwise host).",
                        "default": "auto"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["png", "jpeg"],
                        "description": "Image encoding format.",
                        "default": "png"
                    },
                    "quality": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "description": "JPEG quality (1-100) when format is 'jpeg'.",
                        "default": 80
                    }
                }
            }),
            Self::ListMonitors => json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            Self::SelectMonitor => json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "enum": ["remote", "host"],
                        "description": "Whether to change monitor selection for the remote viewer or local host.",
                        "default": "remote"
                    },
                    "index": {
                        "type": ["integer", "null"],
                        "description": "Single monitor index (0-based), or null to select all monitors."
                    },
                    "indices": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 0 },
                        "description": "Optional list of monitor indices to combine into a virtual bounding rectangle."
                    }
                }
            }),
            Self::MouseMove => json!({
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "description": "Horizontal pixel coordinate." },
                    "y": { "type": "integer", "description": "Vertical pixel coordinate." },
                    "target": {
                        "type": "string",
                        "enum": ["auto", "remote", "host"],
                        "default": "auto"
                    }
                },
                "required": ["x", "y"]
            }),
            Self::MouseClick => json!({
                "type": "object",
                "properties": {
                    "button": {
                        "type": "string",
                        "enum": ["left", "right", "middle"],
                        "default": "left"
                    },
                    "action": {
                        "type": "string",
                        "enum": ["click", "double_click", "down", "up"],
                        "default": "click"
                    },
                    "x": { "type": "integer", "description": "Optional X coordinate to move to before clicking." },
                    "y": { "type": "integer", "description": "Optional Y coordinate to move to before clicking." },
                    "target": {
                        "type": "string",
                        "enum": ["auto", "remote", "host"],
                        "default": "auto"
                    }
                }
            }),
            Self::MouseDrag => json!({
                "type": "object",
                "properties": {
                    "start_x": { "type": "integer" },
                    "start_y": { "type": "integer" },
                    "end_x": { "type": "integer" },
                    "end_y": { "type": "integer" },
                    "button": {
                        "type": "string",
                        "enum": ["left", "right", "middle"],
                        "default": "left"
                    },
                    "target": {
                        "type": "string",
                        "enum": ["auto", "remote", "host"],
                        "default": "auto"
                    }
                },
                "required": ["start_x", "start_y", "end_x", "end_y"]
            }),
            Self::MouseScroll => json!({
                "type": "object",
                "properties": {
                    "delta": {
                        "type": "integer",
                        "description": "Wheel scroll amount (positive for up, negative for down, e.g. 120 or -120)."
                    },
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "target": {
                        "type": "string",
                        "enum": ["auto", "remote", "host"],
                        "default": "auto"
                    }
                },
                "required": ["delta"]
            }),
            Self::TypeText => json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Unicode text string to type."
                    },
                    "caps_lock": {
                        "type": "boolean",
                        "default": false
                    },
                    "target": {
                        "type": "string",
                        "enum": ["auto", "remote", "host"],
                        "default": "auto"
                    }
                },
                "required": ["text"]
            }),
            Self::KeyPress => json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "Key name (e.g. 'Enter', 'Tab', 'Escape', 'Backspace', 'Delete', 'Space', 'Up', 'Down', 'Left', 'Right', 'Home', 'End', 'PageUp', 'PageDown', 'F1'..'F12', 'a'..'z', '0'..'9')."
                    },
                    "vk": {
                        "type": "integer",
                        "description": "Optional explicit Windows virtual-key code (overrides 'key' if provided)."
                    },
                    "action": {
                        "type": "string",
                        "enum": ["press", "down", "up"],
                        "default": "press"
                    },
                    "ctrl": { "type": "boolean", "default": false },
                    "alt": { "type": "boolean", "default": false },
                    "shift": { "type": "boolean", "default": false },
                    "meta": { "type": "boolean", "default": false },
                    "target": {
                        "type": "string",
                        "enum": ["auto", "remote", "host"],
                        "default": "auto"
                    }
                }
            }),
            Self::GetClipboard => json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            Self::SetClipboard => json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Clipboard text to set and send."
                    },
                    "sync_from_system": {
                        "type": "boolean",
                        "description": "Read the local OS clipboard and send it to the partner.",
                        "default": false
                    }
                }
            }),
            Self::SendChat => json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Chat message text to send to the partner."
                    }
                },
                "required": ["text"]
            }),
            Self::ListFiles => json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            Self::SendFile => json!({
                "type": "object",
                "properties": {
                    "file_name": {
                        "type": "string",
                        "description": "File name when sending base64 content."
                    },
                    "data_base64": {
                        "type": "string",
                        "description": "Base64-encoded file bytes."
                    },
                    "file_path": {
                        "type": "string",
                        "description": "Local file path (allowed on stdio MCP transport)."
                    }
                }
            }),
            Self::ManageFileTransfer => json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["accept", "cancel", "download", "receive"]
                    },
                    "file_id": {
                        "type": "string",
                        "description": "Identifier of the file transfer (for accept, cancel, download)."
                    },
                    "remote_path": {
                        "type": "string",
                        "description": "Remote file path to pull from the connected remote host (for action='receive')."
                    }
                },
                "required": ["action"]
            }),
            Self::ManageHost => json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "status",
                            "start",
                            "stop",
                            "approve",
                            "decline",
                            "set_control",
                            "set_master_password",
                            "check_update",
                            "apply_update",
                            "confirm_update",
                            "dismiss_update",
                            "later_update"
                        ],
                        "default": "status"
                    },
                    "allow_control": {
                        "type": "boolean",
                        "default": true
                    },
                    "master_password": {
                        "type": ["string", "null"],
                        "description": "New unattended master password (8-64 chars), or null/empty to clear."
                    }
                }
            }),
        }
    }

    pub fn spec(self) -> McpToolSpec {
        McpToolSpec {
            name: self.name(),
            description: self.description(),
            input_schema: self.input_schema(),
        }
    }

    pub fn execute(
        self,
        session: &ControlSession,
        args: &Value,
        origin: McpTransportOrigin,
    ) -> Result<Vec<Value>, String> {
        match self {
            Self::GetSessionStatus => {
                let state = session.snapshot();
                let text = serde_json::to_string_pretty(&state)
                    .map_err(|e| format!("failed to serialize status: {}", e))?;
                Ok(vec![json!({ "type": "text", "text": text })])
            }
            Self::ConnectRemote => {
                let partner_id = args
                    .get("partner_id")
                    .or_else(|| args.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                let password = args
                    .get("password")
                    .or_else(|| args.get("pass"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if partner_id.is_empty() || password.is_empty() {
                    return Err("partner_id and password are required".to_string());
                }
                let remember = args
                    .get("remember")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let state = session.start_connect_with_remember(partner_id, password, remember)?;
                let summary = json!({
                    "ok": true,
                    "partner_id": state.partner_id,
                    "state": state.state,
                    "last_message": state.last_message
                });
                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string_pretty(&summary).unwrap_or_default()
                })])
            }
            Self::DisconnectRemote => {
                session.disconnect();
                let state = session.snapshot();
                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string_pretty(&json!({
                        "ok": true,
                        "state": state.state,
                        "last_message": state.last_message
                    })).unwrap_or_default()
                })])
            }
            Self::TakeScreenshot => {
                let source = args.get("source").and_then(Value::as_str).unwrap_or("auto");
                let format = args.get("format").and_then(Value::as_str).unwrap_or("png");
                let quality = args
                    .get("quality")
                    .and_then(Value::as_u64)
                    .unwrap_or(80)
                    .clamp(1, 100) as u8;
                let snap = session.snapshot();
                let use_remote = match source {
                    "remote" => true,
                    "host" => false,
                    _ => snap.state == "connected" || snap.frames_received > 0,
                };
                let (mime, width, height, frame_ver, bytes, actual_source) = if use_remote {
                    if format.eq_ignore_ascii_case("jpeg") || format.eq_ignore_ascii_case("jpg") {
                        let (ver, jpg) = session.jpeg_frame_versioned(quality)?;
                        ("image/jpeg", snap.width, snap.height, ver, jpg, "remote")
                    } else {
                        let (ver, png) = session.png_frame_versioned()?;
                        ("image/png", snap.width, snap.height, ver, png, "remote")
                    }
                } else {
                    let (w, h, img) = session.host_capture_image(format, quality)?;
                    let mime = if format.eq_ignore_ascii_case("jpeg")
                        || format.eq_ignore_ascii_case("jpg")
                    {
                        "image/jpeg"
                    } else {
                        "image/png"
                    };
                    (mime, w, h, snap.frame_version, img, "host")
                };
                let b64 = encode_base64(&bytes);
                let meta = json!({
                    "source": actual_source,
                    "width": width,
                    "height": height,
                    "frame_version": frame_ver,
                    "mime_type": mime,
                    "bytes": bytes.len()
                });
                Ok(vec![
                    json!({
                        "type": "text",
                        "text": serde_json::to_string(&meta).unwrap_or_default()
                    }),
                    json!({
                        "type": "image",
                        "data": b64,
                        "mimeType": mime
                    }),
                ])
            }
            Self::ListMonitors => {
                let snap = session.snapshot();
                let host = session.host_status();
                let out = json!({
                    "remote": {
                        "width": snap.width,
                        "height": snap.height,
                        "screen_width": snap.screen_width,
                        "screen_height": snap.screen_height,
                        "monitors": snap.monitors,
                        "selected_monitor": snap.selected_monitor,
                        "selected_monitors": snap.selected_monitors
                    },
                    "host": {
                        "backend": host.backend_name,
                        "monitors": host.monitors,
                        "selected_monitor": host.selected_monitor,
                        "selected_monitors": host.selected_monitors
                    }
                });
                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string_pretty(&out).unwrap_or_default()
                })])
            }
            Self::SelectMonitor => {
                let target = args
                    .get("target")
                    .and_then(Value::as_str)
                    .unwrap_or("remote");
                let index = args
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize);
                let indices: Option<Vec<usize>> =
                    args.get("indices").and_then(Value::as_array).map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_u64)
                            .map(|v| v as usize)
                            .collect()
                    });
                if target.eq_ignore_ascii_case("host") {
                    let host = session.host_select_monitors(index, indices.as_deref());
                    Ok(vec![json!({
                        "type": "text",
                        "text": serde_json::to_string_pretty(&host).unwrap_or_default()
                    })])
                } else {
                    let state = if let Some(ref list) = indices {
                        session.select_monitors(Some(list.as_slice()))
                    } else {
                        session.select_monitor(index)
                    };
                    Ok(vec![json!({
                        "type": "text",
                        "text": serde_json::to_string_pretty(&json!({
                            "selected_monitor": state.selected_monitor,
                            "selected_monitors": state.selected_monitors,
                            "width": state.width,
                            "height": state.height
                        })).unwrap_or_default()
                    })])
                }
            }
            Self::MouseMove => {
                let x = args
                    .get("x")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| "missing integer parameter 'x'".to_string())?
                    as i32;
                let y = args
                    .get("y")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| "missing integer parameter 'y'".to_string())?
                    as i32;
                let target = args.get("target").and_then(Value::as_str).unwrap_or("auto");
                let evt = InputEvent {
                    event_type: "move".to_string(),
                    x,
                    y,
                    ..Default::default()
                };
                let routed = dispatch_input_to_target(session, target, evt)?;
                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string(&json!({
                        "ok": true,
                        "action": "mouse_move",
                        "x": x,
                        "y": y,
                        "target": routed
                    })).unwrap_or_default()
                })])
            }
            Self::MouseClick => {
                let button = args
                    .get("button")
                    .and_then(Value::as_str)
                    .unwrap_or("left")
                    .to_string();
                let action = args
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("click");
                let target = args.get("target").and_then(Value::as_str).unwrap_or("auto");
                if let (Some(x), Some(y)) = (
                    args.get("x").and_then(Value::as_i64),
                    args.get("y").and_then(Value::as_i64),
                ) {
                    let move_evt = InputEvent {
                        event_type: "move".to_string(),
                        x: x as i32,
                        y: y as i32,
                        ..Default::default()
                    };
                    dispatch_input_to_target(session, target, move_evt)?;
                }
                let clicks = match action {
                    "double_click" => 2,
                    "click" => 1,
                    "down" => {
                        let routed = dispatch_input_to_target(
                            session,
                            target,
                            InputEvent {
                                event_type: "button".to_string(),
                                button: button.clone(),
                                down: true,
                                ..Default::default()
                            },
                        )?;
                        return Ok(vec![json!({
                            "type": "text",
                            "text": serde_json::to_string(&json!({
                                "ok": true,
                                "button": button,
                                "action": "down",
                                "target": routed
                            })).unwrap_or_default()
                        })]);
                    }
                    "up" => {
                        let routed = dispatch_input_to_target(
                            session,
                            target,
                            InputEvent {
                                event_type: "button".to_string(),
                                button: button.clone(),
                                down: false,
                                ..Default::default()
                            },
                        )?;
                        return Ok(vec![json!({
                            "type": "text",
                            "text": serde_json::to_string(&json!({
                                "ok": true,
                                "button": button,
                                "action": "up",
                                "target": routed
                            })).unwrap_or_default()
                        })]);
                    }
                    other => return Err(format!("unsupported mouse action: {}", other)),
                };
                let mut routed = "remote";
                for _ in 0..clicks {
                    routed = dispatch_input_to_target(
                        session,
                        target,
                        InputEvent {
                            event_type: "button".to_string(),
                            button: button.clone(),
                            down: true,
                            ..Default::default()
                        },
                    )?;
                    dispatch_input_to_target(
                        session,
                        target,
                        InputEvent {
                            event_type: "button".to_string(),
                            button: button.clone(),
                            down: false,
                            ..Default::default()
                        },
                    )?;
                }
                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string(&json!({
                        "ok": true,
                        "button": button,
                        "action": action,
                        "target": routed
                    })).unwrap_or_default()
                })])
            }
            Self::MouseDrag => {
                let start_x = args
                    .get("start_x")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| "missing integer parameter 'start_x'".to_string())?
                    as i32;
                let start_y = args
                    .get("start_y")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| "missing integer parameter 'start_y'".to_string())?
                    as i32;
                let end_x = args
                    .get("end_x")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| "missing integer parameter 'end_x'".to_string())?
                    as i32;
                let end_y = args
                    .get("end_y")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| "missing integer parameter 'end_y'".to_string())?
                    as i32;
                let button = args
                    .get("button")
                    .and_then(Value::as_str)
                    .unwrap_or("left")
                    .to_string();
                let target = args.get("target").and_then(Value::as_str).unwrap_or("auto");
                dispatch_input_to_target(
                    session,
                    target,
                    InputEvent {
                        event_type: "move".to_string(),
                        x: start_x,
                        y: start_y,
                        ..Default::default()
                    },
                )?;
                dispatch_input_to_target(
                    session,
                    target,
                    InputEvent {
                        event_type: "button".to_string(),
                        button: button.clone(),
                        down: true,
                        ..Default::default()
                    },
                )?;
                dispatch_input_to_target(
                    session,
                    target,
                    InputEvent {
                        event_type: "move".to_string(),
                        x: end_x,
                        y: end_y,
                        ..Default::default()
                    },
                )?;
                let routed = dispatch_input_to_target(
                    session,
                    target,
                    InputEvent {
                        event_type: "button".to_string(),
                        button: button.clone(),
                        down: false,
                        ..Default::default()
                    },
                )?;
                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string(&json!({
                        "ok": true,
                        "action": "mouse_drag",
                        "start": [start_x, start_y],
                        "end": [end_x, end_y],
                        "button": button,
                        "target": routed
                    })).unwrap_or_default()
                })])
            }
            Self::MouseScroll => {
                let delta = args
                    .get("delta")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| "missing integer parameter 'delta'".to_string())?
                    as i32;
                let target = args.get("target").and_then(Value::as_str).unwrap_or("auto");
                if let (Some(x), Some(y)) = (
                    args.get("x").and_then(Value::as_i64),
                    args.get("y").and_then(Value::as_i64),
                ) {
                    dispatch_input_to_target(
                        session,
                        target,
                        InputEvent {
                            event_type: "move".to_string(),
                            x: x as i32,
                            y: y as i32,
                            ..Default::default()
                        },
                    )?;
                }
                let routed = dispatch_input_to_target(
                    session,
                    target,
                    InputEvent {
                        event_type: "wheel".to_string(),
                        delta,
                        ..Default::default()
                    },
                )?;
                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string(&json!({
                        "ok": true,
                        "action": "mouse_scroll",
                        "delta": delta,
                        "target": routed
                    })).unwrap_or_default()
                })])
            }
            Self::TypeText => {
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "missing string parameter 'text'".to_string())?;
                let caps_lock = args
                    .get("caps_lock")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let target = args.get("target").and_then(Value::as_str).unwrap_or("auto");
                let use_remote = resolve_target_is_remote(session, target);
                let packets_sent = if use_remote {
                    session.send_text(text, caps_lock)?
                } else {
                    session.host_send_text(text, caps_lock)?
                };
                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string(&json!({
                        "ok": true,
                        "characters": text.chars().count(),
                        "packets_sent": packets_sent,
                        "target": if use_remote { "remote" } else { "host" }
                    })).unwrap_or_default()
                })])
            }
            Self::KeyPress => {
                let vk = if let Some(code) = args.get("vk").and_then(Value::as_i64) {
                    code as i32
                } else if let Some(key_name) = args.get("key").and_then(Value::as_str) {
                    parse_key_name_to_vk(key_name)?
                } else {
                    return Err("either 'key' or 'vk' must be provided".to_string());
                };
                let action = args
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("press");
                let ctrl = args.get("ctrl").and_then(Value::as_bool).unwrap_or(false);
                let alt = args.get("alt").and_then(Value::as_bool).unwrap_or(false);
                let shift = args.get("shift").and_then(Value::as_bool).unwrap_or(false);
                let meta = args.get("meta").and_then(Value::as_bool).unwrap_or(false);
                let target = args.get("target").and_then(Value::as_str).unwrap_or("auto");

                let mut modifiers = Vec::new();
                if ctrl {
                    modifiers.push(17);
                }
                if shift {
                    modifiers.push(16);
                }
                if alt {
                    modifiers.push(18);
                }
                if meta {
                    modifiers.push(91);
                }

                for &m in &modifiers {
                    dispatch_input_to_target(
                        session,
                        target,
                        InputEvent {
                            event_type: "key".to_string(),
                            vk: m,
                            down: true,
                            system: m == 18 || alt,
                            ..Default::default()
                        },
                    )?;
                }

                let routed = match action {
                    "press" => {
                        dispatch_input_to_target(
                            session,
                            target,
                            InputEvent {
                                event_type: "key".to_string(),
                                vk,
                                down: true,
                                system: alt,
                                ..Default::default()
                            },
                        )?;
                        dispatch_input_to_target(
                            session,
                            target,
                            InputEvent {
                                event_type: "key".to_string(),
                                vk,
                                down: false,
                                system: alt,
                                ..Default::default()
                            },
                        )?
                    }
                    "down" => dispatch_input_to_target(
                        session,
                        target,
                        InputEvent {
                            event_type: "key".to_string(),
                            vk,
                            down: true,
                            system: alt,
                            ..Default::default()
                        },
                    )?,
                    "up" => dispatch_input_to_target(
                        session,
                        target,
                        InputEvent {
                            event_type: "key".to_string(),
                            vk,
                            down: false,
                            system: alt,
                            ..Default::default()
                        },
                    )?,
                    other => return Err(format!("unsupported key action: {}", other)),
                };

                for &m in modifiers.iter().rev() {
                    dispatch_input_to_target(
                        session,
                        target,
                        InputEvent {
                            event_type: "key".to_string(),
                            vk: m,
                            down: false,
                            system: m == 18 || alt,
                            ..Default::default()
                        },
                    )?;
                }

                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string(&json!({
                        "ok": true,
                        "vk": vk,
                        "action": action,
                        "target": routed
                    })).unwrap_or_default()
                })])
            }
            Self::GetClipboard => {
                let snap = session.snapshot();
                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string_pretty(&json!({
                        "local_text": snap.clipboard_local,
                        "remote_text": snap.clipboard_remote,
                        "updates_sent": snap.clipboard_updates_sent,
                        "updates_received": snap.clipboard_updates_received
                    })).unwrap_or_default()
                })])
            }
            Self::SetClipboard => {
                let sync_from_system = args
                    .get("sync_from_system")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let text = if sync_from_system {
                    session.sync_from_system_clipboard()?
                } else {
                    let t = args
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "missing 'text' or 'sync_from_system: true'".to_string())?;
                    session.send_clipboard(t, true)?;
                    t.to_string()
                };
                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string(&json!({
                        "ok": true,
                        "length": text.len(),
                        "synced_from_system": sync_from_system
                    })).unwrap_or_default()
                })])
            }
            Self::SendChat => {
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "missing string parameter 'text'".to_string())?;
                session.send_chat(text)?;
                let snap = session.snapshot();
                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string_pretty(&json!({
                        "ok": true,
                        "chat_messages": snap.chat_messages
                    })).unwrap_or_default()
                })])
            }
            Self::ListFiles => {
                let snap = session.snapshot();
                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string_pretty(&json!({
                        "transfers": snap.file_transfers,
                        "chat_messages": snap.chat_messages
                    })).unwrap_or_default()
                })])
            }
            Self::SendFile => {
                let file_path = args.get("file_path").and_then(Value::as_str).unwrap_or("");
                let file_id = if !file_path.trim().is_empty() {
                    if origin != McpTransportOrigin::Stdio {
                        return Err(
                            "Direct file_path is only permitted over stdio MCP; use data_base64 over network transports"
                                .to_string(),
                        );
                    }
                    session.offer_file_from_path(Path::new(file_path.trim()))?
                } else {
                    let b64 = args
                        .get("data_base64")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "missing 'data_base64' or 'file_path'".to_string())?;
                    let bytes = decode_base64(b64)?;
                    let name = args
                        .get("file_name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or("upload.bin");
                    session.offer_file_bytes(name, bytes)?
                };
                Ok(vec![json!({
                    "type": "text",
                    "text": serde_json::to_string(&json!({
                        "ok": true,
                        "file_id": file_id
                    })).unwrap_or_default()
                })])
            }
            Self::ManageFileTransfer => {
                let action = args
                    .get("action")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "missing 'action'".to_string())?;
                match action {
                    "accept" => {
                        let file_id = args
                            .get("file_id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| "missing 'file_id'".to_string())?;
                        session.accept_file(file_id)?;
                        Ok(vec![json!({
                            "type": "text",
                            "text": serde_json::to_string(&json!({
                                "ok": true,
                                "action": "accept",
                                "file_id": file_id
                            })).unwrap_or_default()
                        })])
                    }
                    "cancel" => {
                        let file_id = args
                            .get("file_id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| "missing 'file_id'".to_string())?;
                        session.cancel_file(file_id)?;
                        Ok(vec![json!({
                            "type": "text",
                            "text": serde_json::to_string(&json!({
                                "ok": true,
                                "action": "cancel",
                                "file_id": file_id
                            })).unwrap_or_default()
                        })])
                    }
                    "download" => {
                        let file_id = args
                            .get("file_id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| "missing 'file_id'".to_string())?;
                        let (name, bytes) = session
                            .downloaded_file_bytes(file_id)
                            .ok_or_else(|| format!("file transfer '{}' not found", file_id))?;
                        Ok(vec![json!({
                            "type": "text",
                            "text": serde_json::to_string(&json!({
                                "ok": true,
                                "file_id": file_id,
                                "file_name": name,
                                "size": bytes.len(),
                                "data_base64": encode_base64(&bytes)
                            })).unwrap_or_default()
                        })])
                    }
                    "receive" => {
                        let remote_path = args
                            .get("remote_path")
                            .or_else(|| args.get("file_path"))
                            .and_then(Value::as_str)
                            .ok_or_else(|| "missing 'remote_path'".to_string())?;
                        let wire = session.request_remote_file(remote_path)?;
                        Ok(vec![json!({
                            "type": "text",
                            "text": serde_json::to_string(&json!({
                                "ok": true,
                                "action": "receive",
                                "remote_path": remote_path.trim(),
                                "bytes_sent": wire.len()
                            })).unwrap_or_default()
                        })])
                    }
                    other => Err(format!("unsupported file transfer action: {}", other)),
                }
            }
            Self::ManageHost => {
                let action = args
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("status");
                let allow_control = args
                    .get("allow_control")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                match action {
                    "check_update" => {
                        let snap = session.check_for_updates()?;
                        Ok(vec![json!({
                            "type": "text",
                            "text": serde_json::to_string_pretty(&json!({
                                "ok": true,
                                "action": "check_update",
                                "update_status": snap
                            })).unwrap_or_default()
                        })])
                    }
                    "apply_update" | "confirm_update" => {
                        let msg = session.confirm_update()?;
                        Ok(vec![json!({
                            "type": "text",
                            "text": serde_json::to_string_pretty(&json!({
                                "ok": true,
                                "action": "apply_update",
                                "message": msg,
                                "update_status": session.update_snapshot()
                            })).unwrap_or_default()
                        })])
                    }
                    "dismiss_update" | "later_update" => {
                        let ver = args.get("version").and_then(Value::as_str);
                        session.dismiss_update(ver);
                        Ok(vec![json!({
                            "type": "text",
                            "text": serde_json::to_string_pretty(&json!({
                                "ok": true,
                                "action": "dismiss_update",
                                "update_status": session.update_snapshot()
                            })).unwrap_or_default()
                        })])
                    }
                    "status"
                    | "start"
                    | "stop"
                    | "approve"
                    | "decline"
                    | "set_control"
                    | "set_master_password" => {
                        let host = match action {
                            "status" => session.host_status(),
                            "start" => session.host_start(),
                            "stop" => session.host_stop(),
                            "approve" => session.host_approve(allow_control)?,
                            "decline" => session.host_decline()?,
                            "set_control" => session.host_set_control(allow_control)?,
                            "set_master_password" => {
                                let pw = args.get("master_password").and_then(Value::as_str);
                                session.host_set_master_password(pw)?
                            }
                            _ => unreachable!(),
                        };
                        Ok(vec![json!({
                            "type": "text",
                            "text": serde_json::to_string_pretty(&host).unwrap_or_default()
                        })])
                    }
                    other => Err(format!("unsupported host action: {}", other)),
                }
            }
        }
    }
}

fn resolve_target_is_remote(session: &ControlSession, target: &str) -> bool {
    match target {
        "remote" => true,
        "host" => false,
        _ => {
            let snap = session.snapshot();
            snap.state == "connected" || snap.frames_received > 0
        }
    }
}

fn dispatch_input_to_target(
    session: &ControlSession,
    target: &str,
    event: InputEvent,
) -> Result<&'static str, String> {
    if resolve_target_is_remote(session, target) {
        session.send_input(event)?;
        Ok("remote")
    } else {
        session.host_inject_input(event)?;
        Ok("host")
    }
}

pub fn parse_key_name_to_vk(key: &str) -> Result<i32, String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("key name cannot be empty".to_string());
    }
    if trimmed.chars().count() == 1 {
        let ch = trimmed.chars().next().unwrap();
        if ch.is_ascii_alphabetic() {
            return Ok(ch.to_ascii_uppercase() as i32);
        }
        if ch.is_ascii_digit() {
            return Ok(ch as i32);
        }
    }
    let upper = trimmed.to_ascii_uppercase();
    if let Some(num_str) = upper.strip_prefix('F') {
        if let Ok(n) = num_str.parse::<i32>() {
            if (1..=24).contains(&n) {
                return Ok(111 + n);
            }
        }
    }
    match upper.as_str() {
        "BACKSPACE" | "BKSP" => Ok(8),
        "TAB" => Ok(9),
        "ENTER" | "RETURN" => Ok(13),
        "SHIFT" | "LSHIFT" | "RSHIFT" => Ok(16),
        "CTRL" | "CONTROL" | "LCTRL" | "RCTRL" => Ok(17),
        "ALT" | "OPTION" | "LALT" | "RALT" => Ok(18),
        "PAUSE" => Ok(19),
        "CAPSLOCK" | "CAPS" => Ok(20),
        "ESC" | "ESCAPE" => Ok(27),
        "SPACE" | "SPACEBAR" => Ok(32),
        "PAGEUP" | "PGUP" => Ok(33),
        "PAGEDOWN" | "PGDN" => Ok(34),
        "END" => Ok(35),
        "HOME" => Ok(36),
        "LEFT" | "ARROWLEFT" => Ok(37),
        "UP" | "ARROWUP" => Ok(38),
        "RIGHT" | "ARROWRIGHT" => Ok(39),
        "DOWN" | "ARROWDOWN" => Ok(40),
        "INSERT" | "INS" => Ok(45),
        "DELETE" | "DEL" => Ok(46),
        "META" | "SUPER" | "WIN" | "COMMAND" | "CMD" => Ok(91),
        _ => Err(format!("unrecognized key name: '{}'", trimmed)),
    }
}

pub fn list_mcp_tools() -> Vec<McpToolSpec> {
    McpTool::ALL.into_iter().map(McpTool::spec).collect()
}

pub fn handle_mcp_raw_json(
    session: &ControlSession,
    raw_bytes: &[u8],
    origin: McpTransportOrigin,
) -> Option<Value> {
    let parsed: Value = match serde_json::from_slice(raw_bytes) {
        Ok(v) => v,
        Err(e) => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": {
                    "code": -32700,
                    "message": format!("Parse error: {}", e)
                }
            }));
        }
    };
    handle_mcp_value(session, &parsed, origin)
}

pub fn handle_mcp_value(
    session: &ControlSession,
    request: &Value,
    origin: McpTransportOrigin,
) -> Option<Value> {
    if let Some(batch) = request.as_array() {
        if batch.is_empty() {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": {
                    "code": -32600,
                    "message": "Invalid Request: empty batch"
                }
            }));
        }
        let responses: Vec<Value> = batch
            .iter()
            .filter_map(|item| handle_single_mcp_object(session, item, origin))
            .collect();
        if responses.is_empty() {
            None
        } else {
            Some(Value::Array(responses))
        }
    } else {
        handle_single_mcp_object(session, request, origin)
    }
}

fn handle_single_mcp_object(
    session: &ControlSession,
    req: &Value,
    origin: McpTransportOrigin,
) -> Option<Value> {
    let Some(obj) = req.as_object() else {
        return Some(json!({
            "jsonrpc": "2.0",
            "id": Value::Null,
            "error": {
                "code": -32600,
                "message": "Invalid Request: expected JSON object"
            }
        }));
    };

    let has_id = obj.contains_key("id");
    let id = obj.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        if !has_id {
            return None;
        }
        return Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32600,
                "message": "Invalid Request: missing 'method'"
            }
        }));
    };

    if !has_id || method.starts_with("notifications/") {
        return None;
    }

    let params = obj.get("params").cloned().unwrap_or_else(|| json!({}));

    match method {
        "initialize" => {
            let requested_version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(MCP_PROTOCOL_VERSION);
            let negotiated = match requested_version {
                "2024-11-05" | "2025-03-26" => requested_version,
                _ => MCP_PROTOCOL_VERSION,
            };
            Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": negotiated,
                    "capabilities": {
                        "tools": {
                            "listChanged": false
                        },
                        "resources": {
                            "subscribe": false,
                            "listChanged": false
                        },
                        "prompts": {
                            "listChanged": false
                        }
                    },
                    "serverInfo": {
                        "name": "ultradesk-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            }))
        }
        "ping" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {}
        })),
        "tools/list" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": list_mcp_tools()
            }
        })),
        "tools/call" => {
            let Some(tool_name) = params.get("name").and_then(Value::as_str) else {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32602,
                        "message": "Invalid params: missing tool 'name'"
                    }
                }));
            };
            let Some(tool) = McpTool::from_name(tool_name) else {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32602,
                        "message": format!("Unknown tool: {}", tool_name)
                    }
                }));
            };
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match tool.execute(session, &arguments, origin) {
                Ok(content) => Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": content,
                        "isError": false
                    }
                })),
                Err(err) => Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": err
                            }
                        ],
                        "isError": true
                    }
                })),
            }
        }
        "resources/list" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "resources": [
                    {
                        "uri": "ultradesk://status",
                        "name": "UltraDesk Session Status",
                        "description": "Live client and host session state snapshot.",
                        "mimeType": "application/json"
                    },
                    {
                        "uri": "ultradesk://host/status",
                        "name": "UltraDesk Host Status",
                        "description": "Local Host ID, temporary password, display backend, and viewer state.",
                        "mimeType": "application/json"
                    },
                    {
                        "uri": "ultradesk://clipboard",
                        "name": "UltraDesk Clipboard State",
                        "description": "Current local and remote clipboard text and sync counters.",
                        "mimeType": "application/json"
                    },
                    {
                        "uri": "ultradesk://screen/current.png",
                        "name": "Current Remote or Host Screen PNG",
                        "description": "Latest desktop frame encoded as a PNG image.",
                        "mimeType": "image/png"
                    }
                ]
            }
        })),
        "resources/read" => {
            let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
            match uri {
                "ultradesk://status" => {
                    let text =
                        serde_json::to_string_pretty(&session.snapshot()).unwrap_or_default();
                    Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "contents": [{
                                "uri": uri,
                                "mimeType": "application/json",
                                "text": text
                            }]
                        }
                    }))
                }
                "ultradesk://host/status" => {
                    let text =
                        serde_json::to_string_pretty(&session.host_status()).unwrap_or_default();
                    Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "contents": [{
                                "uri": uri,
                                "mimeType": "application/json",
                                "text": text
                            }]
                        }
                    }))
                }
                "ultradesk://clipboard" => {
                    let snap = session.snapshot();
                    let text = serde_json::to_string_pretty(&json!({
                        "local_text": snap.clipboard_local,
                        "remote_text": snap.clipboard_remote,
                        "updates_sent": snap.clipboard_updates_sent,
                        "updates_received": snap.clipboard_updates_received
                    }))
                    .unwrap_or_default();
                    Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "contents": [{
                                "uri": uri,
                                "mimeType": "application/json",
                                "text": text
                            }]
                        }
                    }))
                }
                "ultradesk://screen/current.png" => {
                    let png = match session.png_frame_versioned() {
                        Ok((_, bytes)) => bytes,
                        Err(_) => session
                            .host_capture_image("png", 80)
                            .map(|(_, _, b)| b)
                            .unwrap_or_default(),
                    };
                    Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "contents": [{
                                "uri": uri,
                                "mimeType": "image/png",
                                "blob": encode_base64(&png)
                            }]
                        }
                    }))
                }
                _ => Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32602,
                        "message": format!("Unknown resource URI: {}", uri)
                    }
                })),
            }
        }
        "prompts/list" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "prompts": []
            }
        })),
        _ => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("Method not found: {}", method)
            }
        })),
    }
}

pub fn run_stdio_mcp_loop<R: BufRead, W: Write>(
    session: &ControlSession,
    mut reader: R,
    mut writer: W,
) -> std::io::Result<()> {
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(len_str) = trimmed
            .strip_prefix("Content-Length:")
            .or_else(|| trimmed.strip_prefix("content-length:"))
        {
            let content_len = len_str.trim().parse::<usize>().unwrap_or(0);
            loop {
                let mut hdr_line = String::new();
                let hn = reader.read_line(&mut hdr_line)?;
                if hn == 0 || hdr_line == "\r\n" || hdr_line == "\n" {
                    break;
                }
            }
            if content_len == 0 || content_len > 32 * 1024 * 1024 {
                continue;
            }
            let mut payload = vec![0u8; content_len];
            reader.read_exact(&mut payload)?;
            if let Some(resp) = handle_mcp_raw_json(session, &payload, McpTransportOrigin::Stdio) {
                let body = serde_json::to_vec(&resp).unwrap_or_else(|_| b"{}".to_vec());
                write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
                writer.write_all(&body)?;
                writer.flush()?;
            }
        } else if let Some(resp) =
            handle_mcp_raw_json(session, trimmed.as_bytes(), McpTransportOrigin::Stdio)
        {
            let body = serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string());
            writeln!(writer, "{}", body)?;
            writer.flush()?;
        }
    }
    Ok(())
}

pub fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let block_count = msg.len() / 64;
    for b_idx in 0..block_count {
        let chunk = &msg[b_idx * 64..(b_idx + 1) * 64];
        let mut w = [0u32; 80];
        for (i, slot) in w.iter_mut().take(16).enumerate() {
            let off = i * 4;
            *slot =
                u32::from_be_bytes([chunk[off], chunk[off + 1], chunk[off + 2], chunk[off + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

pub fn websocket_accept_key(client_key: &str) -> String {
    let mut input = String::with_capacity(client_key.trim().len() + 36);
    input.push_str(client_key.trim());
    input.push_str("258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    encode_base64(&sha1_digest(input.as_bytes()))
}

fn read_ws_frame<R: Read>(reader: &mut R) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    let mut header = [0u8; 2];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let opcode = header[0] & 0x0F;
    let masked = (header[1] & 0x80) != 0;
    let mut payload_len = (header[1] & 0x7F) as usize;
    if payload_len == 126 {
        let mut ext = [0u8; 2];
        reader.read_exact(&mut ext)?;
        payload_len = u16::from_be_bytes(ext) as usize;
    } else if payload_len == 127 {
        let mut ext = [0u8; 8];
        reader.read_exact(&mut ext)?;
        let len64 = u64::from_be_bytes(ext);
        if len64 > 32 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "WebSocket frame exceeds 32 MiB limit",
            ));
        }
        payload_len = len64 as usize;
    }
    let mut mask_key = [0u8; 4];
    if masked {
        reader.read_exact(&mut mask_key)?;
    }
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        reader.read_exact(&mut payload)?;
    }
    if masked {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask_key[i & 3];
        }
    }
    Ok(Some((opcode, payload)))
}

fn write_ws_frame<W: Write>(writer: &mut W, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut header = Vec::with_capacity(10);
    header.push(0x80 | (opcode & 0x0F));
    if payload.len() < 126 {
        header.push(payload.len() as u8);
    } else if payload.len() <= u16::MAX as usize {
        header.push(126);
        header.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    writer.write_all(&header)?;
    writer.write_all(payload)?;
    writer.flush()
}

pub struct McpServer {
    pub bind_addr: String,
    pub port: u16,
    pub session: ControlSession,
    pub shutdown: Arc<AtomicBool>,
    pub auth_token: Option<String>,
}

impl McpServer {
    pub fn bind(
        host: &str,
        port: u16,
        session: ControlSession,
        auth_token: Option<String>,
    ) -> Result<(Self, TcpListener), String> {
        let listener = TcpListener::bind(format!("{}:{}", host, port))
            .map_err(|e| format!("Failed to bind MCP server on {}:{}: {}", host, port, e))?;
        let local_addr = listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| format!("{}:{}", host, port));
        let bound_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        Ok((
            Self {
                bind_addr: local_addr,
                port: bound_port,
                session,
                shutdown: Arc::new(AtomicBool::new(false)),
                auth_token: auth_token.filter(|t| !t.trim().is_empty()),
            },
            listener,
        ))
    }

    pub fn spawn(self, listener: TcpListener) -> thread::JoinHandle<()> {
        let _ = listener.set_nonblocking(true);
        thread::spawn(move || {
            while !self.shutdown.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_nodelay(true);
                        let session = self.session.clone();
                        let token = self.auth_token.clone();
                        thread::spawn(move || {
                            let _ = handle_mcp_tcp_client(stream, session, token.as_deref());
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        })
    }
}

fn check_mcp_token(headers: &BTreeMap<String, String>, expected: Option<&str>) -> bool {
    let Some(expected_tok) = expected.filter(|s| !s.is_empty()) else {
        return true;
    };
    if let Some(hdr) = headers
        .get("x-mcp-token")
        .or_else(|| headers.get("x-viewer-token"))
    {
        if crate::host::ct_eq(hdr.trim(), expected_tok) {
            return true;
        }
    }
    if let Some(auth) = headers.get("authorization") {
        let bearer = auth
            .trim()
            .strip_prefix("Bearer ")
            .or_else(|| auth.trim().strip_prefix("bearer "))
            .unwrap_or(auth.trim());
        if crate::host::ct_eq(bearer.trim(), expected_tok) {
            return true;
        }
    }
    false
}

fn handle_mcp_tcp_client(
    mut stream: TcpStream,
    session: ControlSession,
    auth_token: Option<&str>,
) -> std::io::Result<()> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut first_line = String::new();
    if reader.read_line(&mut first_line)? == 0 {
        return Ok(());
    }
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }
    let method = parts[0];

    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    if !check_mcp_token(&headers, auth_token) {
        let body = b"{\"error\":\"Unauthorized MCP token\"}";
        let resp = format!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(resp.as_bytes())?;
        stream.write_all(body)?;
        return stream.flush();
    }

    let is_ws_upgrade = headers
        .get("upgrade")
        .is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    if is_ws_upgrade {
        let Some(ws_key) = headers.get("sec-websocket-key") else {
            return Ok(());
        };
        let accept = websocket_accept_key(ws_key);
        let handshake = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept
        );
        stream.write_all(handshake.as_bytes())?;
        stream.flush()?;
        let _ = stream.set_read_timeout(None);
        while let Some((opcode, payload)) = read_ws_frame(&mut reader)? {
            match opcode {
                0x1 | 0x2 => {
                    if let Some(resp) =
                        handle_mcp_raw_json(&session, &payload, McpTransportOrigin::Network)
                    {
                        let out = serde_json::to_vec(&resp).unwrap_or_else(|_| b"{}".to_vec());
                        write_ws_frame(&mut stream, 0x1, &out)?;
                    }
                }
                0x8 => {
                    let _ = write_ws_frame(&mut stream, 0x8, &[]);
                    break;
                }
                0x9 => {
                    write_ws_frame(&mut stream, 0xA, &payload)?;
                }
                _ => {}
            }
        }
        return Ok(());
    }

    let content_len = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if content_len > 32 * 1024 * 1024 {
        return Ok(());
    }
    let mut body = vec![0u8; content_len];
    if content_len > 0 {
        reader.read_exact(&mut body)?;
    }

    if method == "GET" {
        let info = serde_json::to_vec(&json!({
            "ok": true,
            "server": "ultradesk-mcp",
            "version": env!("CARGO_PKG_VERSION"),
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "tools": list_mcp_tools().len()
        }))
        .unwrap_or_default();
        let hdr = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            info.len()
        );
        stream.write_all(hdr.as_bytes())?;
        stream.write_all(&info)?;
        return stream.flush();
    }

    if method == "POST" {
        match handle_mcp_raw_json(&session, &body, McpTransportOrigin::Network) {
            Some(resp) => {
                let out = serde_json::to_vec(&resp).unwrap_or_else(|_| b"{}".to_vec());
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    out.len()
                );
                stream.write_all(hdr.as_bytes())?;
                stream.write_all(&out)?;
                return stream.flush();
            }
            None => {
                let hdr = "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                stream.write_all(hdr.as_bytes())?;
                return stream.flush();
            }
        }
    }

    Ok(())
}

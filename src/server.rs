use crate::input::InputEvent;
use crate::session::ControlSession;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

pub const DEFAULT_VIEWER_HTML: &str = include_str!("../viewer.html");
pub const DEFAULT_APP_ICON_PNG: &[u8] = include_bytes!("../ultradesk-rust.png");

pub use crate::gui::GuiLaunchOutcome;

pub fn materialize_embedded_gui_assets(target_dir: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(target_dir)
        .map_err(|e| format!("failed to create runtime GUI dir: {}", e))?;
    let viewer_path = target_dir.join("viewer.html");
    let icon_path = target_dir.join("ultradesk-rust.png");
    fs::write(&viewer_path, DEFAULT_VIEWER_HTML.as_bytes())
        .map_err(|e| format!("failed to write embedded viewer.html: {}", e))?;
    fs::write(&icon_path, DEFAULT_APP_ICON_PNG)
        .map_err(|e| format!("failed to write embedded icon: {}", e))?;
    Ok(viewer_path)
}

pub fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u8;
    for b in input.bytes() {
        if b.is_ascii_whitespace() || b == b'=' {
            continue;
        }
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return Err(format!("Invalid base64 character: {}", b as char)),
        };
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

pub fn encode_base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = if chunk.len() > 1 {
            chunk[1] as usize
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            chunk[2] as usize
        } else {
            0
        };
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((b1 & 0x0F) << 2) | (b2 >> 6)] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[b2 & 0x3F] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[derive(Deserialize, Default)]
struct ConnectRequest {
    #[serde(default)]
    id: String,
    #[serde(default)]
    pass: String,
    #[serde(default)]
    partner_id: String,
    #[serde(default)]
    password: String,
    #[serde(default = "default_true")]
    remember: bool,
}

#[derive(Deserialize, Default)]
struct AudioToggleRequest {
    #[serde(default = "default_true")]
    enable: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Default)]
struct ClipboardUpdateRequest {
    #[serde(default)]
    text: String,
    #[serde(default)]
    sync_from_system: bool,
}

#[derive(Deserialize, Default)]
struct FileSendRequest {
    #[serde(default)]
    file_name: String,
    #[serde(default)]
    data_base64: String,
    #[serde(default)]
    file_path: String,
}

#[derive(Deserialize, Default)]
struct FileActionRequest {
    #[serde(default)]
    file_id: String,
}

#[derive(Deserialize, Default)]
struct FileReceiveRequest {
    #[serde(default, alias = "file_path", alias = "path")]
    remote_path: String,
}

#[derive(Deserialize, Default)]
struct ChatSendRequest {
    #[serde(default)]
    text: String,
}

#[derive(Deserialize, Default)]
struct MonitorSelectRequest {
    #[serde(default, alias = "monitor")]
    index: Option<usize>,
    #[serde(default, alias = "monitors")]
    indices: Option<Vec<usize>>,
}

#[derive(Deserialize, Default)]
struct TextSendRequest {
    #[serde(default)]
    text: String,
    #[serde(default)]
    caps_lock: bool,
}

#[derive(Deserialize, Default)]
struct HostControlRequest {
    #[serde(default = "default_true")]
    allow_control: bool,
}

#[derive(Deserialize, Default)]
struct MasterPasswordRequest {
    #[serde(default)]
    password: Option<String>,
}

#[derive(Deserialize)]
struct MacPermissionRequest {
    kind: crate::host::MacOsPermissionKind,
}

#[derive(Debug, Default, Deserialize)]
struct PreferencesRequest {
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    toolbar_collapsed: Option<bool>,
}

pub const MAX_HTTP_LINE_BYTES: usize = 8 * 1024;
pub const MAX_HTTP_HEADERS: usize = 64;
pub const MAX_HTTP_HEADER_BLOCK_BYTES: usize = 16 * 1024;

pub struct PortalServer {
    pub bind_addr: String,
    pub port: u16,
    pub session: ControlSession,
    pub shutdown: Arc<AtomicBool>,
    pub viewer_token: String,
    viewer_path: Option<PathBuf>,
}

impl PortalServer {
    pub fn bind(
        host: &str,
        port: u16,
        session: ControlSession,
        viewer_path: Option<PathBuf>,
    ) -> Result<(Self, TcpListener), String> {
        let listener = TcpListener::bind(format!("{}:{}", host, port))
            .map_err(|e| format!("Failed to bind HTTP portal on {}:{}: {}", host, port, e))?;
        let local_addr = listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| format!("{}:{}", host, port));
        let bound_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        let viewer_token = crate::profile::generate_ephemeral_device_id().to_ascii_lowercase();
        Ok((
            Self {
                bind_addr: local_addr,
                port: bound_port,
                session,
                shutdown: Arc::new(AtomicBool::new(false)),
                viewer_token,
                viewer_path,
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
                        let viewer_path = self.viewer_path.clone();
                        let viewer_token = self.viewer_token.clone();
                        let bind_addr = self.bind_addr.clone();
                        thread::spawn(move || {
                            let _ = handle_http_client(
                                stream,
                                session,
                                viewer_path,
                                &viewer_token,
                                &bind_addr,
                            );
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        })
    }
}

fn parse_query_params(query: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut iter = pair.splitn(2, '=');
        let k = iter.next().unwrap_or_default().to_string();
        let v = iter.next().unwrap_or_default().to_string();
        map.insert(k, v);
    }
    map
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    extra_headers: &[(&str, &str)],
    body: &[u8],
    keep_alive: bool,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let conn_val = if keep_alive { "keep-alive" } else { "close" };
    let mut header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: {}\r\n",
        status,
        reason,
        content_type,
        body.len(),
        conn_val
    );
    for (k, v) in extra_headers {
        header.push_str(&format!("{}: {}\r\n", k, v));
    }
    header.push_str("\r\n");
    stream.write_all(header.as_bytes())?;
    if !body.is_empty() {
        stream.write_all(body)?;
    }
    stream.flush()
}

fn write_json_response(
    stream: &mut TcpStream,
    status: u16,
    value: &serde_json::Value,
    keep_alive: bool,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    write_http_response(
        stream,
        status,
        "application/json; charset=utf-8",
        &[],
        &body,
        keep_alive,
    )
}

#[derive(Debug)]
enum BoundedLineRead {
    Line(String),
    Eof,
    TooLarge,
    IoError,
}

fn read_bounded_line<R: BufRead>(reader: &mut R, max_len: usize) -> BoundedLineRead {
    let mut buf = Vec::new();
    match reader
        .take((max_len + 1) as u64)
        .read_until(b'\n', &mut buf)
    {
        Ok(0) => BoundedLineRead::Eof,
        Ok(_) => {
            if buf.len() > max_len && !buf.ends_with(b"\n") {
                BoundedLineRead::TooLarge
            } else {
                BoundedLineRead::Line(String::from_utf8_lossy(&buf).into_owned())
            }
        }
        Err(_) => BoundedLineRead::IoError,
    }
}

fn close_after_error_response<R: BufRead>(stream: &mut TcpStream, reader: &mut R) {
    let _ = stream.set_nonblocking(true);
    loop {
        match reader.fill_buf() {
            Ok(buf) if !buf.is_empty() => {
                let len = buf.len();
                reader.consume(len);
            }
            _ => break,
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn is_allowed_origin(origin: &str, bind_addr: &str) -> bool {
    let trimmed = origin.trim();
    if trimmed.is_empty() {
        return true;
    }
    let rest = match trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
    {
        Some(r) => r,
        None => return false,
    };
    let host_port = rest.split('/').next().unwrap_or("");
    if host_port.eq_ignore_ascii_case(bind_addr) {
        return true;
    }
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    host == "127.0.0.1" || host.eq_ignore_ascii_case("localhost") || host == "::1"
}

fn extract_cookie_token(cookie_header: &str) -> Option<&str> {
    for part in cookie_header.split(';') {
        if let Some((k, v)) = part.trim().split_once('=') {
            if k.trim() == "ultradesk_token" {
                return Some(v.trim());
            }
        }
    }
    None
}

pub(crate) fn handle_http_client(
    mut stream: TcpStream,
    session: ControlSession,
    viewer_path: Option<PathBuf>,
    viewer_token: &str,
    bind_addr: &str,
) -> std::io::Result<()> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut reader = BufReader::new(stream.try_clone()?);
    loop {
        let request_line = match read_bounded_line(&mut reader, MAX_HTTP_LINE_BYTES) {
            BoundedLineRead::Line(line) => {
                if line == "\r\n" || line == "\n" {
                    continue;
                }
                line
            }
            BoundedLineRead::Eof | BoundedLineRead::IoError => break,
            BoundedLineRead::TooLarge => {
                let _ = write_json_response(
                    &mut stream,
                    431,
                    &json!({ "ok": false, "error": "Request line too large" }),
                    false,
                );
                close_after_error_response(&mut stream, &mut reader);
                break;
            }
        };
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            break;
        }
        let method = parts[0];
        let raw_target = parts[1];
        let http_version = if parts.len() >= 3 {
            parts[2]
        } else {
            "HTTP/1.1"
        };
        let (path, query_str) = match raw_target.split_once('?') {
            Some((p, q)) => (p, q),
            None => (raw_target, ""),
        };
        let query = parse_query_params(query_str);

        let mut content_length = 0usize;
        let mut connection_header: Option<String> = None;
        let mut content_type: Option<String> = None;
        let mut origin_header: Option<String> = None;
        let mut sec_fetch_site: Option<String> = None;
        let mut x_viewer_token: Option<String> = None;
        let mut cookie_token: Option<String> = None;
        let mut header_read_err = false;
        let mut header_too_large = false;
        let mut header_count = 0usize;
        let mut total_header_bytes = request_line.len();
        loop {
            match read_bounded_line(&mut reader, MAX_HTTP_LINE_BYTES) {
                BoundedLineRead::Eof | BoundedLineRead::IoError => {
                    header_read_err = true;
                    break;
                }
                BoundedLineRead::TooLarge => {
                    header_too_large = true;
                    break;
                }
                BoundedLineRead::Line(line) => {
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                    header_count += 1;
                    total_header_bytes = total_header_bytes.saturating_add(line.len());
                    if header_count > MAX_HTTP_HEADERS
                        || total_header_bytes > MAX_HTTP_HEADER_BLOCK_BYTES
                    {
                        header_too_large = true;
                        break;
                    }
                    if let Some((name, val)) = line.split_once(':') {
                        let name_trim = name.trim();
                        let val_trim = val.trim();
                        if name_trim.eq_ignore_ascii_case("content-length") {
                            content_length = val_trim.parse::<usize>().unwrap_or(0);
                        } else if name_trim.eq_ignore_ascii_case("connection") {
                            connection_header = Some(val_trim.to_lowercase());
                        } else if name_trim.eq_ignore_ascii_case("content-type") {
                            content_type = Some(val_trim.to_lowercase());
                        } else if name_trim.eq_ignore_ascii_case("origin") {
                            origin_header = Some(val_trim.to_string());
                        } else if name_trim.eq_ignore_ascii_case("sec-fetch-site") {
                            sec_fetch_site = Some(val_trim.to_lowercase());
                        } else if name_trim.eq_ignore_ascii_case("x-viewer-token") {
                            x_viewer_token = Some(val_trim.to_string());
                        } else if name_trim.eq_ignore_ascii_case("cookie") {
                            if let Some(tok) = extract_cookie_token(val_trim) {
                                cookie_token = Some(tok.to_string());
                            }
                        }
                    }
                }
            }
        }
        if header_too_large {
            let _ = write_json_response(
                &mut stream,
                431,
                &json!({ "ok": false, "error": "Request header fields too large" }),
                false,
            );
            close_after_error_response(&mut stream, &mut reader);
            break;
        }
        if header_read_err {
            break;
        }

        let is_http_10 = http_version.starts_with("HTTP/1.0");
        let keep_alive = match connection_header.as_deref() {
            Some("close") => false,
            Some("keep-alive") => true,
            _ => !is_http_10,
        };

        if content_length > 32 * 1024 * 1024 {
            let _ = write_json_response(
                &mut stream,
                413,
                &json!({ "ok": false, "error": "Payload too large" }),
                false,
            );
            break;
        }
        let mut body = vec![0u8; content_length];
        if !body.is_empty() && reader.read_exact(&mut body).is_err() {
            break;
        }

        let is_public_asset = matches!(
            (method, path),
            (
                "GET",
                "/" | "/index.html"
                    | "/viewer.html"
                    | "/favicon.ico"
                    | "/ultradesk-rust.png"
                    | "/ultraviewer-linux.png"
            )
        );
        if let Some(ref sfs) = sec_fetch_site {
            if !is_public_asset && sfs != "same-origin" && sfs != "none" {
                let _ = write_json_response(
                    &mut stream,
                    403,
                    &json!({ "ok": false, "error": "Cross-site request blocked" }),
                    false,
                );
                break;
            }
        }
        if let Some(ref orig) = origin_header {
            if !is_allowed_origin(orig, bind_addr) {
                let _ = write_json_response(
                    &mut stream,
                    403,
                    &json!({ "ok": false, "error": "Origin not allowed" }),
                    false,
                );
                break;
            }
        }
        if !is_public_asset {
            let token_ok = x_viewer_token
                .as_deref()
                .is_some_and(|t| crate::host::ct_eq(t, viewer_token))
                || cookie_token
                    .as_deref()
                    .is_some_and(|t| crate::host::ct_eq(t, viewer_token));
            if !token_ok {
                let _ = write_json_response(
                    &mut stream,
                    401,
                    &json!({ "ok": false, "error": "Unauthorized viewer token" }),
                    false,
                );
                break;
            }
        }
        if method == "POST" {
            let is_json = content_type
                .as_deref()
                .is_some_and(|ct| ct.starts_with("application/json"));
            if !is_json {
                let _ = write_json_response(
                    &mut stream,
                    415,
                    &json!({ "ok": false, "error": "Content-Type must be application/json" }),
                    keep_alive,
                );
                if !keep_alive {
                    break;
                }
                continue;
            }
        }

        let res = match (method, path) {
            ("GET", "/" | "/index.html" | "/viewer.html") => {
                let raw_html = viewer_path
                    .as_ref()
                    .and_then(|p| fs::read_to_string(p).ok())
                    .unwrap_or_else(|| DEFAULT_VIEWER_HTML.to_string());
                let html = raw_html.replace("__TOKEN__", viewer_token).into_bytes();
                let cookie_hdr = format!(
                    "ultradesk_token={}; HttpOnly; SameSite=Strict; Path=/",
                    viewer_token
                );
                write_http_response(
                    &mut stream,
                    200,
                    "text/html; charset=utf-8",
                    &[("Set-Cookie", &cookie_hdr)],
                    &html,
                    keep_alive,
                )
            }
            ("GET", "/favicon.ico" | "/ultradesk-rust.png" | "/ultraviewer-linux.png") => {
                write_http_response(
                    &mut stream,
                    200,
                    "image/png",
                    &[],
                    DEFAULT_APP_ICON_PNG,
                    keep_alive,
                )
            }
            ("GET", "/status" | "/state") => {
                let state = session.snapshot();
                let val = serde_json::to_value(state).unwrap_or_else(|_| json!({}));
                write_json_response(&mut stream, 200, &val, keep_alive)
            }
            ("GET", "/frame.png") => {
                let not_modified = query
                    .get("v")
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(|v| v == session.frame_version())
                    .unwrap_or(false);
                if not_modified {
                    let v_str = session.frame_version().to_string();
                    write_http_response(
                        &mut stream,
                        304,
                        "image/png",
                        &[("X-Frame-Version", &v_str)],
                        &[],
                        keep_alive,
                    )
                } else {
                    match session.png_frame_versioned() {
                        Ok((ver, png)) => {
                            let v_str = ver.to_string();
                            write_http_response(
                                &mut stream,
                                200,
                                "image/png",
                                &[("X-Frame-Version", &v_str)],
                                &png,
                                keep_alive,
                            )
                        }
                        Err(e) => write_json_response(
                            &mut stream,
                            500,
                            &json!({ "ok": false, "error": e }),
                            keep_alive,
                        ),
                    }
                }
            }
            ("GET", "/frame.jpg") => {
                let quality = query
                    .get("q")
                    .and_then(|s| s.parse::<u8>().ok())
                    .unwrap_or(80);
                let not_modified = query
                    .get("v")
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(|v| v == session.frame_version())
                    .unwrap_or(false);
                if not_modified {
                    let v_str = session.frame_version().to_string();
                    write_http_response(
                        &mut stream,
                        304,
                        "image/jpeg",
                        &[("X-Frame-Version", &v_str)],
                        &[],
                        keep_alive,
                    )
                } else {
                    match session.jpeg_frame_versioned(quality) {
                        Ok((ver, jpg)) => {
                            let v_str = ver.to_string();
                            write_http_response(
                                &mut stream,
                                200,
                                "image/jpeg",
                                &[("X-Frame-Version", &v_str)],
                                &jpg,
                                keep_alive,
                            )
                        }
                        Err(e) => write_json_response(
                            &mut stream,
                            500,
                            &json!({ "ok": false, "error": e }),
                            keep_alive,
                        ),
                    }
                }
            }
            ("GET", "/frame.bmp") => {
                let not_modified = query
                    .get("v")
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(|v| v == session.frame_version())
                    .unwrap_or(false);
                if not_modified {
                    let v_str = session.frame_version().to_string();
                    write_http_response(
                        &mut stream,
                        304,
                        "image/bmp",
                        &[("X-Frame-Version", &v_str)],
                        &[],
                        keep_alive,
                    )
                } else {
                    let (ver, bmp) = session.bmp_frame_versioned();
                    let v_str = ver.to_string();
                    write_http_response(
                        &mut stream,
                        200,
                        "image/bmp",
                        &[("X-Frame-Version", &v_str)],
                        &bmp,
                        keep_alive,
                    )
                }
            }
            ("GET", "/audio.wav" | "/audio/stream.wav") => {
                let after_seq = query
                    .get("after")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                let (latest_seq, wav) = session.audio_wav_since(after_seq);
                let seq_str = latest_seq.to_string();
                write_http_response(
                    &mut stream,
                    200,
                    "audio/wav",
                    &[("X-Audio-Seq", &seq_str)],
                    &wav,
                    keep_alive,
                )
            }
            ("GET", "/clipboard") => {
                let snap = session.snapshot();
                write_json_response(
                    &mut stream,
                    200,
                    &json!({
                        "ok": true,
                        "local_text": snap.clipboard_local,
                        "remote_text": snap.clipboard_remote,
                        "updates_sent": snap.clipboard_updates_sent,
                        "updates_received": snap.clipboard_updates_received
                    }),
                    keep_alive,
                )
            }
            ("GET", "/files") => {
                let snap = session.snapshot();
                write_json_response(
                    &mut stream,
                    200,
                    &json!({
                        "ok": true,
                        "transfers": snap.file_transfers,
                        "chat_messages": snap.chat_messages
                    }),
                    keep_alive,
                )
            }
            ("GET", "/files/download") => {
                let file_id = query.get("file_id").cloned().unwrap_or_default();
                if let Some((name, bytes)) = session.downloaded_file_bytes(&file_id) {
                    let disp = format!("attachment; filename=\"{}\"", name.replace('"', ""));
                    write_http_response(
                        &mut stream,
                        200,
                        "application/octet-stream",
                        &[("Content-Disposition", &disp)],
                        &bytes,
                        keep_alive,
                    )
                } else {
                    write_json_response(
                        &mut stream,
                        404,
                        &json!({ "ok": false, "error": "File transfer not found" }),
                        keep_alive,
                    )
                }
            }
            ("POST", "/connect") => {
                let req: ConnectRequest = serde_json::from_slice(&body).unwrap_or_default();
                let pid = if !req.id.trim().is_empty() {
                    &req.id
                } else {
                    &req.partner_id
                };
                let pwd = if !req.pass.trim().is_empty() {
                    &req.pass
                } else {
                    &req.password
                };
                match session.start_connect_with_remember(pid, pwd, req.remember) {
                    Ok(state) => write_json_response(
                        &mut stream,
                        200,
                        &json!({ "ok": true, "state": state }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("GET", "/credentials") => {
                let snap = session.snapshot();
                write_json_response(
                    &mut stream,
                    200,
                    &json!({
                        "ok": true,
                        "partner_id": snap.saved_partner_id,
                        "password": snap.saved_password,
                        "remember": snap.remember_credentials,
                        "saved_remotes": snap.saved_remotes,
                        "theme": snap.saved_theme,
                        "language": snap.saved_language,
                        "toolbar_collapsed": snap.saved_toolbar_collapsed,
                    }),
                    keep_alive,
                )
            }
            ("POST", "/credentials") => {
                let req: ConnectRequest = serde_json::from_slice(&body).unwrap_or_default();
                let pid = if !req.id.trim().is_empty() {
                    &req.id
                } else {
                    &req.partner_id
                };
                let pwd = if !req.pass.trim().is_empty() {
                    &req.pass
                } else {
                    &req.password
                };
                let _ = session.update_saved_credentials(pid, pwd, req.remember);
                write_json_response(
                    &mut stream,
                    200,
                    &json!({ "ok": true, "state": session.snapshot() }),
                    keep_alive,
                )
            }
            ("POST", "/preferences") => {
                let req: PreferencesRequest = match serde_json::from_slice(&body) {
                    Ok(v) => v,
                    Err(e) => {
                        write_json_response(
                            &mut stream,
                            400,
                            &json!({ "ok": false, "error": format!("Invalid preferences JSON: {}", e) }),
                            keep_alive,
                        )?;
                        if !keep_alive {
                            break;
                        }
                        continue;
                    }
                };
                match session.update_ui_preferences(
                    req.theme.as_deref(),
                    req.language.as_deref(),
                    req.toolbar_collapsed,
                ) {
                    Ok(()) => write_json_response(
                        &mut stream,
                        200,
                        &json!({ "ok": true, "state": session.snapshot() }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/disconnect") => {
                session.disconnect();
                write_json_response(
                    &mut stream,
                    200,
                    &json!({ "ok": true, "state": session.snapshot() }),
                    keep_alive,
                )
            }
            ("POST", "/event" | "/input") => {
                let evt: InputEvent = match serde_json::from_slice(&body) {
                    Ok(v) => v,
                    Err(e) => {
                        write_json_response(
                            &mut stream,
                            400,
                            &json!({ "ok": false, "error": format!("Invalid input JSON: {}", e) }),
                            keep_alive,
                        )?;
                        if !keep_alive {
                            break;
                        }
                        continue;
                    }
                };
                match session.send_input(evt) {
                    Ok(wire) => write_json_response(
                        &mut stream,
                        200,
                        &json!({ "ok": true, "bytes_sent": wire.len() }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/audio/toggle") => {
                let req: AudioToggleRequest = serde_json::from_slice(&body).unwrap_or_default();
                match session.toggle_remote_sound(req.enable) {
                    Ok(wire) => write_json_response(
                        &mut stream,
                        200,
                        &json!({
                            "ok": true,
                            "audio_enabled": req.enable,
                            "bytes_sent": wire.len(),
                            "state": session.snapshot()
                        }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/clipboard") => {
                let req: ClipboardUpdateRequest = serde_json::from_slice(&body).unwrap_or_default();
                let res = if req.sync_from_system {
                    session.sync_from_system_clipboard().map(|t| (t, true))
                } else {
                    session
                        .send_clipboard(&req.text, true)
                        .map(|_| (req.text.clone(), false))
                };
                match res {
                    Ok((text, from_sys)) => write_json_response(
                        &mut stream,
                        200,
                        &json!({
                            "ok": true,
                            "text": text,
                            "synced_from_system": from_sys,
                            "state": session.snapshot()
                        }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/files/send") => {
                let req: FileSendRequest = serde_json::from_slice(&body).unwrap_or_default();
                let result = if !req.file_path.trim().is_empty() {
                    Err(
                        "Direct filesystem paths are not allowed over HTTP; use data_base64"
                            .to_string(),
                    )
                } else {
                    match decode_base64(&req.data_base64) {
                        Ok(bytes) => {
                            let name = if req.file_name.trim().is_empty() {
                                "upload.bin"
                            } else {
                                req.file_name.trim()
                            };
                            session.offer_file_bytes(name, bytes)
                        }
                        Err(e) => Err(e),
                    }
                };
                match result {
                    Ok(file_id) => write_json_response(
                        &mut stream,
                        200,
                        &json!({
                            "ok": true,
                            "file_id": file_id,
                            "state": session.snapshot()
                        }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/files/accept") => {
                let req: FileActionRequest = serde_json::from_slice(&body).unwrap_or_default();
                match session.accept_file(&req.file_id) {
                    Ok(_) => write_json_response(
                        &mut stream,
                        200,
                        &json!({ "ok": true, "state": session.snapshot() }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/files/cancel") => {
                let req: FileActionRequest = serde_json::from_slice(&body).unwrap_or_default();
                match session.cancel_file(&req.file_id) {
                    Ok(_) => write_json_response(
                        &mut stream,
                        200,
                        &json!({ "ok": true, "state": session.snapshot() }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/files/receive") => {
                let req: FileReceiveRequest = serde_json::from_slice(&body).unwrap_or_default();
                match session.request_remote_file(&req.remote_path) {
                    Ok(wire) => write_json_response(
                        &mut stream,
                        200,
                        &json!({
                            "ok": true,
                            "remote_path": req.remote_path.trim(),
                            "bytes_sent": wire.len(),
                            "state": session.snapshot()
                        }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/chat") => {
                let req: ChatSendRequest = serde_json::from_slice(&body).unwrap_or_default();
                match session.send_chat(&req.text) {
                    Ok(_) => write_json_response(
                        &mut stream,
                        200,
                        &json!({ "ok": true, "state": session.snapshot() }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/monitor") => {
                let req: MonitorSelectRequest = serde_json::from_slice(&body).unwrap_or_default();
                let state = if let Some(ref indices) = req.indices {
                    session.select_monitors(Some(indices.as_slice()))
                } else {
                    session.select_monitor(req.index)
                };
                write_json_response(
                    &mut stream,
                    200,
                    &json!({ "ok": true, "state": state }),
                    keep_alive,
                )
            }
            ("POST", "/host/monitor") => {
                let req: MonitorSelectRequest = serde_json::from_slice(&body).unwrap_or_default();
                let host = session.host_select_monitors(req.index, req.indices.as_deref());
                write_json_response(
                    &mut stream,
                    200,
                    &json!({ "ok": true, "host": host, "state": session.snapshot() }),
                    keep_alive,
                )
            }
            ("POST", "/text") => {
                let req: TextSendRequest = serde_json::from_slice(&body).unwrap_or_default();
                match session.send_text(&req.text, req.caps_lock) {
                    Ok(packets_sent) => write_json_response(
                        &mut stream,
                        200,
                        &json!({
                            "ok": true,
                            "packets_sent": packets_sent,
                            "state": session.snapshot()
                        }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("GET", "/host/status") => {
                let host = session.host_status();
                write_json_response(
                    &mut stream,
                    200,
                    &json!({ "ok": true, "host": host }),
                    keep_alive,
                )
            }
            ("POST", "/host/start") => {
                let host = session.host_start();
                write_json_response(
                    &mut stream,
                    200,
                    &json!({ "ok": true, "host": host }),
                    keep_alive,
                )
            }
            ("POST", "/host/stop") => {
                let host = session.host_stop();
                write_json_response(
                    &mut stream,
                    200,
                    &json!({ "ok": true, "host": host }),
                    keep_alive,
                )
            }
            ("POST", "/host/approve") => {
                let req: HostControlRequest = serde_json::from_slice(&body).unwrap_or_default();
                match session.host_approve(req.allow_control) {
                    Ok(host) => write_json_response(
                        &mut stream,
                        200,
                        &json!({ "ok": true, "host": host }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/host/decline") => match session.host_decline() {
                Ok(host) => write_json_response(
                    &mut stream,
                    200,
                    &json!({ "ok": true, "host": host }),
                    keep_alive,
                ),
                Err(e) => write_json_response(
                    &mut stream,
                    400,
                    &json!({ "ok": false, "error": e }),
                    keep_alive,
                ),
            },
            ("POST", "/host/control") => {
                let req: HostControlRequest = serde_json::from_slice(&body).unwrap_or_default();
                match session.host_set_control(req.allow_control) {
                    Ok(host) => write_json_response(
                        &mut stream,
                        200,
                        &json!({ "ok": true, "host": host }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/host/master-password") => {
                let req: MasterPasswordRequest = serde_json::from_slice(&body).unwrap_or_default();
                match session.host_set_master_password(req.password.as_deref()) {
                    Ok(host) => write_json_response(
                        &mut stream,
                        200,
                        &json!({ "ok": true, "host": host }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/host/permissions/request") => {
                let req: MacPermissionRequest = match serde_json::from_slice(&body) {
                    Ok(v) => v,
                    Err(e) => {
                        write_json_response(
                            &mut stream,
                            400,
                            &json!({ "ok": false, "error": format!("Invalid permission request JSON: {}", e) }),
                            keep_alive,
                        )?;
                        if !keep_alive {
                            break;
                        }
                        continue;
                    }
                };
                let host = session.host_request_macos_permission(req.kind);
                write_json_response(
                    &mut stream,
                    200,
                    &json!({ "ok": true, "host": host, "state": session.snapshot() }),
                    keep_alive,
                )
            }
            ("POST", "/host/permissions/settings") => {
                let req: MacPermissionRequest = match serde_json::from_slice(&body) {
                    Ok(v) => v,
                    Err(e) => {
                        write_json_response(
                            &mut stream,
                            400,
                            &json!({ "ok": false, "error": format!("Invalid permission settings JSON: {}", e) }),
                            keep_alive,
                        )?;
                        if !keep_alive {
                            break;
                        }
                        continue;
                    }
                };
                match session.host_open_macos_permission_settings(req.kind) {
                    Ok((pane, host)) => write_json_response(
                        &mut stream,
                        200,
                        &json!({ "ok": true, "pane": pane, "host": host, "state": session.snapshot() }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/update/check") => {
                if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&body) {
                    if let Some(api_url) = val.get("api_url").and_then(serde_json::Value::as_str) {
                        let trimmed = api_url.trim();
                        if crate::updater::is_loopback_update_url(trimmed) {
                            session.set_update_api_url(trimmed.to_string());
                        }
                    }
                }
                match session.check_for_updates() {
                    Ok(snap) => write_json_response(
                        &mut stream,
                        200,
                        &json!({ "ok": true, "update_status": snap }),
                        keep_alive,
                    ),
                    Err(e) => write_json_response(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": e, "update_status": session.update_snapshot() }),
                        keep_alive,
                    ),
                }
            }
            ("POST", "/update/confirm" | "/update/apply") => match session.confirm_update() {
                Ok(msg) => write_json_response(
                    &mut stream,
                    200,
                    &json!({ "ok": true, "message": msg, "update_status": session.update_snapshot() }),
                    keep_alive,
                ),
                Err(e) => write_json_response(
                    &mut stream,
                    400,
                    &json!({ "ok": false, "error": e, "update_status": session.update_snapshot() }),
                    keep_alive,
                ),
            },
            ("POST", "/update/later" | "/update/dismiss") => {
                let ver = if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&body) {
                    val.get("version")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                } else {
                    None
                };
                session.dismiss_update(ver.as_deref());
                write_json_response(
                    &mut stream,
                    200,
                    &json!({ "ok": true, "update_status": session.update_snapshot() }),
                    keep_alive,
                )
            }
            ("GET", "/mcp" | "/mcp/tools") => {
                let tools = crate::mcp::list_mcp_tools();
                write_json_response(
                    &mut stream,
                    200,
                    &json!({
                        "ok": true,
                        "server": "ultradesk-mcp",
                        "version": env!("CARGO_PKG_VERSION"),
                        "protocolVersion": crate::mcp::MCP_PROTOCOL_VERSION,
                        "tools": tools
                    }),
                    keep_alive,
                )
            }
            ("POST", "/mcp") => match crate::mcp::handle_mcp_raw_json(
                &session,
                &body,
                crate::mcp::McpTransportOrigin::Network,
            ) {
                Some(resp) => write_json_response(&mut stream, 200, &resp, keep_alive),
                None => write_http_response(
                    &mut stream,
                    202,
                    "application/json; charset=utf-8",
                    &[],
                    &[],
                    keep_alive,
                ),
            },
            _ => write_json_response(
                &mut stream,
                404,
                &json!({ "error": "Not found" }),
                keep_alive,
            ),
        };

        if res.is_err() || !keep_alive {
            break;
        }
    }
    Ok(())
}

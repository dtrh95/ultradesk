use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use ultradesk_rust::audio::{encode_audio_packet, AudioBuffer, WaveSource};
use ultradesk_rust::clipboard::{encode_clipboard_modern_packet, ClipboardSyncMode};
use ultradesk_rust::cursor::decode_cur_to_rgba;
use ultradesk_rust::file_transfer::{
    decode_file_transfer_packet, encode_file_chunk, encode_file_offer, FileTransferManager,
    FileTransferPacket,
};
use ultradesk_rust::host::{compute_master_password_proof, HostAction, HostCore, HostPhase};
use ultradesk_rust::input::InputEvent;
use ultradesk_rust::mcp::{handle_mcp_raw_json, McpServer, McpTransportOrigin};
use ultradesk_rust::protocol::{
    decode_target_route, encode_byte_array_fields, is_public_routable_ipv4, md5_hex_lower,
    TargetRoute,
};
use ultradesk_rust::screen::{
    DisplayLayout, MonitorRect, RGB555Screen, ZstdDictionaryDecoder,
};
use ultradesk_rust::server::{encode_base64, PortalServer};
use ultradesk_rust::session::{
    ControlSession, RegistrationProfile, DEFAULT_PARTNER_JSON, DEFAULT_REGISTRATION_JSON,
};

fn build_valid_16x16_cur_png() -> Vec<u8> {
    let mut rgba = vec![0u8; 16 * 16 * 4];
    for px in rgba.chunks_exact_mut(4) {
        px.copy_from_slice(&[0x20, 0x90, 0xF0, 0xFF]);
    }
    let mut png = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut png);
    image::ImageEncoder::write_image(
        encoder,
        &rgba,
        16,
        16,
        image::ExtendedColorType::Rgba8,
    )
    .unwrap();

    let mut cur = Vec::with_capacity(22 + png.len());
    cur.extend_from_slice(&0u16.to_le_bytes());
    cur.extend_from_slice(&2u16.to_le_bytes());
    cur.extend_from_slice(&1u16.to_le_bytes());
    cur.push(16);
    cur.push(16);
    cur.push(0);
    cur.push(0);
    cur.extend_from_slice(&2u16.to_le_bytes());
    cur.extend_from_slice(&3u16.to_le_bytes());
    cur.extend_from_slice(&(png.len() as u32).to_le_bytes());
    cur.extend_from_slice(&22u32.to_le_bytes());
    cur.extend_from_slice(&png);
    cur
}

fn build_valid_8x8_jpeg() -> Vec<u8> {
    let rgb = vec![0x66u8; 8 * 8 * 3];
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
    image::ImageEncoder::write_image(encoder, &rgb, 8, 8, image::ExtendedColorType::Rgb8).unwrap();
    out
}

#[test]
fn test_simulated_ultraviewer_windows_6_6_133_full_session_compatibility() {
    let reg: RegistrationProfile = serde_json::from_str(DEFAULT_REGISTRATION_JSON).unwrap();
    assert_eq!(reg.terms.get(8).map(String::as_str), Some("BC241128BDE1"));

    let default_partner: Value = serde_json::from_str(DEFAULT_PARTNER_JSON).unwrap();
    assert_eq!(default_partner["request_terms"].as_array().map(Vec::len), Some(17));
    assert_eq!(default_partner["request_terms"][1], json!(""));
    assert_eq!(default_partner["password"], json!(""));

    assert!(is_public_routable_ipv4("103.56.158.44".parse().unwrap()));
    assert!(!is_public_routable_ipv4("127.0.0.1".parse().unwrap()));
    assert!(!is_public_routable_ipv4("10.0.0.5".parse().unwrap()));
    assert!(!is_public_routable_ipv4("169.254.169.254".parse().unwrap()));
    assert!(!is_public_routable_ipv4("100.64.0.1".parse().unwrap()));

    let relay_route_raw = b"8:1:192.168.1.25:0:113.161.72.10:51234:5938:0:V2luZG93c1BD:54321:0:103.56.158.44:0:7:functions3.ultraviewer.net:tok123";
    let route = decode_target_route(relay_route_raw).unwrap();
    assert_eq!(
        route,
        TargetRoute {
            lan_ip: "192.168.1.25".to_string(),
            public_ip: "113.161.72.10".to_string(),
            public_port: 51234,
            lan_port: 5938,
            command_port: 0,
            computer_name_encoded: "V2luZG93c1BD".to_string(),
            tcp_port: 54321,
            tcp_server_ip: "103.56.158.44".to_string(),
            matched_custom_password: 0,
            target_session_id: 7,
            tcp_server_hostname: "functions3.ultraviewer.net".to_string(),
            partner_secret_token: Some("tok123".to_string()),
        }
    );

    let loopback_relay_raw = b"8:1:127.0.0.1:0:127.0.0.1:51234:5938:0:V2luZG93c1BD:54321:0:127.0.0.1:0:7:functions3.ultraviewer.net:tok123";
    assert!(decode_target_route(loopback_relay_raw).is_err());

    let linklocal_relay_raw = b"8:1:192.168.1.25:0:113.161.72.10:51234:5938:0:V2luZG93c1BD:54321:0:169.254.169.254:0:7:functions3.ultraviewer.net:tok123";
    assert!(decode_target_route(linklocal_relay_raw).is_err());

    let md5_proof = md5_hex_lower(b"8765432155667788");
    assert_eq!(md5_proof.len(), 32);

    let session = ControlSession::new();
    session.set_clipboard_sync_mode(ClipboardSyncMode::Off);

    let init_pkt = encode_byte_array_fields(&[
        &[0u8][..],
        &3840i32.to_le_bytes(),
        &1080i32.to_le_bytes(),
        &[][..],
        &[][..],
        &[0u8][..],
        b"6.6.133",
        &[2u8][..],
        b"-1920:0:0:1080|0:0:1920:1080|\0",
        &(-1920i32).to_le_bytes(),
        &0i32.to_le_bytes(),
    ])
    .unwrap();
    session.handle_session_payload(&init_pkt);

    let jpeg_tile = build_valid_8x8_jpeg();
    let rect_hdr = [
        0i16.to_le_bytes(),
        0i16.to_le_bytes(),
        8i16.to_le_bytes(),
        8i16.to_le_bytes(),
    ]
    .concat();
    let screen_pkt = encode_byte_array_fields(&[
        &[1u8][..],
        &[][..],
        &[][..],
        &rect_hdr,
        &encode_byte_array_fields(&[&jpeg_tile]).unwrap(),
        &[][..],
        &[][..],
        &[][..],
        &[][..],
        &[][..],
        &[][..],
    ])
    .unwrap();
    session.handle_session_payload(&screen_pkt);

    let cur_bytes = build_valid_16x16_cur_png();
    let cur_zstd = zstd::bulk::compress(&cur_bytes, 3).unwrap();
    let cursor_shape_pkt = encode_byte_array_fields(&[
        &[4u8][..],
        &424242i32.to_le_bytes(),
        &(cur_bytes.len() as i32).to_le_bytes(),
        &cur_zstd,
    ])
    .unwrap();
    session.handle_session_payload(&cursor_shape_pkt);

    let cursor_sel_pkt =
        encode_byte_array_fields(&[&[3u8][..], &424242i32.to_le_bytes()]).unwrap();
    session.handle_session_payload(&cursor_sel_pkt);

    let audio_pkt = encode_audio_packet(WaveSource::PcSound, &[0x55u8; 60], false).unwrap();
    session.handle_session_payload(&audio_pkt);

    session.handle_session_payload(b"12|||Hello from UltraViewer Windows 6.6.133");

    let clip_pkt = encode_clipboard_modern_packet(1, "UltraViewer Windows Clipboard").unwrap();
    session.handle_session_payload(&clip_pkt);

    let offer_pkt = encode_file_offer("uv_win_doc", "report.txt", 18, false).unwrap();
    session.handle_session_payload(&offer_pkt);

    let snap = session.snapshot();
    assert_eq!(snap.state, "connected");
    assert!(!snap.e2e_enabled);
    assert_eq!(snap.screen_width, 3840);
    assert_eq!(snap.screen_height, 1080);
    assert_eq!(snap.monitors.len(), 2);
    assert_eq!(snap.monitors[0].0, -1920);
    assert_eq!(snap.frames, 1);
    assert!(snap
        .cursor_png_base64
        .as_deref()
        .is_some_and(|s| !s.is_empty()));
    assert_eq!(snap.clipboard_remote, "UltraViewer Windows Clipboard");
    assert!(snap
        .chat_messages
        .iter()
        .any(|m| m.contains("Hello from UltraViewer Windows 6.6.133")));

    let accept_wire = session.accept_file("uv_win_doc").unwrap();
    assert!(matches!(
        decode_file_transfer_packet(&accept_wire),
        Some(FileTransferPacket::Accept { ref file_id, .. }) if file_id == "uv_win_doc"
    ));

    let chunk_pkt = encode_file_chunk("uv_win_doc", 0, b"UV_WIN_6_6_133_DOC", false).unwrap();
    session.handle_session_payload(&chunk_pkt);

    let (dl_name, dl_bytes) = session.downloaded_file_bytes("uv_win_doc").unwrap();
    assert_eq!(dl_name, "report.txt");
    assert_eq!(dl_bytes, b"UV_WIN_6_6_133_DOC");

    let move_wire = session
        .send_input(InputEvent {
            event_type: "move".to_string(),
            x: 1420,
            y: 300,
            ..Default::default()
        })
        .unwrap();
    let move_str = String::from_utf8_lossy(&move_wire);
    assert!(
        move_str.contains("-500") || move_wire.windows(4).any(|w| w == (-500i32).to_le_bytes()),
        "expected negative-coordinate mouse move (-500) across dual-monitor layout, got {:?}",
        move_str
    );

    session.disconnect();
}

fn raw_http(port: u16, req: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let _ = s.write_all(req.as_bytes());
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).to_string()
}

#[test]
fn test_portal_http_pre_body_guards_bootstrap_cookie_and_header_hardening() {
    let session = ControlSession::new();
    let login_pkt = encode_byte_array_fields(&[
        b"\x00\x00",
        &1280i32.to_le_bytes(),
        &720i32.to_le_bytes(),
    ])
    .unwrap();
    session.handle_session_payload(&login_pkt);

    let (mut portal, listener) =
        PortalServer::bind("127.0.0.1", 0, session.clone(), None).unwrap();
    let bootstrap = portal.enable_gui_bootstrap();
    let port = portal.port;
    let viewer_token = portal.viewer_token.clone();
    let shutdown = portal.shutdown.clone();
    let handle = portal.spawn(listener);

    let rebind_resp = raw_http(
        port,
        "GET / HTTP/1.1\r\nHost: evil.attacker.example\r\nConnection: close\r\n\r\n",
    );
    assert!(
        rebind_resp.starts_with("HTTP/1.1 403 Forbidden"),
        "expected 403 on DNS rebinding Host header, got: {}",
        rebind_resp
    );

    let unauth_root = raw_http(
        port,
        &format!(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            port
        ),
    );
    assert!(
        unauth_root.starts_with("HTTP/1.1 403 Forbidden"),
        "expected 403 on unauthenticated GET / when GUI bootstrap is active, got: {}",
        unauth_root
    );
    assert!(!unauth_root.contains(&viewer_token));

    let boot_exchange = raw_http(
        port,
        &format!(
            "GET /?bootstrap={}&theme=onedark HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            bootstrap, port
        ),
    );
    assert!(
        boot_exchange.starts_with("HTTP/1.1 302 Found"),
        "expected 302 redirect on bootstrap exchange, got: {}",
        boot_exchange
    );
    assert!(boot_exchange.contains(&format!("ultradesk_token={}", viewer_token)));
    assert!(boot_exchange.contains("Location: /?theme=onedark\r\n"));

    let authed_root = raw_http(
        port,
        &format!(
            "GET /?theme=onedark HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: ultradesk_token={}\r\nConnection: close\r\n\r\n",
            port, viewer_token
        ),
    );
    assert!(authed_root.starts_with("HTTP/1.1 200 OK"));
    assert!(authed_root.contains("X-Frame-Options: DENY"));
    assert!(authed_root.contains("frame-ancestors 'none'"));

    let unauth_huge_post = raw_http(
        port,
        &format!(
            "POST /connect HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: 20000000\r\nConnection: close\r\n\r\n",
            port
        ),
    );
    assert!(
        unauth_huge_post.starts_with("HTTP/1.1 401 Unauthorized"),
        "expected 401 BEFORE reading 20MB body, got: {}",
        unauth_huge_post
    );

    let authed_oversize_json = raw_http(
        port,
        &format!(
            "POST /preferences HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Viewer-Token: {}\r\nContent-Type: application/json\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n",
            port, viewer_token
        ),
    );
    assert!(
        authed_oversize_json.starts_with("HTTP/1.1 413"),
        "expected 413 BEFORE allocating >64KiB non-upload body, got: {}",
        authed_oversize_json
    );

    let wrong_ctype = raw_http(
        port,
        &format!(
            "POST /preferences HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Viewer-Token: {}\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}",
            port, viewer_token
        ),
    );
    assert!(wrong_ctype.starts_with("HTTP/1.1 415"));

    let disabled_mcp = raw_http(
        port,
        &format!(
            "GET /mcp/tools HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Viewer-Token: {}\r\nConnection: close\r\n\r\n",
            port, viewer_token
        ),
    );
    assert!(
        disabled_mcp.starts_with("HTTP/1.1 404"),
        "expected /mcp/tools to be 404 when --mcp is disabled, got: {}",
        disabled_mcp
    );

    let crlf_send_body = json!({
        "file_name": "evil\"\r\nX-Injected-Header: pwned\r\n.txt",
        "data_base64": encode_base64(b"safe-bytes")
    })
    .to_string();
    let send_resp = raw_http(
        port,
        &format!(
            "POST /files/send HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Viewer-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            port,
            viewer_token,
            crlf_send_body.len(),
            crlf_send_body
        ),
    );
    assert!(send_resp.starts_with("HTTP/1.1 200 OK"));
    let send_body_part = send_resp.split("\r\n\r\n").nth(1).unwrap_or("{}");
    let send_json: Value = serde_json::from_str(send_body_part).unwrap();
    let sent_id = send_json["file_id"].as_str().unwrap();

    let dl_resp = raw_http(
        port,
        &format!(
            "GET /files/download?file_id={} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Viewer-Token: {}\r\nConnection: close\r\n\r\n",
            sent_id, port, viewer_token
        ),
    );
    assert!(dl_resp.starts_with("HTTP/1.1 200 OK"));
    let headers_part = dl_resp.split("\r\n\r\n").next().unwrap_or("");
    assert!(
        !headers_part.to_ascii_lowercase().contains("\nx-injected-header:"),
        "CRLF header injection in Content-Disposition must be sanitized: {}",
        headers_part
    );

    shutdown.store(true, Ordering::SeqCst);
    let _ = handle.join();
}

#[test]
fn test_mcp_mandatory_auth_network_isolation_and_secret_redaction() {
    let session = ControlSession::new();
    let _ = session.update_saved_credentials("987654321", "TopSecretPartnerPass!", true);

    assert!(McpServer::bind(
        "127.0.0.1",
        0,
        session.clone(),
        Some("   ".to_string())
    )
    .is_err());

    let (mcp_srv, listener) = McpServer::bind("127.0.0.1", 0, session.clone(), None).unwrap();
    let generated_token = mcp_srv.auth_token().to_string();
    assert!(generated_token.len() >= 32);
    let port = mcp_srv.port;
    let shutdown = mcp_srv.shutdown.clone();
    let handle = mcp_srv.spawn(listener);

    let unauth_mcp = raw_http(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: 44\r\nConnection: close\r\n\r\n{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}}",
            port
        ),
    );
    assert!(unauth_mcp.starts_with("HTTP/1.1 401 Unauthorized"));

    let cross_origin_mcp = raw_http(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: https://evil.example\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: 44\r\nConnection: close\r\n\r\n{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}}",
            port, generated_token
        ),
    );
    assert!(cross_origin_mcp.starts_with("HTTP/1.1 403 Forbidden"));

    let viewer_tok_rejected = raw_http(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Viewer-Token: {}\r\nContent-Type: application/json\r\nContent-Length: 44\r\nConnection: close\r\n\r\n{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}}",
            port, generated_token
        ),
    );
    assert!(viewer_tok_rejected.starts_with("HTTP/1.1 401 Unauthorized"));

    let net_host_shot = handle_mcp_raw_json(
        &session,
        br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"take_screenshot","arguments":{"source":"host"}}}"#,
        McpTransportOrigin::Network,
    )
    .unwrap();
    assert_eq!(net_host_shot["result"]["isError"], json!(true));

    let net_default_type = handle_mcp_raw_json(
        &session,
        br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"type_text","arguments":{"text":"secret"}}}"#,
        McpTransportOrigin::Network,
    )
    .unwrap();
    assert_eq!(net_default_type["result"]["isError"], json!(true));

    let net_manage_host = handle_mcp_raw_json(
        &session,
        br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"manage_host","arguments":{"action":"start"}}}"#,
        McpTransportOrigin::Network,
    )
    .unwrap();
    assert_eq!(net_manage_host["result"]["isError"], json!(true));

    let net_loopback_pivot = handle_mcp_raw_json(
        &session,
        br#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"connect_remote","arguments":{"partner_id":"lan://127.0.0.1:5938","password":"123"}}}"#,
        McpTransportOrigin::Network,
    )
    .unwrap();
    assert_eq!(net_loopback_pivot["result"]["isError"], json!(true));

    for origin in [McpTransportOrigin::Network, McpTransportOrigin::Stdio] {
        let status_resp = handle_mcp_raw_json(
            &session,
            br#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_session_status","arguments":{}}}"#,
            origin,
        )
        .unwrap();
        let inner_txt = status_resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(
            !inner_txt.contains("TopSecretPartnerPass!"),
            "MCP status leaked saved plaintext partner password on {:?}",
            origin
        );
    }

    shutdown.store(true, Ordering::SeqCst);
    let _ = handle.join();
}

#[test]
fn test_leaf_and_host_run2_security_invariants() {
    let mut audio = AudioBuffer::new(32);
    audio.set_enabled(false);
    let muted_pkt = encode_audio_packet(WaveSource::PcSound, &[0x33u8; 64], false).unwrap();
    assert!(audio.ingest_packet(&muted_pkt).unwrap().is_none());
    assert_eq!(audio.queued_bytes(), 0);

    audio.set_enabled(true);
    let silence_bomb =
        encode_audio_packet(WaveSource::PcSound, &(10_000_000i32).to_le_bytes(), true).unwrap();
    let clamped = audio.ingest_packet(&silence_bomb).unwrap().unwrap();
    assert_eq!(clamped.pcm_s16le.len(), 262_144);
    let bogus_silence =
        encode_audio_packet(WaveSource::PcSound, &(-5i32).to_le_bytes(), true).unwrap();
    assert!(audio.ingest_packet(&bogus_silence).is_err());

    let mut giant_png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    giant_png.extend_from_slice(&13u32.to_be_bytes());
    giant_png.extend_from_slice(b"IHDR");
    giant_png.extend_from_slice(&4096u32.to_be_bytes());
    giant_png.extend_from_slice(&4096u32.to_be_bytes());
    giant_png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    let mut giant_cur = Vec::with_capacity(22 + giant_png.len());
    giant_cur.extend_from_slice(&0u16.to_le_bytes());
    giant_cur.extend_from_slice(&2u16.to_le_bytes());
    giant_cur.extend_from_slice(&1u16.to_le_bytes());
    giant_cur.extend_from_slice(&[0, 0, 0, 0]);
    giant_cur.extend_from_slice(&0u16.to_le_bytes());
    giant_cur.extend_from_slice(&0u16.to_le_bytes());
    giant_cur.extend_from_slice(&(giant_png.len() as u32).to_le_bytes());
    giant_cur.extend_from_slice(&22u32.to_le_bytes());
    giant_cur.extend_from_slice(&giant_png);
    assert!(decode_cur_to_rgba(&giant_cur).is_err());

    let overflow_layout_fields = vec![
        vec![0u8],
        3840i32.to_le_bytes().to_vec(),
        1080i32.to_le_bytes().to_vec(),
        vec![],
        vec![],
        vec![0u8],
        b"6.6.133".to_vec(),
        vec![1u8],
        b"-1073741824:0:1073741824:1080|\0".to_vec(),
        0i32.to_le_bytes().to_vec(),
        0i32.to_le_bytes().to_vec(),
    ];
    let safe_fallback = DisplayLayout::parse_from_login(&overflow_layout_fields).unwrap();
    assert_eq!(safe_fallback.monitors.len(), 1);
    assert_eq!(
        safe_fallback.monitors[0],
        MonitorRect {
            left: 0,
            top: 0,
            right: 3840,
            bottom: 1080
        }
    );
    assert_eq!(safe_fallback.monitors[0].width(), 3840);

    let oversized_desktop_fields = vec![
        vec![0u8],
        20_000i32.to_le_bytes().to_vec(),
        20_000i32.to_le_bytes().to_vec(),
    ];
    assert!(DisplayLayout::parse_from_login(&oversized_desktop_fields).is_none());

    let decoder = ZstdDictionaryDecoder::new(Vec::new(), 64 * 1024 * 1024);
    let mut screen = RGB555Screen::new((2048, 2048), decoder, 1000, 32);
    let mut store_hdr = Vec::new();
    store_hdr.extend_from_slice(&0i16.to_le_bytes());
    store_hdr.extend_from_slice(&0i16.to_le_bytes());
    store_hdr.extend_from_slice(&512i16.to_le_bytes());
    store_hdr.extend_from_slice(&512i16.to_le_bytes());
    store_hdr.extend_from_slice(&77i32.to_le_bytes());
    let store_fields = vec![
        vec![1u8],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        store_hdr,
    ];
    assert!(screen.apply(&store_fields).is_ok());
    assert!(screen.cache_keys().contains(&77));

    let mut bogus_hdr = Vec::new();
    bogus_hdr.extend_from_slice(&0i16.to_le_bytes());
    bogus_hdr.extend_from_slice(&0i16.to_le_bytes());
    bogus_hdr.extend_from_slice(&1024i16.to_le_bytes());
    bogus_hdr.extend_from_slice(&1024i16.to_le_bytes());
    bogus_hdr.extend_from_slice(&88i32.to_le_bytes());
    let bogus_fields = vec![
        vec![1u8],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        bogus_hdr,
    ];
    let _ = screen.apply(&bogus_fields);
    assert!(!screen.cache_keys().contains(&88));

    let ft_dir = std::env::temp_dir().join(format!("uv_ft_sec_inv_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ft_dir);
    std::fs::create_dir_all(&ft_dir).unwrap();
    let mut ft = FileTransferManager::new(ft_dir.clone());
    ft.record_requested_remote_path("C:\\Users\\Alice\\secret.pdf");
    ft.reset_for_new_session();
    let stale_offer = encode_file_offer("f_stale", "secret.pdf", 12, false).unwrap();
    let reply = ft.handle_packet(&stale_offer).unwrap();
    assert!(
        matches!(
            reply,
            Some((FileTransferPacket::Offer { .. }, ref replies)) if replies.is_empty()
        ),
        "stale requested basename from previous session must NOT emit auto-accept reply"
    );

    let host_layout = DisplayLayout {
        width: 16,
        height: 16,
        virtual_left: 0,
        virtual_top: 0,
        monitors: vec![MonitorRect {
            left: 0,
            top: 0,
            right: 16,
            bottom: 16,
        }],
    };
    let master_proof = compute_master_password_proof("host_master_secret");
    let mut host_core = HostCore::new("12345678", Some(master_proof.clone()), host_layout);
    assert!(!host_core.files.auto_accept);
    assert!(!host_core.files.allow_remote_requests);
    let master_login = format!(
        "login:87654321:00000000000000000000000000000000:1920:1080:7:15:15:30:16:111:Viewer:1:{}:x:conn:2:45:0:0::6.6.133:0",
        master_proof
    );
    let actions = host_core.handle_packet(master_login.as_bytes(), Instant::now());
    assert_eq!(host_core.phase, HostPhase::Approval);
    assert!(matches!(actions.as_slice(), [HostAction::PromptApproval(_)]));

    let _ = std::fs::remove_dir_all(&ft_dir);
}

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::time::Instant;

use ultradesk_rust::clipboard::encode_clipboard_for_remote;
use ultradesk_rust::cursor::decode_cur_to_rgba;
use ultradesk_rust::e2e::{
    accept_viewer_offer_on_host, create_viewer_offer_for_password, derive_e2e_temp_secret,
    finalize_viewer_handshake, finalize_viewer_handshake_with_policy,
    offer_uses_derived_temp_secret,
};
use ultradesk_rust::file_transfer::{encode_file_chunk, encode_file_offer, FileTransferManager};
use ultradesk_rust::host::{
    encode_host_auth_failed, DesktopBackend, HostAction, HostCore, HostPhase, HostSession,
};
use ultradesk_rust::profile::{verify_master_password_proof, ProfileManager, MASTER_PASSWORD_FILE};
use ultradesk_rust::protocol::{
    atlas_layout, decode_byte_array_fields_checked, decode_target_route, encode_transport_frame,
    http_service_preface, login_password_fields, md5_hex_lower, FrameStream,
    MAX_FRAME_STREAM_BYTES,
};
use ultradesk_rust::screen::{DisplayLayout, MonitorRect};
use ultradesk_rust::server::PortalServer;
use ultradesk_rust::session::{ControlSession, PartnerProfile, RegistrationProfile};

#[test]
fn test_dib_cursor_short_header_no_panic() {
    for res_len in 1usize..64 {
        let mut cur = vec![
            0, 0, 2, 0, 1, 0, 16, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 22, 0, 0, 0,
        ];
        cur[14..18].copy_from_slice(&(res_len as u32).to_le_bytes());
        let mut res = vec![0u8; res_len];
        if res_len >= 4 {
            res[0..4].copy_from_slice(&40u32.to_le_bytes());
        }
        cur.extend_from_slice(&res);
        assert!(decode_cur_to_rgba(&cur).is_err());
    }
    let mut bad_header_cur = vec![
        0, 0, 2, 0, 1, 0, 16, 16, 0, 0, 0, 0, 0, 0, 48, 0, 0, 0, 22, 0, 0, 0,
    ];
    let mut bad_res = vec![0u8; 48];
    bad_res[0..4].copy_from_slice(&99u32.to_le_bytes());
    bad_header_cur.extend_from_slice(&bad_res);
    assert!(decode_cur_to_rgba(&bad_header_cur).is_err());
}

#[test]
fn test_atlas_layout_bounds_and_frame_stream_cap() {
    assert!(atlas_layout(&[(0, 0, 64, 64)], 0, 64).is_err());
    assert!(atlas_layout(&[(0, 0, 64, 64)], -16, 64).is_err());
    assert!(atlas_layout(&[(0, 0, 64, 64)], 512, 64).is_err());
    assert!(atlas_layout(&[(0, 0, 30_000, 30_000)], 1, 64).is_err());

    let mut http_stream = FrameStream::new(true);
    let oversized_header = vec![b'A'; 20_000];
    assert!(http_stream.feed(&oversized_header).is_empty());
    assert!(http_stream.buffer.is_empty());

    let mut bin_stream = FrameStream::new(false);
    let mut bad_prefix = Vec::new();
    bad_prefix.extend_from_slice(&((MAX_FRAME_STREAM_BYTES as u32) + 1024).to_le_bytes());
    bad_prefix.extend_from_slice(&[0u8; 64]);
    assert!(bin_stream.feed(&bad_prefix).is_empty());
    assert!(bin_stream.buffer.is_empty());

    let valid_frame = encode_transport_frame(b"\x01ok");
    let parsed = bin_stream.feed(&valid_frame);
    assert_eq!(parsed, vec![b"\x01ok".to_vec()]);
}

#[test]
fn test_http_preface_and_route_host_sanitization() {
    let preface = http_service_preface("relay.example.com\r\nX-Injected: pwn\r\n", true);
    let preface_str = String::from_utf8_lossy(&preface);
    assert!(!preface_str.contains("X-Injected"));
    assert!(preface_str.contains("Host: relay.example.com\r\n\r\n"));

    let injected_route = b"8:1:192.168.1.10:0:203.0.113.10:55000:55001:55002:hostB64:2112:0:198.51.100.22:0:1:evil.host\r\nInjected:0:0:0:0:0:token-xyz:";
    assert!(decode_target_route(injected_route).is_err());

    let bad_ip_route = b"8:1:192.168.1.10:0:203.0.113.10:55000:55001:55002:hostB64:2112:0:not_an_ip:0:1:functions3.ultraviewer.net:0:0:0:0:0:token-xyz:";
    assert!(decode_target_route(bad_ip_route).is_err());
}

#[test]
fn test_file_transfer_requires_explicit_accept_and_bounds_chunks() {
    let tmp_dir = std::env::temp_dir().join(format!("uv_sec_ft_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_dir);
    let mut mgr = FileTransferManager::new(tmp_dir.clone());
    assert!(!mgr.auto_accept);

    let offer = encode_file_offer("fid1", "secret.txt", 4, true).unwrap();
    let (_, replies) = mgr.handle_packet(&offer).unwrap().unwrap();
    assert!(replies.is_empty());

    let unaccepted_chunk = encode_file_chunk("fid1", 0, b"abcd", true).unwrap();
    assert!(mgr.handle_packet(&unaccepted_chunk).is_err());
    assert!(!tmp_dir.join("secret.txt").exists());

    let _accept_pkt = mgr.accept_incoming_offer("fid1").unwrap();
    let out_of_order_chunk = encode_file_chunk("fid1", 2, b"cd", true).unwrap();
    assert!(mgr.handle_packet(&out_of_order_chunk).is_err());

    let oversized_chunk = encode_file_chunk("fid1", 0, b"abcdef", true).unwrap();
    assert!(mgr.handle_packet(&oversized_chunk).is_err());

    let valid_chunk = encode_file_chunk("fid1", 0, b"abcd", true).unwrap();
    let (_, done_replies) = mgr.handle_packet(&valid_chunk).unwrap().unwrap();
    assert_eq!(done_replies.len(), 1);
    assert_eq!(fs::read(tmp_dir.join("secret.txt")).unwrap(), b"abcd");

    let _ = fs::remove_dir_all(&tmp_dir);
}

fn raw_http_request(port: u16, req: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    resp
}

#[test]
fn test_portal_server_auth_csrf_and_file_confinement() {
    let tmp_dir = std::env::temp_dir().join(format!("uv_sec_portal_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).unwrap();
    let secret_local_file = tmp_dir.join("local_secret.txt");
    fs::write(&secret_local_file, b"TOP_SECRET_LOCAL_FILE").unwrap();

    let session = ControlSession::new();
    let local_fid = session.offer_file_from_path(&secret_local_file).unwrap();
    assert!(session.downloaded_file_bytes(&local_fid).is_none());

    let (server, listener) = PortalServer::bind("127.0.0.1", 0, session, None).unwrap();
    let port = server.port;
    let token = server.viewer_token.clone();
    let shutdown = server.shutdown.clone();
    let handle = server.spawn(listener);

    let unauth_status = raw_http_request(
        port,
        "GET /status HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(unauth_status.starts_with("HTTP/1.1 401 Unauthorized"));

    let unauth_creds = raw_http_request(
        port,
        "GET /credentials HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(unauth_creds.starts_with("HTTP/1.1 401 Unauthorized"));

    let unauth_frame = raw_http_request(
        port,
        "GET /frame.jpg HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(unauth_frame.starts_with("HTTP/1.1 401 Unauthorized"));

    let csrf_post = raw_http_request(
        port,
        &format!(
            "POST /preferences HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://evil.example\r\nCookie: ultradesk_token={}\r\nContent-Type: application/json\r\nContent-Length: 19\r\nConnection: close\r\n\r\n{{\"theme\":\"onedark\"}}",
            token
        ),
    );
    assert!(csrf_post.starts_with("HTTP/1.1 403 Forbidden"));

    let cross_site_get = raw_http_request(
        port,
        &format!(
            "GET /status HTTP/1.1\r\nHost: 127.0.0.1\r\nSec-Fetch-Site: cross-site\r\nCookie: ultradesk_token={}\r\nConnection: close\r\n\r\n",
            token
        ),
    );
    assert!(cross_site_get.starts_with("HTTP/1.1 403 Forbidden"));

    let body_file_path = format!(r#"{{"file_path":"{}"}}"#, secret_local_file.display());
    let send_path_resp = raw_http_request(
        port,
        &format!(
            "POST /files/send HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Viewer-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            token,
            body_file_path.len(),
            body_file_path
        ),
    );
    assert!(send_path_resp.starts_with("HTTP/1.1 400 Bad Request"));

    let download_local_resp = raw_http_request(
        port,
        &format!(
            "GET /files/download?id={} HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Viewer-Token: {}\r\nConnection: close\r\n\r\n",
            local_fid, token
        ),
    );
    assert!(download_local_resp.starts_with("HTTP/1.1 404 Not Found"));

    let auth_status = raw_http_request(
        port,
        &format!(
            "GET /status HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Viewer-Token: {}\r\nConnection: close\r\n\r\n",
            token
        ),
    );
    assert!(auth_status.starts_with("HTTP/1.1 200 OK"));

    shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = handle.join();
    let _ = fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_host_core_e2e_preserved_on_bad_login_manual_approval_clipboard_and_nonce_replay() {
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
    let session = HostSession::with_profile(
        "FEDCBA9876543210FEDCBA9876543210",
        "123456789",
        "001122334455",
        "87654321",
        None,
        "Host",
        layout.clone(),
        DesktopBackend::headless(4, 4),
    );
    assert!(!session.status().auto_approve);

    let now = Instant::now();
    let temp_pw = "temp_pass_99";
    let nonce1 = "11112222";
    let auth1 = login_password_fields(temp_pw, 0, Some(nonce1)).unwrap();
    let (offer1, viewer_state1) =
        create_viewer_offer_for_password(nonce1, temp_pw, &auth1.secondary);
    assert!(offer_uses_derived_temp_secret(&offer1));

    let login1 = format!(
        "login:{}:{}:1920:1080:7:15:15:30:16:111222333:Viewer1:1:{}:x:conn:2:45:0:0:{}:6.6.133:0",
        nonce1, auth1.primary, auth1.secondary, offer1
    )
    .into_bytes();

    let mut core = HostCore::new(temp_pw, None, layout);
    let actions1 = core.handle_packet(&login1, now);
    assert_eq!(core.phase, HostPhase::Approval);
    assert!(matches!(
        actions1.as_slice(),
        [HostAction::PromptApproval(_)]
    ));

    let approve_actions = core.approve(false);
    assert_eq!(core.phase, HostPhase::Starting);
    assert!(!core.control_allowed);
    assert!(core.e2e.is_enabled());

    let init_pkt = match &approve_actions[0] {
        HostAction::SendPeer(b) => b.clone(),
        _ => panic!("expected screen init"),
    };
    let init_fields = decode_byte_array_fields_checked(&init_pkt, Some(11)).unwrap();
    let mut viewer_e2e =
        finalize_viewer_handshake_with_policy(&viewer_state1, &init_fields[3], true).unwrap();
    assert!(viewer_e2e.is_enabled());

    let ctrl_sealed = match &approve_actions[1] {
        HostAction::SendPeer(b) => b.clone(),
        _ => panic!("expected control permission"),
    };
    let _ = viewer_e2e.cipher_mut().unwrap().open(&ctrl_sealed).unwrap();

    let start_sealed = viewer_e2e.cipher_mut().unwrap().seal(b"17", false);
    assert_eq!(
        core.handle_packet(&start_sealed, now),
        vec![HostAction::RequestCapture]
    );
    assert_eq!(core.phase, HostPhase::Streaming);

    let bad_login = b"login:99998888:00000000000000000000000000000000:1920:1080:7:15:15:30:16:999:Attacker:1:00000000000000000000000000000000:x:conn:2:45:0:0::6.6.133:0";
    let bad_actions = core.handle_packet(bad_login, now);
    assert_eq!(
        bad_actions,
        vec![HostAction::SendPeer(encode_host_auth_failed())]
    );
    assert_eq!(core.phase, HostPhase::Streaming);
    assert!(core.e2e.is_enabled());

    let replay_actions = core.handle_packet(&login1, now);
    assert_eq!(
        replay_actions,
        vec![HostAction::SendPeer(encode_host_auth_failed())]
    );
    assert_eq!(core.phase, HostPhase::Streaming);
    assert!(core.e2e.is_enabled());

    let (clip_pkt, clip_unicode) = encode_clipboard_for_remote("injected clip").unwrap();
    let sealed_clip = viewer_e2e
        .cipher_mut()
        .unwrap()
        .seal(&clip_pkt, clip_unicode);
    assert!(core.handle_packet(&sealed_clip, now).is_empty());

    let nonce2 = "33334444";
    let auth2 = login_password_fields(temp_pw, 0, Some(nonce2)).unwrap();
    let (offer2, _) = create_viewer_offer_for_password(nonce2, temp_pw, &auth2.secondary);
    let login2 = format!(
        "login:{}:{}:1920:1080:7:15:15:30:16:111222333:Viewer1:1:{}:x:conn:2:45:0:0:{}:6.6.133:0",
        nonce2, auth2.primary, auth2.secondary, offer2
    )
    .into_bytes();
    let relogin_actions = core.handle_packet(&login2, now);
    assert_eq!(core.phase, HostPhase::Approval);
    assert!(relogin_actions
        .iter()
        .any(|a| matches!(a, HostAction::PromptApproval(_))));

    let bad_peer_login = b"login:99990000:00000000000000000000000000000000:1920:1080:7:15:15:30:16:999:Attacker:1:00000000000000000000000000000000:x:conn:2:45:0:0::6.6.133:0";
    for _ in 0..6 {
        let res = session
            .handle_peer_payload_from_peer("203.0.113.66", bad_peer_login)
            .unwrap();
        assert_eq!(res, vec![HostAction::SendPeer(encode_host_auth_failed())]);
    }
    assert!(!session.status().is_locked);
    let good_nonce = "77778888";
    let good_auth = login_password_fields("87654321", 0, Some(good_nonce)).unwrap();
    let good_login = format!(
        "login:{}:{}:1920:1080:7:15:15:30:16:111222333:LegitPeer:1:{}:x:conn:2:45:0:0::6.6.133:0",
        good_nonce, good_auth.primary, good_auth.secondary
    )
    .into_bytes();
    let locked_peer_res = session
        .handle_peer_payload_from_peer("203.0.113.66", &good_login)
        .unwrap();
    assert_eq!(
        locked_peer_res,
        vec![HostAction::SendPeer(encode_host_auth_failed())]
    );
    let legit_peer_res = session
        .handle_peer_payload_from_peer("192.168.1.50", &good_login)
        .unwrap();
    assert!(legit_peer_res
        .iter()
        .any(|a| matches!(a, HostAction::PromptApproval(_))));
}

#[test]
fn test_e2e_derived_temp_secret_and_downgrade_policy() {
    let nonce = "87654321";
    let password = "secret_password";
    let wire_md5_proof = md5_hex_lower(format!("{}{}", nonce, password).as_bytes());
    let (offer_wire, viewer_state) = create_viewer_offer_for_password(nonce, password, "");

    let (mitm_answer, _) =
        accept_viewer_offer_on_host(&offer_wire, nonce, &wire_md5_proof).unwrap();
    assert!(finalize_viewer_handshake(&viewer_state, &mitm_answer).is_err());

    assert!(finalize_viewer_handshake_with_policy(&viewer_state, &[], true).is_err());

    let derived = derive_e2e_temp_secret(nonce, password);
    let (valid_answer, host_cipher) =
        accept_viewer_offer_on_host(&offer_wire, nonce, &derived).unwrap();
    let established =
        finalize_viewer_handshake_with_policy(&viewer_state, &valid_answer, true).unwrap();
    assert_eq!(established.fingerprint(), host_cipher.fingerprint);
}

#[test]
fn test_profile_and_credentials_permissions_and_locked_master_password() {
    let reg_default = RegistrationProfile::default();
    let partner_default = PartnerProfile::default();
    assert_ne!(
        partner_default.request_terms.get(1).map(String::as_str),
        Some("90135737")
    );
    assert_ne!(partner_default.password, "27296");
    assert_eq!(
        reg_default.terms.get(1).map(String::as_str),
        Some("00000000000000000000000000000000")
    );

    assert!(!verify_master_password_proof("any", ""));
    assert!(!verify_master_password_proof("any", "zzzz"));

    let tmp_dir = std::env::temp_dir().join(format!("uv_sec_prof_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_dir);
    let mgr = ProfileManager::from_custom_dir(tmp_dir.clone());
    let client = mgr.load_or_create_client().unwrap();
    let host = mgr.load_or_create_host().unwrap();

    let dir_mode = fs::metadata(&tmp_dir).unwrap().permissions().mode() & 0o777;
    let client_mode = fs::metadata(&client.path).unwrap().permissions().mode() & 0o777;
    let host_mode = fs::metadata(&host.path).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700);
    assert_eq!(client_mode, 0o600);
    assert_eq!(host_mode, 0o600);

    fs::write(tmp_dir.join(MASTER_PASSWORD_FILE), b"corrupted-non-hex\n").unwrap();
    assert!(mgr.load_or_create_host().is_err());

    let _ = fs::remove_dir_all(&tmp_dir);
}

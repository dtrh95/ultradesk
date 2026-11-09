use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use ultradesk_rust::gui::{launch_native_gui, GuiConfig, GuiLaunchOutcome};
use ultradesk_rust::mcp::{run_stdio_mcp_loop, McpServer, DEFAULT_MCP_PORT};
use ultradesk_rust::profile::ProfileManager;
use ultradesk_rust::server::{PortalServer, DEFAULT_APP_ICON_PNG};
use ultradesk_rust::session::{
    ControlSession, PartnerProfile, RegistrationProfile, DEFAULT_ZSTD_DICT,
};

#[derive(Debug)]
struct CliArgs {
    subcommand: Option<String>,
    host: String,
    port: u16,
    mcp_enabled: bool,
    mcp_port: Option<u16>,
    mcp_token: Option<String>,
    registration_path: Option<PathBuf>,
    partner_path: Option<PathBuf>,
    dictionary_path: Option<PathBuf>,
    partner_id: String,
    password: String,
    master_password_arg: Option<String>,
    clear_master_password: bool,
    auto_approve: bool,
    no_gui: bool,
    open_browser: bool,
    update_apply: bool,
    update_check: bool,
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            subcommand: None,
            host: "127.0.0.1".to_string(),
            port: 0,
            mcp_enabled: false,
            mcp_port: None,
            mcp_token: None,
            registration_path: None,
            partner_path: None,
            dictionary_path: None,
            partner_id: String::new(),
            password: String::new(),
            master_password_arg: None,
            clear_master_password: false,
            auto_approve: false,
            no_gui: false,
            open_browser: false,
            update_apply: false,
            update_check: false,
        }
    }
}

fn parse_args() -> CliArgs {
    let mut cfg = CliArgs::default();
    let args: Vec<String> = env::args().skip(1).collect();
    let mut i = 0usize;
    if let Some(first) = args.first() {
        if matches!(
            first.as_str(),
            "host" | "id" | "master-password" | "mcp" | "update" | "help"
        ) {
            cfg.subcommand = Some(first.clone());
            i = 1;
        }
    }
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                cfg.subcommand = Some("help".to_string());
                i += 1;
            }
            "--host" if i + 1 < args.len() => {
                cfg.host = args[i + 1].clone();
                i += 2;
            }
            "--port" if i + 1 < args.len() => {
                cfg.port = args[i + 1].parse().unwrap_or(0);
                i += 2;
            }
            "--mcp" | "--ws" => {
                cfg.mcp_enabled = true;
                i += 1;
            }
            "--mcp-port" if i + 1 < args.len() => {
                cfg.mcp_enabled = true;
                cfg.mcp_port = args[i + 1].parse().ok();
                i += 2;
            }
            "--mcp-token" if i + 1 < args.len() => {
                cfg.mcp_token = Some(args[i + 1].clone());
                i += 2;
            }
            "--registration" if i + 1 < args.len() => {
                cfg.registration_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--partner" if i + 1 < args.len() => {
                let val = &args[i + 1];
                if val.ends_with(".json") || PathBuf::from(val).exists() {
                    cfg.partner_path = Some(PathBuf::from(val));
                } else {
                    cfg.partner_id = val.clone();
                }
                i += 2;
            }
            "--dictionary" if i + 1 < args.len() => {
                cfg.dictionary_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--partner-id" if i + 1 < args.len() => {
                cfg.partner_id = args[i + 1].clone();
                i += 2;
            }
            "--password" if i + 1 < args.len() => {
                cfg.password = args[i + 1].clone();
                i += 2;
            }
            "--clear" => {
                cfg.clear_master_password = true;
                i += 1;
            }
            "--auto-approve" => {
                cfg.auto_approve = true;
                i += 1;
            }
            "--no-gui" => {
                cfg.no_gui = true;
                i += 1;
            }
            "--open-browser" => {
                cfg.open_browser = true;
                i += 1;
            }
            "--check" => {
                cfg.update_check = true;
                i += 1;
            }
            "--apply" => {
                cfg.update_apply = true;
                i += 1;
            }
            other => {
                if cfg.subcommand.as_deref() == Some("master-password")
                    && !other.starts_with('-')
                    && cfg.master_password_arg.is_none()
                {
                    cfg.master_password_arg = Some(other.to_string());
                }
                i += 1;
            }
        }
    }
    cfg
}

fn find_asset_file(file_name: &str) -> Option<PathBuf> {
    if file_name.contains('/') || file_name.contains('\\') || file_name.contains("..") {
        return None;
    }
    let user_config_asset = env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .map(|base| base.join("ultradesk-rust").join(file_name))
        .unwrap_or_default();
    let exe_dir_asset = env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .and_then(|p| {
            let parent = p.parent()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let meta = fs::metadata(parent).ok()?;
                let euid = unsafe { libc::geteuid() };
                if (meta.mode() & 0o002) != 0 || (meta.uid() != 0 && meta.uid() != euid) {
                    return None;
                }
            }
            Some(parent.join(file_name))
        })
        .unwrap_or_default();
    let exe_share_asset = env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .and_then(|p| {
            let prefix = p.parent()?.parent()?;
            let share_dir = prefix.join("share").join("ultradesk-rust");
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let meta = fs::metadata(&share_dir).ok()?;
                let euid = unsafe { libc::geteuid() };
                if (meta.mode() & 0o002) != 0 || (meta.uid() != 0 && meta.uid() != euid) {
                    return None;
                }
            }
            Some(share_dir.join(file_name))
        })
        .unwrap_or_default();
    let exe_bundle_asset = env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .and_then(|p| {
            let prefix = p.parent()?.parent()?;
            let res_dir = prefix.join("Resources");
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let meta = fs::metadata(&res_dir).ok()?;
                let euid = unsafe { libc::geteuid() };
                if (meta.mode() & 0o002) != 0 || (meta.uid() != 0 && meta.uid() != euid) {
                    return None;
                }
            }
            Some(res_dir.join(file_name))
        })
        .unwrap_or_default();
    let candidates = [
        PathBuf::from("/usr/share/ultradesk-rust").join(file_name),
        exe_share_asset,
        exe_bundle_asset,
        exe_dir_asset,
        user_config_asset,
    ];
    candidates
        .into_iter()
        .find(|p| !p.as_os_str().is_empty() && p.is_file())
}

fn open_url_in_browser(url: &str) -> bool {
    let launchers: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[
            ("open", &[url]),
            ("xdg-open", &[url]),
            ("gio", &["open", url]),
            ("sensible-browser", &[url]),
            ("x-www-browser", &[url]),
        ]
    } else {
        &[
            ("xdg-open", &[url]),
            ("gio", &["open", url]),
            ("sensible-browser", &[url]),
            ("x-www-browser", &[url]),
        ]
    };
    for (bin, args) in launchers {
        if let Ok(child) = Command::new(bin)
            .args(*args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            drop(child);
            return true;
        }
    }
    false
}

fn main() {
    let args = parse_args();

    if let Some(ref sub) = args.subcommand {
        if sub == "help" {
            println!("Usage:");
            println!("  ultradesk-rust [OPTIONS]                      Launch Dual-Pane Portal (Client + Host)");
            println!("  ultradesk-rust host [--auto-approve] [OPTS]   Start directly in Host screen-sharing mode");
            println!("  ultradesk-rust mcp [--port <PORT>] [OPTS]     Run Model Context Protocol (MCP) server (stdio or WebSocket/HTTP)");
            println!("  ultradesk-rust id                             Print Host ID, Temp Password, and XDG profile info");
            println!("  ultradesk-rust master-password <PW>|--clear   Set or clear persistent Host master password");
            println!("  ultradesk-rust update [--check | --apply]     Check for updates or apply staged release");
            println!("\nOptions:");
            println!(
                "  --host <ADDR>          Bind address for Portal/MCP server (default: 127.0.0.1)"
            );
            println!("  --port <PORT>          Bind port for Portal/MCP server (default: auto)");
            println!(
                "  --mcp                  Enable local MCP server on ws://127.0.0.1:{}",
                DEFAULT_MCP_PORT
            );
            println!("  --mcp-port <PORT>      Enable local MCP server on a custom port");
            println!(
                "  --mcp-token <TOKEN>    Require bearer/header token on network MCP requests"
            );
            println!("  --partner-id <ID>      Auto-connect to partner ID on startup");
            println!("  --password <PW>        Partner password for auto-connect");
            println!("  --auto-approve         Auto-approve incoming Host connections");
            println!(
                "  --no-gui               Run headless without opening WebKit2/browser window"
            );
            println!("  --open-browser         Open Portal URL in default web browser");
            return;
        }
        let pm = ProfileManager::from_xdg().unwrap_or_else(|_| {
            ProfileManager::from_custom_dir(ultradesk_rust::profile::fallback_private_config_dir())
        });
        if sub == "id" {
            let host_prof = pm
                .load_or_create_host()
                .expect("Failed to load host profile");
            let client_prof = pm
                .load_or_create_client()
                .expect("Failed to load client profile");
            let live_id = ultradesk_rust::host::query_live_host_id(
                &host_prof.device_id,
                &host_prof.hardware_mac,
                &host_prof.temp_password,
                host_prof.master_password_proof.as_deref(),
                &host_prof.display_name,
            )
            .unwrap_or_else(|_| "offline".to_string());
            println!("Host UltraViewer ID: {}", live_id);
            println!("Temp Password:       {}", host_prof.temp_password);
            println!("Host Profile Path:   {}", host_prof.path.display());
            println!("Host Device ID:      {}", host_prof.device_id);
            println!("Hardware MAC:        {}", host_prof.hardware_mac);
            println!("Client Synth MAC:    {}", client_prof.synthetic_mac);
            return;
        }
        if sub == "master-password" {
            let target_pw = if args.clear_master_password {
                None
            } else {
                args.master_password_arg.as_deref()
            };
            match pm.set_master_password(target_pw) {
                Ok(()) => {
                    if target_pw.is_some() {
                        println!("Master password updated.");
                    } else {
                        println!("Master password cleared.");
                    }
                    return;
                }
                Err(e) => {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            }
        }
        if sub == "update" {
            let session = ControlSession::new();
            match session.check_for_updates() {
                Ok(snap) => {
                    println!("UltraDesk Update Status:");
                    println!("  Current version: {}", snap.current_version);
                    println!(
                        "  Latest version:  {}",
                        snap.latest_version.as_deref().unwrap_or("---")
                    );
                    println!(
                        "  Install target:  {} ({})",
                        snap.install_target, snap.install_target_kind
                    );
                    if let Some(ref asset) = snap.asset_name {
                        println!("  Matched asset:   {}", asset);
                    }
                    if let Some(ref notes) = snap.release_notes {
                        println!("  Release notes:   {}", notes.lines().next().unwrap_or(""));
                    }
                    if snap.state == ultradesk_rust::updater::UpdateState::UpToDate {
                        println!("Status: UltraDesk is up to date.");
                        return;
                    }
                    if snap.state == ultradesk_rust::updater::UpdateState::Available {
                        println!("Status: New update is available!");
                        if args.update_apply {
                            println!("Applying update...");
                            match session.confirm_update() {
                                Ok(msg) => println!("Success: {}", msg),
                                Err(e) => {
                                    eprintln!("Error applying update: {}", e);
                                    std::process::exit(1);
                                }
                            }
                        } else {
                            println!("Run 'ultradesk-rust update --apply' to download and install this update.");
                        }
                        return;
                    }
                }
                Err(e) => {
                    eprintln!("Update check failed: {}", e);
                    std::process::exit(1);
                }
            }
            return;
        }
    }

    let reg_profile = args
        .registration_path
        .or_else(|| find_asset_file("linux-controller-private.json"))
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<RegistrationProfile>(&s).ok())
        .unwrap_or_default();

    let partner_profile = args
        .partner_path
        .or_else(|| find_asset_file("default-partner.json"))
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<PartnerProfile>(&s).ok())
        .unwrap_or_default();

    let dict_bytes = args
        .dictionary_path
        .or_else(|| find_asset_file("8bit.dat"))
        .and_then(|p| fs::read(p).ok())
        .unwrap_or_else(|| DEFAULT_ZSTD_DICT.to_vec());

    let session = ControlSession::with_profiles(reg_profile, partner_profile, dict_bytes);
    session.spawn_background_update_check();

    if args.auto_approve {
        if let Ok(guard) = session.inner.lock() {
            guard.host_session.set_auto_approve(true);
        }
    }

    if args.subcommand.as_deref() == Some("mcp") {
        let _ = session.host_start();
        if !args.partner_id.trim().is_empty() {
            let _ = session.start_connect(&args.partner_id, &args.password);
        }
        let network_port = args.mcp_port.or({
            if args.port > 0 {
                Some(args.port)
            } else if args.mcp_enabled {
                Some(DEFAULT_MCP_PORT)
            } else {
                None
            }
        });
        if let Some(p) = network_port {
            let (mcp_srv, mcp_listener) =
                match McpServer::bind(&args.host, p, session.clone(), args.mcp_token.clone()) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("{}", e);
                        std::process::exit(1);
                    }
                };
            eprintln!(
                "UltraDesk MCP server listening on ws://{} and http://{}/mcp",
                mcp_srv.bind_addr, mcp_srv.bind_addr
            );
            let shutdown = mcp_srv.shutdown.clone();
            let handle = mcp_srv.spawn(mcp_listener);
            while !shutdown.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(250));
            }
            session.disconnect();
            let _ = handle.join();
        } else {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            let _ = run_stdio_mcp_loop(&session, stdin.lock(), stdout.lock());
            session.disconnect();
        }
        return;
    }

    let hs = session.host_start();
    if args.subcommand.as_deref() == Some("host") || !hs.host_id.is_empty() {
        println!(
            "UltraDesk Host Mode active (ID: {}, Temp Password: {}, Backend: {})",
            hs.host_id, hs.temp_password, hs.backend_name
        );
    }

    if !args.partner_id.trim().is_empty() {
        let _ = session.start_connect(&args.partner_id, &args.password);
    }

    let mut mcp_bg: Option<(
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread::JoinHandle<()>,
    )> = None;
    if args.mcp_enabled || args.mcp_port.is_some() {
        let port = args.mcp_port.unwrap_or(DEFAULT_MCP_PORT);
        match McpServer::bind(&args.host, port, session.clone(), args.mcp_token.clone()) {
            Ok((mcp_srv, mcp_listener)) => {
                println!(
                    "UltraDesk MCP server listening on ws://{} and http://{}/mcp",
                    mcp_srv.bind_addr, mcp_srv.bind_addr
                );
                let mcp_shutdown = mcp_srv.shutdown.clone();
                let handle = mcp_srv.spawn(mcp_listener);
                mcp_bg = Some((mcp_shutdown, handle));
            }
            Err(e) => {
                eprintln!("Warning: {}", e);
            }
        }
    }

    let viewer_path = find_asset_file("viewer.html");
    let (portal, listener) =
        match PortalServer::bind(&args.host, args.port, session.clone(), viewer_path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{}", e);
                std::process::exit(1);
            }
        };

    let bound_port = portal.port;
    let snap = session.snapshot();
    let pill = if snap.saved_toolbar_collapsed {
        "collapsed"
    } else {
        "expanded"
    };
    let url = format!(
        "http://{}/?theme={}&lang={}&pill={}",
        portal.bind_addr, snap.saved_theme, snap.saved_language, pill
    );
    println!(
        "UltraDesk Controller core listening on http://127.0.0.1:{}",
        bound_port
    );

    let shutdown = portal.shutdown.clone();
    let server_handle = portal.spawn(listener);

    let has_display = cfg!(target_os = "macos")
        || env::var("DISPLAY").is_ok()
        || env::var("WAYLAND_DISPLAY").is_ok();
    let mut browser_fallback_needed = args.open_browser;
    if !args.no_gui && has_display {
        let viewer_path = find_asset_file("viewer.html");
        let gui_config = GuiConfig {
            url: url.clone(),
            title: "UltraDesk (Rust)".to_string(),
            default_width: 1200.0,
            default_height: 820.0,
            icon_bytes: Some(DEFAULT_APP_ICON_PNG),
            viewer_path,
            disable_dmabuf: true,
            disable_compositing: true,
            enable_native_menubar: false,
        };

        match launch_native_gui(gui_config, session.clone(), shutdown.clone()) {
            GuiLaunchOutcome::ClosedCleanly => {
                shutdown.store(true, Ordering::SeqCst);
                if let Some((ref mcp_stop, _)) = mcp_bg {
                    mcp_stop.store(true, Ordering::SeqCst);
                }
                session.disconnect();
                let _ = server_handle.join();
                if let Some((_, h)) = mcp_bg {
                    let _ = h.join();
                }
                return;
            }
            GuiLaunchOutcome::FallbackToBrowser(reason) => {
                eprintln!(
                    "Native GUI unavailable ({}), falling back to default browser at {}",
                    reason, url
                );
                browser_fallback_needed = true;
            }
        }
    }

    if browser_fallback_needed && has_display {
        let _ = open_url_in_browser(&url);
    }

    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }

    if let Some((ref mcp_stop, _)) = mcp_bg {
        mcp_stop.store(true, Ordering::SeqCst);
    }
    session.disconnect();
    let _ = server_handle.join();
    if let Some((_, h)) = mcp_bg {
        let _ = h.join();
    }
}

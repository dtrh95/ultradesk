use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};
use ultradesk_rust::profile::ProfileManager;
use ultradesk_rust::server::{
    materialize_embedded_gui_assets, GuiLaunchOutcome, PortalServer,
};
use ultradesk_rust::session::{
    ControlSession, PartnerProfile, RegistrationProfile, DEFAULT_ZSTD_DICT,
};

#[derive(Debug)]
struct CliArgs {
    subcommand: Option<String>,
    host: String,
    port: u16,
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
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            subcommand: None,
            host: "127.0.0.1".to_string(),
            port: 0,
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
        }
    }
}

fn parse_args() -> CliArgs {
    let mut cfg = CliArgs::default();
    let args: Vec<String> = env::args().skip(1).collect();
    let mut i = 0usize;
    if let Some(first) = args.first() {
        if matches!(first.as_str(), "host" | "id" | "master-password" | "help") {
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
    let user_config_asset = env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))
        .map(|base| base.join("ultradesk-rust").join(file_name))
        .unwrap_or_default();
    let candidates = [
        PathBuf::from(file_name),
        env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join(file_name)))
            .unwrap_or_default(),
        env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("../..").join(file_name)))
            .unwrap_or_default(),
        PathBuf::from("/usr/share/ultradesk-rust").join(file_name),
        user_config_asset,
    ];
    candidates
        .into_iter()
        .find(|p| !p.as_os_str().is_empty() && p.exists())
}

fn resolve_gui_script() -> Option<PathBuf> {
    if let Some(path) = find_asset_file("app_window.py") {
        return Some(path);
    }
    let runtime_base = env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::temp_dir());
    let target_dir = runtime_base.join("ultradesk-rust");
    materialize_embedded_gui_assets(&target_dir).ok()
}

fn open_url_in_browser(url: &str) -> bool {
    let launchers: &[(&str, &[&str])] = &[
        ("xdg-open", &[url]),
        ("gio", &["open", url]),
        ("sensible-browser", &[url]),
        ("x-www-browser", &[url]),
    ];
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
            println!("  ultradesk-rust id                             Print Host ID, Temp Password, and XDG profile info");
            println!("  ultradesk-rust master-password <PW>|--clear   Set or clear persistent Host master password");
            println!("\nOptions:");
            println!("  --host <ADDR>          Bind address for Portal HTTP server (default: 127.0.0.1)");
            println!("  --port <PORT>          Bind port for Portal HTTP server (default: auto)");
            println!("  --partner-id <ID>      Auto-connect to partner ID on startup");
            println!("  --password <PW>        Partner password for auto-connect");
            println!("  --auto-approve         Auto-approve incoming Host connections");
            println!("  --no-gui               Run headless without opening WebKit2/browser window");
            println!("  --open-browser         Open Portal URL in default web browser");
            return;
        }
        let pm = ProfileManager::from_xdg().unwrap_or_else(|_| {
            ProfileManager::from_custom_dir(env::temp_dir().join("ultradesk-rust"))
        });
        if sub == "id" {
            let host_prof = pm.load_or_create_host().expect("Failed to load host profile");
            let client_prof = pm.load_or_create_client().expect("Failed to load client profile");
            println!("Host Profile Path: {}", host_prof.path.display());
            println!("Host Device ID:    {}", host_prof.device_id);
            println!("Temp Password:     {}", host_prof.temp_password);
            println!("Hardware MAC:      {}", host_prof.hardware_mac);
            println!("Client Synth MAC:  {}", client_prof.synthetic_mac);
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

    if args.subcommand.as_deref() == Some("host") {
        if args.auto_approve {
            if let Ok(guard) = session.inner.lock() {
                guard.host_session.set_auto_approve(true);
            }
        }
        let hs = session.host_start();
        println!(
            "UltraDesk Host Mode active (ID: {}, Temp Password: {}, Backend: {})",
            hs.host_id, hs.temp_password, hs.backend_name
        );
    }

    if !args.partner_id.trim().is_empty() {
        let _ = session.start_connect(&args.partner_id, &args.password);
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
    let url = format!("http://{}/", portal.bind_addr);
    println!("UltraDesk Controller core listening on http://127.0.0.1:{}", bound_port);

    let shutdown = portal.shutdown.clone();
    let server_handle = portal.spawn(listener);

    let has_display = env::var("DISPLAY").is_ok() || env::var("WAYLAND_DISPLAY").is_ok();
    let mut browser_fallback_needed = args.open_browser;
    if !args.no_gui && has_display {
        match resolve_gui_script() {
            Some(window_script) => {
                let started = Instant::now();
                match Command::new("python3")
                    .arg(&window_script)
                    .arg(bound_port.to_string())
                    .spawn()
                {
                    Ok(mut child) => {
                        let code = child.wait().ok().and_then(|s| s.code());
                        match GuiLaunchOutcome::from_exit(code, started.elapsed()) {
                            GuiLaunchOutcome::ClosedCleanly => {
                                shutdown.store(true, Ordering::SeqCst);
                                session.disconnect();
                                let _ = server_handle.join();
                                return;
                            }
                            GuiLaunchOutcome::FallbackToBrowser(reason) => {
                                eprintln!(
                                    "Native WebKit2 GUI unavailable ({}), falling back to default browser at {}",
                                    reason, url
                                );
                                browser_fallback_needed = true;
                            }
                        }
                    }
                    Err(err) => {
                        eprintln!(
                            "Failed to launch python3 for native GUI ({}), falling back to default browser at {}",
                            err, url
                        );
                        browser_fallback_needed = true;
                    }
                }
            }
            None => {
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

    session.disconnect();
    let _ = server_handle.join();
}

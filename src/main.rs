use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use ultraviewer_linux::server::PortalServer;
use ultraviewer_linux::session::{
    ControlSession, PartnerProfile, RegistrationProfile, DEFAULT_ZSTD_DICT,
};

#[derive(Debug)]
struct CliArgs {
    host: String,
    port: u16,
    registration_path: Option<PathBuf>,
    partner_path: Option<PathBuf>,
    dictionary_path: Option<PathBuf>,
    partner_id: String,
    password: String,
    no_gui: bool,
    open_browser: bool,
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 0,
            registration_path: None,
            partner_path: None,
            dictionary_path: None,
            partner_id: String::new(),
            password: String::new(),
            no_gui: false,
            open_browser: false,
        }
    }
}

fn parse_args() -> CliArgs {
    let mut cfg = CliArgs::default();
    let args: Vec<String> = env::args().skip(1).collect();
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
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
            "--no-gui" => {
                cfg.no_gui = true;
                i += 1;
            }
            "--open-browser" => {
                cfg.open_browser = true;
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    cfg
}

fn find_asset_file(file_name: &str) -> Option<PathBuf> {
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
    ];
    candidates.into_iter().find(|p| !p.as_os_str().is_empty() && p.exists())
}

fn main() {
    let args = parse_args();

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
    println!("UltraViewer Controller core listening on http://127.0.0.1:{}", bound_port);

    let shutdown = portal.shutdown.clone();
    let server_handle = portal.spawn(listener);

    let has_display = env::var("DISPLAY").is_ok() || env::var("WAYLAND_DISPLAY").is_ok();
    if !args.no_gui && has_display {
        if let Some(window_script) = find_asset_file("app_window.py") {
            if let Ok(mut child) = Command::new("python3")
                .arg(window_script)
                .arg(bound_port.to_string())
                .spawn()
            {
                let _ = child.wait();
                shutdown.store(true, Ordering::SeqCst);
                session.disconnect();
                let _ = server_handle.join();
                return;
            }
        }
    }

    if args.open_browser {
        let _ = Command::new("xdg-open").arg(&url).spawn();
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

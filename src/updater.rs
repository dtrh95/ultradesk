use std::env;
use std::fs::{self};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use native_tls::{TlsConnector, TlsStream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const DEFAULT_GITHUB_API_URL: &str =
    "https://api.github.com/repos/dtrh95/ultradesk/releases/latest";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    Idle,
    Checking,
    Available,
    UpToDate,
    Downloading,
    Ready,
    Applying,
    Applied,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallTarget {
    DebianDeb { codename: String, arch: String },
    UbuntuDeb { codename: String, arch: String },
    FedoraRpm { version: String, arch: String },
    OpenSuseRpm { distro: String, arch: String },
    ArchPkg { arch: String },
    NixOs,
    MacOsApp { arch: String },
    MacOsHomebrew,
    PortableBinary { arch: String },
}

impl InstallTarget {
    pub fn detect_current() -> Self {
        if let Ok(exe) = env::current_exe() {
            let exe_str = exe.to_string_lossy();
            if exe_str.contains("/nix/store/") {
                return Self::NixOs;
            }
            if exe_str.contains("/opt/homebrew/") || exe_str.contains("/usr/local/Cellar/") {
                return Self::MacOsHomebrew;
            }
            if exe_str.contains(".app/Contents/MacOS/") {
                let arch = if cfg!(target_arch = "aarch64") {
                    "arm64"
                } else {
                    "x86_64"
                };
                return Self::MacOsApp {
                    arch: arch.to_string(),
                };
            }
        }

        #[cfg(target_os = "macos")]
        {
            let arch = if cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                "x86_64"
            };
            return Self::MacOsApp {
                arch: arch.to_string(),
            };
        }

        #[cfg(target_os = "linux")]
        {
            let arch = if cfg!(target_arch = "x86_64") {
                "x86_64"
            } else {
                "aarch64"
            };
            if let Ok(os_release) = fs::read_to_string("/etc/os-release") {
                let mut id = String::new();
                let mut id_like = String::new();
                let mut version_id = String::new();
                let mut codename = String::new();
                for line in os_release.lines() {
                    let trimmed = line.trim();
                    if let Some(val) = trimmed.strip_prefix("ID=") {
                        id = val.trim_matches('"').to_lowercase();
                    } else if let Some(val) = trimmed.strip_prefix("ID_LIKE=") {
                        id_like = val.trim_matches('"').to_lowercase();
                    } else if let Some(val) = trimmed.strip_prefix("VERSION_ID=") {
                        version_id = val.trim_matches('"').to_lowercase();
                    } else if let Some(val) = trimmed.strip_prefix("VERSION_CODENAME=") {
                        codename = val.trim_matches('"').to_lowercase();
                    }
                }

                let is_ubuntu =
                    id == "ubuntu" || id == "pop" || id == "mint" || id_like.contains("ubuntu");
                let is_debian = id == "debian" || id_like.contains("debian");
                let is_fedora = id == "fedora" || id_like.contains("fedora");
                let is_suse = id.contains("suse") || id_like.contains("suse");
                let is_arch = id == "arch"
                    || id == "manjaro"
                    || id == "endeavouros"
                    || id_like.contains("arch");

                if is_ubuntu {
                    let deb_arch = if arch == "x86_64" { "amd64" } else { "arm64" };
                    return Self::UbuntuDeb {
                        codename: if codename.is_empty() {
                            "noble".into()
                        } else {
                            codename
                        },
                        arch: deb_arch.into(),
                    };
                }
                if is_debian {
                    let deb_arch = if arch == "x86_64" { "amd64" } else { "arm64" };
                    return Self::DebianDeb {
                        codename: if codename.is_empty() {
                            "bookworm".into()
                        } else {
                            codename
                        },
                        arch: deb_arch.into(),
                    };
                }
                if is_fedora {
                    return Self::FedoraRpm {
                        version: if version_id.is_empty() {
                            "43".into()
                        } else {
                            version_id
                        },
                        arch: arch.into(),
                    };
                }
                if is_suse {
                    return Self::OpenSuseRpm {
                        distro: "tumbleweed".into(),
                        arch: arch.into(),
                    };
                }
                if is_arch {
                    return Self::ArchPkg { arch: arch.into() };
                }
            }

            if let Ok(exe) = env::current_exe() {
                if let Ok(meta) = fs::metadata(&exe) {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        let euid = unsafe { libc::geteuid() };
                        if meta.uid() == euid && !exe.starts_with("/usr/") {
                            return Self::PortableBinary { arch: arch.into() };
                        }
                    }
                }
            }

            Self::PortableBinary { arch: arch.into() }
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Self::PortableBinary {
                arch: "x86_64".into(),
            }
        }
    }

    pub fn match_asset_name(&self, version: &str) -> String {
        let v = version.trim_start_matches('v');
        match self {
            Self::DebianDeb { codename, arch } => {
                format!("ultradesk-rust_{v}-1_{arch}_{codename}.deb")
            }
            Self::UbuntuDeb { codename, arch } => {
                format!("ultradesk-rust_{v}-1_{arch}_{codename}.deb")
            }
            Self::FedoraRpm {
                version: fed_ver,
                arch,
            } => {
                format!("ultradesk-rust-{v}-1.fc{fed_ver}.{arch}.{fed_ver}.rpm")
            }
            Self::OpenSuseRpm { distro, arch } => {
                format!("ultradesk-rust-{v}-1.{arch}.{distro}.rpm")
            }
            Self::ArchPkg { arch } => {
                format!("ultradesk-rust-v{v}-{arch}-archlinux.tar.zst")
            }
            Self::NixOs => {
                format!("ultradesk-rust-{v}-x86_64-nixos.nar.xz")
            }
            Self::MacOsApp { arch } => {
                format!("ultradesk-rust-v{v}-{arch}-macos.dmg")
            }
            Self::MacOsHomebrew => String::new(),
            Self::PortableBinary { arch } => {
                format!("ultradesk-rust-v{v}-{arch}-linux.tar.gz")
            }
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::UbuntuDeb { .. } => "Ubuntu (.deb)",
            Self::DebianDeb { .. } => "Debian (.deb)",
            Self::FedoraRpm { .. } => "Fedora (.rpm)",
            Self::OpenSuseRpm { .. } => "openSUSE (.rpm)",
            Self::ArchPkg { .. } => "Arch Linux (.pkg.tar.zst)",
            Self::NixOs => "NixOS (Declarative / Flake)",
            Self::MacOsApp { .. } => "macOS (.dmg / .app)",
            Self::MacOsHomebrew => "macOS (Homebrew)",
            Self::PortableBinary { .. } => "Portable Linux Binary (tar.gz)",
        }
    }

    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::UbuntuDeb { .. } => "UbuntuDeb",
            Self::DebianDeb { .. } => "DebianDeb",
            Self::FedoraRpm { .. } => "FedoraRpm",
            Self::OpenSuseRpm { .. } => "OpenSuseRpm",
            Self::ArchPkg { .. } => "ArchPkg",
            Self::NixOs => "NixOs",
            Self::MacOsApp { .. } => "MacOsApp",
            Self::MacOsHomebrew => "MacOsHomebrew",
            Self::PortableBinary { .. } => "PortableBinary",
        }
    }

    pub fn find_matching_asset(&self, assets: &[Value], version: &str) -> Option<ReleaseAssetMeta> {
        let exact_name = self.match_asset_name(version);
        if !exact_name.is_empty() {
            for a in assets {
                let name = a.get("name").and_then(Value::as_str).unwrap_or("");
                if sanitize_asset_filename(name).is_err() {
                    continue;
                }
                if name == exact_name {
                    let url = a
                        .get("browser_download_url")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let size = a.get("size").and_then(Value::as_u64).unwrap_or(0);
                    let digest = a
                        .get("digest")
                        .and_then(Value::as_str)
                        .and_then(|d| d.strip_prefix("sha256:").map(str::to_string));
                    return Some(ReleaseAssetMeta {
                        name: name.to_string(),
                        download_url: url.to_string(),
                        expected_sha256: digest,
                        size_bytes: size,
                    });
                }
            }
        }

        for a in assets {
            let name = a.get("name").and_then(Value::as_str).unwrap_or("");
            if sanitize_asset_filename(name).is_err() {
                continue;
            }
            let url = a
                .get("browser_download_url")
                .and_then(Value::as_str)
                .unwrap_or("");
            let size = a.get("size").and_then(Value::as_u64).unwrap_or(0);
            let digest = a
                .get("digest")
                .and_then(Value::as_str)
                .and_then(|d| d.strip_prefix("sha256:").map(str::to_string));

            let matches = match self {
                Self::DebianDeb { arch, .. } | Self::UbuntuDeb { arch, .. } => {
                    name.ends_with(".deb") && name.contains(arch)
                }
                Self::FedoraRpm { arch, .. } => {
                    name.ends_with(".rpm")
                        && (name.contains("fc") || name.contains("fedora"))
                        && name.contains(arch)
                }
                Self::OpenSuseRpm { .. } => {
                    name.ends_with(".rpm")
                        && (name.contains("opensuse")
                            || name.contains("tumbleweed")
                            || name.contains("suse"))
                }
                Self::ArchPkg { arch } => {
                    name.ends_with(".tar.zst") && (name.contains("arch") || name.contains(arch))
                }
                Self::MacOsApp { arch } => {
                    (name.ends_with(".dmg") || name.ends_with(".app.tar.gz"))
                        && (name.contains("macos") || name.contains(arch))
                }
                Self::PortableBinary { arch } => {
                    name.ends_with(".tar.gz") && name.contains("linux") && name.contains(arch)
                }
                Self::NixOs => name.ends_with(".nar.xz"),
                Self::MacOsHomebrew => false,
            };

            if matches && !url.is_empty() {
                return Some(ReleaseAssetMeta {
                    name: name.to_string(),
                    download_url: url.to_string(),
                    expected_sha256: digest,
                    size_bytes: size,
                });
            }
        }

        for a in assets {
            let name = a.get("name").and_then(Value::as_str).unwrap_or("");
            if sanitize_asset_filename(name).is_err() {
                continue;
            }
            let url = a
                .get("browser_download_url")
                .and_then(Value::as_str)
                .unwrap_or("");
            let size = a.get("size").and_then(Value::as_u64).unwrap_or(0);
            let digest = a
                .get("digest")
                .and_then(Value::as_str)
                .and_then(|d| d.strip_prefix("sha256:").map(str::to_string));

            let matches = match self {
                Self::DebianDeb { .. } | Self::UbuntuDeb { .. } => name.ends_with(".deb"),
                Self::FedoraRpm { .. } | Self::OpenSuseRpm { .. } => name.ends_with(".rpm"),
                Self::ArchPkg { .. } => name.ends_with(".tar.zst"),
                Self::MacOsApp { .. } => name.ends_with(".dmg") || name.ends_with(".app.tar.gz"),
                Self::PortableBinary { .. } => {
                    name.ends_with(".tar.gz")
                        && !name.contains("macos")
                        && !name.ends_with(".app.tar.gz")
                }
                _ => false,
            };

            if matches && !url.is_empty() {
                return Some(ReleaseAssetMeta {
                    name: name.to_string(),
                    download_url: url.to_string(),
                    expected_sha256: digest,
                    size_bytes: size,
                });
            }
        }

        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseAssetMeta {
    pub name: String,
    pub download_url: String,
    pub expected_sha256: Option<String>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleasePlan {
    pub version: String,
    pub release_notes: String,
    pub release_url: String,
    pub asset: ReleaseAssetMeta,
    pub target: InstallTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateStatusSnapshot {
    pub state: UpdateState,
    pub current_version: String,
    pub latest_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_version: Option<String>,
    pub release_notes: Option<String>,
    pub release_url: Option<String>,
    pub download_progress_pct: Option<u8>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub install_target: String,
    pub install_target_kind: String,
    pub asset_name: Option<String>,
    pub staged_path: Option<String>,
    pub error: Option<String>,
    pub dismissed_version: Option<String>,
    pub message: Option<String>,
}

pub type UpdateStatus = UpdateStatusSnapshot;

impl Default for UpdateStatusSnapshot {
    fn default() -> Self {
        let t = InstallTarget::detect_current();
        Self {
            state: UpdateState::Idle,
            current_version: CURRENT_VERSION.to_string(),
            latest_version: None,
            available_version: None,
            release_notes: None,
            release_url: None,
            download_progress_pct: None,
            downloaded_bytes: 0,
            total_bytes: None,
            install_target: t.display_name().to_string(),
            install_target_kind: t.kind_str().to_string(),
            asset_name: None,
            staged_path: None,
            error: None,
            dismissed_version: None,
            message: None,
        }
    }
}

pub fn is_version_greater(remote: &str, current: &str) -> bool {
    let parse_nums = |s: &str| -> Vec<u64> {
        let clean = s.trim().trim_start_matches('v');
        clean
            .split('.')
            .map(|part| {
                part.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse::<u64>()
                    .unwrap_or(0)
            })
            .collect()
    };
    let r = parse_nums(remote);
    let c = parse_nums(current);
    let max_len = r.len().max(c.len());
    for i in 0..max_len {
        let r_val = r.get(i).copied().unwrap_or(0);
        let c_val = c.get(i).copied().unwrap_or(0);
        if r_val > c_val {
            return true;
        } else if r_val < c_val {
            return false;
        }
    }
    false
}

pub fn resolve_private_update_cache_dir(
    xdg_cache_home: Option<&str>,
    home: Option<&str>,
) -> PathBuf {
    let base = if let Some(cache) = xdg_cache_home {
        if !cache.trim().is_empty() {
            PathBuf::from(cache)
        } else {
            crate::profile::fallback_private_config_dir()
        }
    } else if let Some(h) = home {
        if !h.trim().is_empty() {
            PathBuf::from(h).join(".cache")
        } else {
            crate::profile::fallback_private_config_dir()
        }
    } else {
        crate::profile::fallback_private_config_dir()
    };
    let candidate = base.join("ultradesk-rust").join("updates");
    if crate::profile::make_private_dir(&candidate).is_ok() {
        return candidate;
    }
    let fallback_base = crate::profile::fallback_private_config_dir();
    let _ = crate::profile::make_private_dir(&fallback_base);
    let fallback = fallback_base.join("updates");
    let _ = crate::profile::make_private_dir(&fallback);
    fallback
}

pub fn get_private_update_cache_dir() -> PathBuf {
    let xdg = env::var("XDG_CACHE_HOME").ok();
    let home = env::var("HOME").ok();
    resolve_private_update_cache_dir(xdg.as_deref(), home.as_deref())
}

pub fn ensure_private_update_cache_dir() -> Result<PathBuf, String> {
    let dir = get_private_update_cache_dir();
    crate::profile::make_private_dir(&dir)?;
    Ok(dir)
}

enum TransportStream {
    Plain(TcpStream),
    Tls(TlsStream<TcpStream>),
}

impl Read for TransportStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Tls(s) => s.read(buf),
        }
    }
}

impl Write for TransportStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
        }
    }
}

struct ParsedUrl {
    is_tls: bool,
    host: String,
    port: u16,
    path_and_query: String,
}

const MAX_METADATA_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ASSET_BYTES: u64 = 256 * 1024 * 1024;

pub fn is_loopback_update_url(url: &str) -> bool {
    match parse_http_url(url.trim()) {
        Ok(parsed) => !parsed.is_tls && (parsed.host == "127.0.0.1" || parsed.host == "localhost"),
        Err(_) => false,
    }
}

fn is_allowed_remote_host(is_tls: bool, host: &str) -> bool {
    if !is_tls {
        return false;
    }
    let h = host.to_ascii_lowercase();
    h == "api.github.com"
        || h == "github.com"
        || h == "objects.githubusercontent.com"
        || h.ends_with(".githubusercontent.com")
        || h.ends_with(".github.com")
}

pub fn sanitize_asset_filename(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Asset filename cannot be empty".to_string());
    }
    if trimmed.starts_with('-') || trimmed.starts_with('.') {
        return Err(format!("Unsafe asset filename prefix: {}", trimmed));
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        return Err(format!(
            "Path traversal rejected in asset filename: {}",
            trimmed
        ));
    }
    if Path::new(trimmed).file_name().and_then(|n| n.to_str()) != Some(trimmed) {
        return Err(format!("Invalid asset basename: {}", trimmed));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
    {
        return Err(format!(
            "Disallowed characters in asset filename: {}",
            trimmed
        ));
    }
    Ok(trimmed.to_string())
}

fn parse_http_url(url: &str) -> Result<ParsedUrl, String> {
    let (is_tls, rest) = if let Some(stripped) = url.strip_prefix("https://") {
        (true, stripped)
    } else if let Some(stripped) = url.strip_prefix("http://") {
        (false, stripped)
    } else {
        return Err(format!(
            "Unsupported URL protocol (expected http:// or https://): {}",
            url
        ));
    };

    let (host_port, path_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };

    let (host, port) = match host_port.find(':') {
        Some(idx) => {
            let h = &host_port[..idx];
            let p = host_port[idx + 1..]
                .parse::<u16>()
                .map_err(|e| format!("Invalid port in URL {}: {}", url, e))?;
            (h.to_string(), p)
        }
        None => (host_port.to_string(), if is_tls { 443 } else { 80 }),
    };

    Ok(ParsedUrl {
        is_tls,
        host,
        port,
        path_and_query: path_query.to_string(),
    })
}

struct HttpResponse {
    status_code: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn get_header(&self, name: &str) -> Option<&str> {
        for (k, v) in &self.headers {
            if k.eq_ignore_ascii_case(name) {
                return Some(v.as_str());
            }
        }
        None
    }
}

fn execute_http_request(
    url: &str,
    max_bytes: u64,
    progress_callback: Option<&mut dyn FnMut(u64, Option<u64>)>,
) -> Result<HttpResponse, String> {
    let parsed = parse_http_url(url)?;
    let addr_str = format!("{}:{}", parsed.host, parsed.port);
    let addrs: Vec<_> = addr_str
        .to_socket_addrs()
        .map_err(|e| format!("DNS resolution failed for {}: {}", addr_str, e))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("No IP address resolved for {}", addr_str));
    }

    let tcp = TcpStream::connect_timeout(&addrs[0], Duration::from_secs(12))
        .map_err(|e| format!("TCP connection failed to {}: {}", addr_str, e))?;
    let _ = tcp.set_nodelay(true);
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(20)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(15)));

    let mut stream: TransportStream = if parsed.is_tls {
        let connector = TlsConnector::builder()
            .build()
            .map_err(|e| format!("TLS builder failed: {}", e))?;
        let tls = connector
            .connect(&parsed.host, tcp)
            .map_err(|e| format!("TLS handshake failed with {}: {}", parsed.host, e))?;
        TransportStream::Tls(tls)
    } else {
        TransportStream::Plain(tcp)
    };

    let host_header =
        if (parsed.is_tls && parsed.port == 443) || (!parsed.is_tls && parsed.port == 80) {
            parsed.host.clone()
        } else {
            format!("{}:{}", parsed.host, parsed.port)
        };
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: ultradesk-rust/{}\r\nAccept: application/vnd.github.v3+json, application/octet-stream, */*\r\nConnection: close\r\n\r\n",
        parsed.path_and_query, host_header, CURRENT_VERSION
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("Failed writing HTTP request: {}", e))?;
    stream
        .flush()
        .map_err(|e| format!("Failed flushing HTTP request: {}", e))?;

    let mut header_buf = Vec::new();
    let mut one = [0u8; 1];
    let mut header_ended = false;
    while header_buf.len() < 32768 {
        match stream.read(&mut one) {
            Ok(0) => break,
            Ok(_) => {
                header_buf.push(one[0]);
                if header_buf.ends_with(b"\r\n\r\n") || header_buf.ends_with(b"\n\n") {
                    header_ended = true;
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("Error reading HTTP header: {}", e)),
        }
    }

    if !header_ended {
        return Err("HTTP header incomplete or exceeds 32KB".to_string());
    }

    let header_text = String::from_utf8_lossy(&header_buf);
    let mut lines = header_text.lines();
    let status_line = lines.next().unwrap_or("");
    let status_code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);

    let mut headers = Vec::new();
    for l in lines {
        if l.is_empty() {
            continue;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }

    let resp = HttpResponse {
        status_code,
        headers,
        body: Vec::new(),
    };

    if status_code == 301
        || status_code == 302
        || status_code == 303
        || status_code == 307
        || status_code == 308
    {
        return Ok(resp);
    }

    let is_chunked = resp
        .get_header("transfer-encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false);
    let content_len = resp
        .get_header("content-length")
        .and_then(|v| v.parse::<u64>().ok());

    if let Some(len) = content_len {
        if len > max_bytes {
            return Err(format!(
                "HTTP response Content-Length ({} bytes) exceeds maximum allowed ({} bytes)",
                len, max_bytes
            ));
        }
    }

    let mut body = Vec::new();

    if is_chunked {
        let mut reader = BufReader::new(stream);
        let mut progress_acc = 0u64;
        let mut cb_opt = progress_callback;
        loop {
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .map_err(|e| format!("Failed reading chunk header: {}", e))?;
            let hex_str = line.trim();
            if hex_str.is_empty() {
                continue;
            }
            let chunk_size = usize::from_str_radix(hex_str, 16)
                .map_err(|e| format!("Invalid chunk size '{}': {}", hex_str, e))?;
            if chunk_size == 0 {
                let mut trailer = String::new();
                let _ = reader.read_line(&mut trailer);
                break;
            }
            if (body.len() as u64).saturating_add(chunk_size as u64) > max_bytes {
                return Err(format!(
                    "Chunked HTTP response exceeds maximum allowed size ({} bytes)",
                    max_bytes
                ));
            }
            let cur_len = body.len();
            body.resize(cur_len + chunk_size, 0u8);
            reader
                .read_exact(&mut body[cur_len..])
                .map_err(|e| format!("Failed reading chunk data: {}", e))?;
            let mut crlf = [0u8; 2];
            let _ = reader.read_exact(&mut crlf);
            progress_acc += chunk_size as u64;
            if let Some(ref mut cb) = cb_opt {
                cb(progress_acc, None);
            }
        }
    } else if let Some(len) = content_len {
        let mut progress_acc = 0u64;
        let mut cb_opt = progress_callback;
        let mut chunk = [0u8; 16384];
        while (body.len() as u64) < len {
            let to_read = (len - body.len() as u64).min(chunk.len() as u64) as usize;
            match stream.read(&mut chunk[..to_read]) {
                Ok(0) => break,
                Ok(n) => {
                    body.extend_from_slice(&chunk[..n]);
                    progress_acc += n as u64;
                    if let Some(ref mut cb) = cb_opt {
                        cb(progress_acc, Some(len));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(format!("Error reading body stream: {}", e)),
            }
        }
        if (body.len() as u64) != len {
            return Err(format!(
                "Truncated HTTP response body: expected {} bytes, got {}",
                len,
                body.len()
            ));
        }
    } else {
        let mut chunk = [0u8; 16384];
        let mut progress_acc = 0u64;
        let mut cb_opt = progress_callback;
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if (body.len() as u64).saturating_add(n as u64) > max_bytes {
                        return Err(format!(
                            "HTTP response body exceeds maximum allowed size ({} bytes)",
                            max_bytes
                        ));
                    }
                    body.extend_from_slice(&chunk[..n]);
                    progress_acc += n as u64;
                    if let Some(ref mut cb) = cb_opt {
                        cb(progress_acc, None);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(format!("Error reading body stream: {}", e)),
            }
        }
    }

    Ok(HttpResponse {
        status_code,
        headers: resp.headers,
        body,
    })
}

fn validate_redirect_or_fetch_url(
    initial_is_loopback: bool,
    candidate_url: &str,
) -> Result<(), String> {
    let parsed = parse_http_url(candidate_url)?;
    if initial_is_loopback {
        if !parsed.is_tls && (parsed.host == "127.0.0.1" || parsed.host == "localhost") {
            return Ok(());
        }
        return Err(format!(
            "Loopback update source cannot redirect or fetch non-loopback URL: {}",
            candidate_url
        ));
    }
    if is_allowed_remote_host(parsed.is_tls, &parsed.host) {
        return Ok(());
    }
    Err(format!(
        "Disallowed update URL host or scheme: {}",
        candidate_url
    ))
}

fn fetch_with_redirects(
    url: &str,
    max_hops: usize,
    max_bytes: u64,
    mut progress_callback: Option<&mut dyn FnMut(u64, Option<u64>)>,
) -> Result<HttpResponse, String> {
    let initial_parsed = parse_http_url(url)?;
    let initial_is_loopback = !initial_parsed.is_tls
        && (initial_parsed.host == "127.0.0.1" || initial_parsed.host == "localhost");
    validate_redirect_or_fetch_url(initial_is_loopback, url)?;

    let mut current_url = url.to_string();
    for _ in 0..=max_hops {
        validate_redirect_or_fetch_url(initial_is_loopback, &current_url)?;
        let cb = progress_callback
            .as_mut()
            .map(|cb| &mut **cb as &mut dyn FnMut(u64, Option<u64>));
        let resp = execute_http_request(&current_url, max_bytes, cb)?;
        if [301, 302, 303, 307, 308].contains(&resp.status_code) {
            let loc = resp
                .get_header("location")
                .ok_or_else(|| format!("HTTP {} without Location header", resp.status_code))?;
            if loc.starts_with("http://") || loc.starts_with("https://") {
                current_url = loc.to_string();
            } else if loc.starts_with('/') {
                let parsed = parse_http_url(&current_url)?;
                let scheme = if parsed.is_tls { "https" } else { "http" };
                current_url = format!("{}://{}:{}{}", scheme, parsed.host, parsed.port, loc);
            } else {
                return Err(format!("Unsupported relative redirect URL: {}", loc));
            }
            validate_redirect_or_fetch_url(initial_is_loopback, &current_url)?;
            continue;
        }
        return Ok(resp);
    }
    Err(format!("Too many redirects (> {} hops)", max_hops))
}

pub struct UpdateEngine {
    api_url: Mutex<String>,
    target: InstallTarget,
    current_version: String,
    dismissed_version: Mutex<Option<String>>,
    cached_plan: Mutex<Option<ReleasePlan>>,
    staged_package_path: Mutex<Option<PathBuf>>,
    status: Mutex<UpdateStatusSnapshot>,
}

impl Default for UpdateEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateEngine {
    pub fn new() -> Self {
        let target = InstallTarget::detect_current();
        let api_url = env::var("ULTRADESK_UPDATE_CHECK_URL")
            .unwrap_or_else(|_| DEFAULT_GITHUB_API_URL.to_string());
        let initial_status = UpdateStatusSnapshot {
            install_target: target.display_name().to_string(),
            install_target_kind: target.kind_str().to_string(),
            ..Default::default()
        };
        Self {
            api_url: Mutex::new(api_url),
            target,
            current_version: CURRENT_VERSION.to_string(),
            dismissed_version: Mutex::new(None),
            cached_plan: Mutex::new(None),
            staged_package_path: Mutex::new(None),
            status: Mutex::new(initial_status),
        }
    }

    pub fn with_api_url(url: String) -> Self {
        let engine = Self::new();
        engine.set_api_url(url);
        engine
    }

    pub fn with_target_and_url(target: InstallTarget, url: String) -> Self {
        let initial_status = UpdateStatusSnapshot {
            install_target: target.display_name().to_string(),
            install_target_kind: target.kind_str().to_string(),
            ..Default::default()
        };
        Self {
            api_url: Mutex::new(url),
            target,
            current_version: CURRENT_VERSION.to_string(),
            dismissed_version: Mutex::new(None),
            cached_plan: Mutex::new(None),
            staged_package_path: Mutex::new(None),
            status: Mutex::new(initial_status),
        }
    }

    pub fn dismiss_update(&self, ver: Option<&str>) {
        self.set_dismissed_version(ver.map(str::to_string));
    }

    pub fn set_api_url(&self, url: String) {
        let mut u = self.api_url.lock().unwrap_or_else(|e| e.into_inner());
        *u = url;
    }

    pub fn api_url(&self) -> String {
        self.api_url
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_dismissed_version(&self, ver: Option<String>) {
        let mut d = self
            .dismissed_version
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *d = ver.clone();
        let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
        st.dismissed_version = ver;
    }

    pub fn dismiss_version(&self, ver: &str) {
        self.set_dismissed_version(Some(ver.to_string()));
    }

    pub fn snapshot(&self) -> UpdateStatusSnapshot {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn target(&self) -> &InstallTarget {
        &self.target
    }

    pub fn check_for_updates(&self) -> Result<UpdateStatusSnapshot, String> {
        {
            let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(
                st.state,
                UpdateState::Checking | UpdateState::Downloading | UpdateState::Applying
            ) {
                return Err("An update operation is already in progress".to_string());
            }
            st.state = UpdateState::Checking;
            st.error = None;
            st.message = None;
        }

        let res = self.check_for_updates_inner();
        if let Err(ref e) = res {
            let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
            st.state = UpdateState::Error;
            st.error = Some(e.clone());
        }
        res
    }

    fn check_for_updates_inner(&self) -> Result<UpdateStatusSnapshot, String> {
        let current_api = self.api_url();
        let resp = fetch_with_redirects(&current_api, 5, MAX_METADATA_BYTES, None)?;

        if resp.status_code != 200 {
            return Err(format!(
                "HTTP error from release server: {}",
                resp.status_code
            ));
        }

        let resp_json: Value = serde_json::from_slice(&resp.body)
            .map_err(|e| format!("Invalid release JSON payload: {}", e))?;

        let tag_name = resp_json
            .get("tag_name")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing tag_name in release payload".to_string())?;
        let remote_ver = tag_name.trim_start_matches('v');
        let body = resp_json
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("No release summary provided.");
        let release_url = resp_json
            .get("html_url")
            .and_then(Value::as_str)
            .map(str::to_string);

        if !is_version_greater(remote_ver, &self.current_version) {
            let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
            st.state = UpdateState::UpToDate;
            st.latest_version = Some(remote_ver.to_string());
            st.available_version = Some(remote_ver.to_string());
            st.release_notes = Some(body.to_string());
            st.release_url = release_url;
            return Ok(st.clone());
        }

        let assets = resp_json
            .get("assets")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut matched_asset = self.target.find_matching_asset(&assets, remote_ver);

        let sha256sums_url = assets.iter().find_map(|a| {
            let name = a.get("name").and_then(Value::as_str).unwrap_or("");
            if name == "SHA256SUMS.txt" {
                a.get("browser_download_url")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            } else {
                None
            }
        });

        if let Some(ref mut asset) = matched_asset {
            if asset.expected_sha256.is_none() {
                if let Some(sums_url) = sha256sums_url {
                    if let Ok(sums_resp) =
                        fetch_with_redirects(&sums_url, 5, MAX_METADATA_BYTES, None)
                    {
                        if sums_resp.status_code == 200 {
                            if let Ok(sums_text) = std::str::from_utf8(&sums_resp.body) {
                                for line in sums_text.lines() {
                                    let parts: Vec<&str> = line.split_whitespace().collect();
                                    if parts.len() >= 2 {
                                        let raw_part = parts[1]
                                            .trim_start_matches('*')
                                            .trim_start_matches("./");
                                        let base_part = Path::new(raw_part)
                                            .file_name()
                                            .and_then(|s| s.to_str())
                                            .unwrap_or(raw_part);
                                        if base_part == asset.name
                                            || raw_part == asset.name
                                            || parts[1] == asset.name
                                        {
                                            asset.expected_sha256 =
                                                Some(parts[0].to_ascii_lowercase());
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let plan = ReleasePlan {
            version: remote_ver.to_string(),
            release_notes: body.to_string(),
            release_url: release_url.clone().unwrap_or_default(),
            asset: matched_asset.unwrap_or_else(|| ReleaseAssetMeta {
                name: self.target.match_asset_name(remote_ver),
                download_url: release_url.clone().unwrap_or_default(),
                expected_sha256: None,
                size_bytes: 0,
            }),
            target: self.target.clone(),
        };

        {
            let mut cached = self.cached_plan.lock().unwrap_or_else(|e| e.into_inner());
            *cached = Some(plan.clone());
        }

        let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
        st.state = UpdateState::Available;
        st.latest_version = Some(remote_ver.to_string());
        st.available_version = Some(remote_ver.to_string());
        st.release_notes = Some(body.to_string());
        st.release_url = release_url;
        st.asset_name = if !plan.asset.name.is_empty() {
            Some(plan.asset.name.clone())
        } else {
            None
        };
        st.total_bytes = if plan.asset.size_bytes > 0 {
            Some(plan.asset.size_bytes)
        } else {
            None
        };
        st.downloaded_bytes = 0;
        st.download_progress_pct = None;
        st.error = None;

        Ok(st.clone())
    }

    pub fn confirm_and_apply(&self) -> Result<String, String> {
        let status = self.confirm_and_apply_for_version(None)?;
        Ok(status.message.unwrap_or_default())
    }

    pub fn confirm_and_apply_for_version(
        &self,
        expected_version: Option<&str>,
    ) -> Result<UpdateStatus, String> {
        {
            let st = self.status.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(
                st.state,
                UpdateState::Checking | UpdateState::Downloading | UpdateState::Applying
            ) {
                return Err("An update operation is already in progress".to_string());
            }
            if let Some(exp_ver) = expected_version {
                let avail = st
                    .available_version
                    .as_deref()
                    .or(st.latest_version.as_deref());
                match avail {
                    Some(a) if a == exp_ver => {}
                    Some(a) => {
                        return Err(format!(
                            "Expected update version '{}' does not match available version '{}'",
                            exp_ver, a
                        ));
                    }
                    None => {
                        return Err(format!(
                            "Expected update version '{}', but no update is currently available",
                            exp_ver
                        ));
                    }
                }
            }
        }

        let res = self.confirm_and_apply_inner(expected_version);
        if let Err(ref e) = res {
            let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
            st.state = UpdateState::Error;
            st.error = Some(e.clone());
            return Err(e.clone());
        }
        Ok(self.snapshot())
    }

    fn confirm_and_apply_inner(&self, expected_version: Option<&str>) -> Result<String, String> {
        let plan = {
            let cached = self.cached_plan.lock().unwrap_or_else(|e| e.into_inner());
            cached.clone().ok_or_else(|| {
                "No update available to confirm. Check for updates first.".to_string()
            })?
        };

        if let Some(exp_ver) = expected_version {
            if plan.version != exp_ver {
                return Err(format!(
                    "Expected update version '{}' does not match plan version '{}'",
                    exp_ver, plan.version
                ));
            }
        }

        if plan.asset.download_url.is_empty() || plan.asset.download_url == plan.release_url {
            let msg = match &plan.target {
                InstallTarget::MacOsHomebrew => {
                    if Command::new("which")
                        .arg("brew")
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false)
                    {
                        let _ = Command::new("brew")
                            .args(["upgrade", "ultradesk-rust"])
                            .status();
                        "Initiated Homebrew upgrade via 'brew upgrade ultradesk-rust'.".to_string()
                    } else {
                        "Run: brew upgrade ultradesk-rust".to_string()
                    }
                }
                InstallTarget::NixOs => {
                    format!("Declarative NixOS update available at {}. Update your flake or system derivation.", plan.release_url)
                }
                _other => {
                    format!(
                        "New release {} available at {}",
                        plan.version, plan.release_url
                    )
                }
            };
            let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
            st.state = UpdateState::Applied;
            st.message = Some(msg.clone());
            return Ok(msg);
        }

        let safe_asset_name = sanitize_asset_filename(&plan.asset.name)?;
        let expected_sha256 = match plan.asset.expected_sha256.as_deref() {
            Some(exp) if exp.len() == 64 && exp.chars().all(|c| c.is_ascii_hexdigit()) => exp,
            Some(_) => {
                return Err(format!(
                    "Invalid SHA-256 digest format for update asset '{}'",
                    safe_asset_name
                ));
            }
            None => {
                return Err(format!(
                    "Refusing update: no expected SHA-256 checksum found for asset '{}'",
                    safe_asset_name
                ));
            }
        };
        let api_is_loopback = is_loopback_update_url(&self.api_url());
        validate_redirect_or_fetch_url(api_is_loopback, &plan.asset.download_url)?;

        {
            let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
            st.state = UpdateState::Downloading;
            st.downloaded_bytes = 0;
            st.download_progress_pct = Some(0);
            st.error = None;
        }

        let cache_dir = ensure_private_update_cache_dir()?;
        let staged_file = cache_dir.join(&safe_asset_name);

        let mut progress_fn = |downloaded: u64, total: Option<u64>| {
            if let Ok(mut st) = self.status.lock() {
                st.downloaded_bytes = downloaded;
                if let Some(t) = total.or(st.total_bytes) {
                    st.total_bytes = Some(t);
                    if t > 0 {
                        let pct = ((downloaded as f64 / t as f64) * 100.0).clamp(0.0, 100.0) as u8;
                        st.download_progress_pct = Some(pct);
                    }
                }
            }
        };

        let resp = fetch_with_redirects(
            &plan.asset.download_url,
            5,
            MAX_ASSET_BYTES,
            Some(&mut progress_fn),
        )?;

        if resp.status_code != 200 {
            return Err(format!(
                "Failed to download update asset (HTTP {})",
                resp.status_code
            ));
        }

        let mut hasher = Sha256::new();
        hasher.update(&resp.body);
        let hash_bytes: [u8; 32] = hasher.finalize().into();
        let calculated: String = hash_bytes.iter().map(|b| format!("{:02x}", b)).collect();
        if !calculated.eq_ignore_ascii_case(expected_sha256) {
            return Err(format!(
                "SHA-256 verification failed for '{}'! Expected {}, computed {}",
                safe_asset_name, expected_sha256, calculated
            ));
        }

        crate::profile::write_private_bytes(&staged_file, &resp.body)
            .map_err(|e| format!("Failed to stage update package: {}", e))?;

        {
            let mut sp = self
                .staged_package_path
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *sp = Some(staged_file.clone());
            let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
            st.state = UpdateState::Ready;
            st.download_progress_pct = Some(100);
            st.downloaded_bytes = resp.body.len() as u64;
            st.staged_path = Some(staged_file.to_string_lossy().to_string());
        }

        {
            let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
            st.state = UpdateState::Applying;
        }

        let msg = self.execute_install_strategy(&plan, &staged_file)?;
        let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
        st.state = UpdateState::Applied;
        st.message = Some(msg.clone());
        st.error = None;
        Ok(msg)
    }

    fn execute_install_strategy(
        &self,
        plan: &ReleasePlan,
        pkg_path: &Path,
    ) -> Result<String, String> {
        let is_dry_run = env::var("ULTRADESK_UPDATE_DRY_RUN")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        if is_dry_run {
            return Ok(
                "Simulation mode: update package staged and verified successfully".to_string(),
            );
        }

        let is_root = {
            #[cfg(unix)]
            unsafe {
                libc::geteuid() == 0
            }
            #[cfg(not(unix))]
            false
        };

        let has_display = env::var("DISPLAY")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
            || env::var("WAYLAND_DISPLAY")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);

        let has_pkexec = Command::new("which")
            .arg("pkexec")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
            || Path::new("/usr/bin/pkexec").exists();

        match &plan.target {
            InstallTarget::UbuntuDeb { .. } | InstallTarget::DebianDeb { .. } => {
                if is_root {
                    let status = Command::new("apt-get")
                        .args(["install", "-y", "--reinstall", "--"])
                        .arg(pkg_path)
                        .status()
                        .map_err(|e| format!("Failed to spawn apt-get: {}", e))?;
                    if !status.success() {
                        return Err(format!("apt-get install exited with status {}", status));
                    }
                    Ok("Debian/Ubuntu package installed successfully".to_string())
                } else if has_display && has_pkexec {
                    let status = Command::new("pkexec")
                        .args(["apt-get", "install", "-y", "--reinstall", "--"])
                        .arg(pkg_path)
                        .status()
                        .map_err(|e| format!("Failed to spawn pkexec apt-get: {}", e))?;
                    if !status.success() {
                        return Err(format!("pkexec apt-get exited with status {}", status));
                    }
                    Ok("Debian/Ubuntu package installed successfully via pkexec".to_string())
                } else {
                    Ok(format!(
                        "Update package downloaded and verified at {}. Install with: sudo apt install --reinstall {}",
                        pkg_path.display(),
                        pkg_path.display()
                    ))
                }
            }
            InstallTarget::FedoraRpm { .. } => {
                if is_root {
                    let status = Command::new("dnf")
                        .args(["install", "-y", "--"])
                        .arg(pkg_path)
                        .status()
                        .map_err(|e| format!("Failed to spawn dnf: {}", e))?;
                    if !status.success() {
                        return Err(format!("dnf install exited with status {}", status));
                    }
                    Ok("Fedora RPM package installed successfully".to_string())
                } else if has_display && has_pkexec {
                    let status = Command::new("pkexec")
                        .args(["dnf", "install", "-y", "--"])
                        .arg(pkg_path)
                        .status()
                        .map_err(|e| format!("Failed to spawn pkexec dnf: {}", e))?;
                    if !status.success() {
                        return Err(format!("pkexec dnf exited with status {}", status));
                    }
                    Ok("Fedora RPM package installed successfully via pkexec".to_string())
                } else {
                    Ok(format!(
                        "Update package downloaded and verified at {}. Install with: sudo dnf install -y {}",
                        pkg_path.display(),
                        pkg_path.display()
                    ))
                }
            }
            InstallTarget::OpenSuseRpm { .. } => {
                if is_root {
                    let status = Command::new("zypper")
                        .args(["--non-interactive", "install", "-y", "--"])
                        .arg(pkg_path)
                        .status()
                        .map_err(|e| format!("Failed to spawn zypper: {}", e))?;
                    if !status.success() {
                        return Err(format!("zypper install exited with status {}", status));
                    }
                    Ok("openSUSE RPM package installed successfully".to_string())
                } else if has_display && has_pkexec {
                    let status = Command::new("pkexec")
                        .args(["zypper", "--non-interactive", "install", "-y", "--"])
                        .arg(pkg_path)
                        .status()
                        .map_err(|e| format!("Failed to spawn pkexec zypper: {}", e))?;
                    if !status.success() {
                        return Err(format!("pkexec zypper exited with status {}", status));
                    }
                    Ok("openSUSE RPM package installed successfully via pkexec".to_string())
                } else {
                    Ok(format!(
                        "Update package downloaded and verified at {}. Install with: sudo zypper install {}",
                        pkg_path.display(),
                        pkg_path.display()
                    ))
                }
            }
            InstallTarget::ArchPkg { .. } => {
                let is_pacman_pkg = pkg_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".pkg.tar.zst") || n.ends_with(".pkg.tar.xz"))
                    .unwrap_or(false);
                if !is_pacman_pkg {
                    return Err(format!(
                        "Refusing unmanaged root extraction. Package '{}' is not an Arch package (.pkg.tar.zst/.pkg.tar.xz). Build or install with pacman or your AUR helper.",
                        pkg_path.display()
                    ));
                }
                if is_root {
                    let status = Command::new("pacman")
                        .args(["-U", "--noconfirm", "--"])
                        .arg(pkg_path)
                        .status()
                        .map_err(|e| format!("Failed to spawn pacman: {}", e))?;
                    if !status.success() {
                        return Err(format!("pacman exited with status {}", status));
                    }
                    Ok("Arch Linux package installed successfully".to_string())
                } else if has_display && has_pkexec {
                    let status = Command::new("pkexec")
                        .args(["pacman", "-U", "--noconfirm", "--"])
                        .arg(pkg_path)
                        .status()
                        .map_err(|e| format!("Failed to spawn pkexec pacman: {}", e))?;
                    if !status.success() {
                        return Err(format!("pkexec pacman exited with status {}", status));
                    }
                    Ok("Arch Linux package installed successfully via pkexec".to_string())
                } else {
                    Ok(format!(
                        "Update package downloaded and verified at {}. Install with: sudo pacman -U {}",
                        pkg_path.display(),
                        pkg_path.display()
                    ))
                }
            }
            InstallTarget::PortableBinary { .. } => {
                let current_exe = env::current_exe()
                    .map_err(|e| format!("Could not determine current executable path: {}", e))?;
                let tmp_exe = current_exe.with_extension(format!("tmp.{}", std::process::id()));
                let pkg_name = pkg_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                let raw_bytes = fs::read(pkg_path)
                    .map_err(|e| format!("Failed to read staged package: {}", e))?;
                let is_gzip = raw_bytes.starts_with(&[0x1f, 0x8b]);
                let is_tar_gz =
                    pkg_name.ends_with(".tar.gz") || pkg_name.ends_with(".tgz") || is_gzip;

                let exe_bytes = if is_tar_gz {
                    let cache_dir = ensure_private_update_cache_dir()?;
                    let extract_dir = cache_dir.join(format!("extract-{}", std::process::id()));
                    let _ = fs::remove_dir_all(&extract_dir);
                    crate::profile::make_private_dir(&extract_dir)?;
                    let status = Command::new("tar")
                        .arg("-xzf")
                        .arg(pkg_path)
                        .arg("-C")
                        .arg(&extract_dir)
                        .status()
                        .map_err(|e| {
                            let _ = fs::remove_dir_all(&extract_dir);
                            format!("Failed to extract portable tar.gz archive: {}", e)
                        })?;
                    if !status.success() {
                        let _ = fs::remove_dir_all(&extract_dir);
                        return Err(format!("tar -xzf exited with status {}", status));
                    }
                    let candidate_root = extract_dir.join("ultradesk-rust");
                    let candidate_usr = extract_dir.join("usr").join("bin").join("ultradesk-rust");
                    let bin_path = if candidate_root.is_file() {
                        candidate_root
                    } else if candidate_usr.is_file() {
                        candidate_usr
                    } else {
                        let _ = fs::remove_dir_all(&extract_dir);
                        return Err(
                            "Extracted archive does not contain ultradesk-rust binary".to_string(),
                        );
                    };
                    let extracted = fs::read(&bin_path).map_err(|e| {
                        let _ = fs::remove_dir_all(&extract_dir);
                        format!("Failed reading extracted binary: {}", e)
                    })?;
                    let _ = fs::remove_dir_all(&extract_dir);
                    extracted
                } else {
                    raw_bytes
                };

                let is_valid_binary = exe_bytes.starts_with(b"\x7fELF")
                    || exe_bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
                    || exe_bytes.starts_with(&[0xfe, 0xed, 0xfa, 0xcf])
                    || exe_bytes.starts_with(&[0xca, 0xfe, 0xba, 0xbe]);
                if !is_valid_binary {
                    return Err(
                        "Staged portable binary is not a valid ELF or Mach-O executable"
                            .to_string(),
                    );
                }

                fs::write(&tmp_exe, &exe_bytes).map_err(|e| {
                    format!(
                        "Failed writing binary replacement to {}: {}",
                        tmp_exe.display(),
                        e
                    )
                })?;

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&tmp_exe, fs::Permissions::from_mode(0o755));
                }

                fs::rename(&tmp_exe, &current_exe)
                    .map_err(|e| format!("Atomic binary rename failed: {}", e))?;

                Ok(format!(
                    "Portable binary upgraded successfully at {}",
                    current_exe.display()
                ))
            }
            InstallTarget::MacOsApp { .. } => {
                let app_dest = Path::new("/Applications/UltraDesk.app");
                let target_app_dir = if app_dest.parent().map(|p| p.is_dir()).unwrap_or(false) {
                    app_dest.to_path_buf()
                } else if let Ok(home) = env::var("HOME") {
                    PathBuf::from(home).join("Applications").join("UltraDesk.app")
                } else {
                    app_dest.to_path_buf()
                };

                let pkg_name = pkg_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                if pkg_name.ends_with(".app.tar.gz") {
                    let parent_dir = target_app_dir
                        .parent()
                        .unwrap_or_else(|| Path::new("/Applications"));
                    let status = Command::new("tar")
                        .arg("-xzf")
                        .arg(pkg_path)
                        .arg("-C")
                        .arg(parent_dir)
                        .status()
                        .map_err(|e| format!("Failed extracting macOS app archive: {}", e))?;
                    if !status.success() {
                        return Err(format!("tar -xzf exited with status {}", status));
                    }
                    return Ok(format!(
                        "macOS application updated to {}",
                        target_app_dir.display()
                    ));
                }

                let attach = Command::new("hdiutil")
                    .args(["attach", "-nobrowse"])
                    .arg(pkg_path)
                    .output()
                    .map_err(|e| format!("Failed to execute hdiutil attach: {}", e))?;
                if !attach.status.success() {
                    return Err(format!(
                        "hdiutil attach failed: {}",
                        String::from_utf8_lossy(&attach.stderr)
                    ));
                }

                let mounted_app = Path::new("/Volumes/UltraDesk/UltraDesk.app");
                let copy_res = Command::new("ditto")
                    .arg(mounted_app)
                    .arg(&target_app_dir)
                    .output();

                let _ = Command::new("hdiutil")
                    .args(["detach", "/Volumes/UltraDesk", "-force"])
                    .status();

                match copy_res {
                    Ok(out) if out.status.success() => Ok(format!(
                        "macOS application updated to {}",
                        target_app_dir.display()
                    )),
                    Ok(out) => Err(format!(
                        "ditto copy to {} failed: {}",
                        target_app_dir.display(),
                        String::from_utf8_lossy(&out.stderr)
                    )),
                    Err(e) => Err(format!("Failed to execute ditto: {}", e)),
                }
            }
            InstallTarget::MacOsHomebrew => {
                if Command::new("which")
                    .arg("brew")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
                {
                    let status = Command::new("brew")
                        .args(["upgrade", "ultradesk-rust"])
                        .status()
                        .map_err(|e| format!("Failed to run brew upgrade: {}", e))?;
                    if !status.success() {
                        return Err(format!("brew upgrade exited with status {}", status));
                    }
                    Ok("Upgraded via brew upgrade ultradesk-rust".to_string())
                } else {
                    Ok("Run: brew upgrade ultradesk-rust".to_string())
                }
            }
            InstallTarget::NixOs => Ok(format!(
                "Update package verified at {}. Run nix flake update or update system configuration.",
                pkg_path.display()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_is_version_greater_semver() {
        assert!(is_version_greater("1.0.6", "1.0.5"));
        assert!(!is_version_greater("1.0.5", "1.0.5"));
        assert!(!is_version_greater("1.0.4", "1.0.5"));
        assert!(is_version_greater("1.1.0", "1.0.9"));
        assert!(is_version_greater("2.0.0", "1.99.99"));
        assert!(is_version_greater("v1.0.6", "1.0.5"));
        assert!(is_version_greater("1.0.5.1", "1.0.5"));
        assert!(!is_version_greater("1.0.5", "1.0.5.1"));
    }

    #[test]
    fn test_install_target_asset_names() {
        let udeb = InstallTarget::UbuntuDeb {
            codename: "noble".into(),
            arch: "amd64".into(),
        };
        assert_eq!(
            udeb.match_asset_name("1.0.6"),
            "ultradesk-rust_1.0.6-1_amd64_noble.deb"
        );

        let ddeb = InstallTarget::DebianDeb {
            codename: "bookworm".into(),
            arch: "amd64".into(),
        };
        assert_eq!(
            ddeb.match_asset_name("1.0.6"),
            "ultradesk-rust_1.0.6-1_amd64_bookworm.deb"
        );

        let frpm = InstallTarget::FedoraRpm {
            version: "43".into(),
            arch: "x86_64".into(),
        };
        assert_eq!(
            frpm.match_asset_name("1.0.6"),
            "ultradesk-rust-1.0.6-1.fc43.x86_64.43.rpm"
        );

        let srpm = InstallTarget::OpenSuseRpm {
            distro: "opensuse".into(),
            arch: "x86_64".into(),
        };
        assert_eq!(
            srpm.match_asset_name("1.0.6"),
            "ultradesk-rust-1.0.6-1.x86_64.opensuse.rpm"
        );

        let apkg = InstallTarget::ArchPkg {
            arch: "x86_64".into(),
        };
        assert_eq!(
            apkg.match_asset_name("1.0.6"),
            "ultradesk-rust-v1.0.6-x86_64-archlinux.tar.zst"
        );

        let nix = InstallTarget::NixOs;
        assert_eq!(
            nix.match_asset_name("1.0.6"),
            "ultradesk-rust-1.0.6-x86_64-nixos.nar.xz"
        );

        let mac = InstallTarget::MacOsApp {
            arch: "x86_64".into(),
        };
        assert_eq!(
            mac.match_asset_name("1.0.6"),
            "ultradesk-rust-v1.0.6-x86_64-macos.dmg"
        );

        let port = InstallTarget::PortableBinary {
            arch: "x86_64".into(),
        };
        assert_eq!(
            port.match_asset_name("1.0.6"),
            "ultradesk-rust-v1.0.6-x86_64-linux.tar.gz"
        );
    }

    #[test]
    fn test_find_matching_asset_exact_and_fallback() {
        let target = InstallTarget::UbuntuDeb {
            codename: "noble".into(),
            arch: "amd64".into(),
        };

        let assets = serde_json::json!([
            {
                "name": "ultradesk-rust_1.0.6-1_amd64_noble.deb",
                "browser_download_url": "http://127.0.0.1/downloads/noble.deb",
                "size": 1024,
                "digest": "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            }
        ]);
        let matched = target.find_matching_asset(assets.as_array().unwrap(), "1.0.6");
        assert!(matched.is_some());
        let m = matched.unwrap();
        assert_eq!(m.name, "ultradesk-rust_1.0.6-1_amd64_noble.deb");
        assert_eq!(
            m.expected_sha256.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );

        let fallback_assets = serde_json::json!([
            {
                "name": "ultradesk-rust-generic-amd64.deb",
                "browser_download_url": "http://127.0.0.1/downloads/generic.deb",
                "size": 2048
            }
        ]);
        let fallback_matched =
            target.find_matching_asset(fallback_assets.as_array().unwrap(), "1.0.6");
        assert!(fallback_matched.is_some());
        assert_eq!(
            fallback_matched.unwrap().name,
            "ultradesk-rust-generic-amd64.deb"
        );
    }

    #[test]
    fn test_loopback_mock_update_flow() {
        unsafe {
            env::set_var("ULTRADESK_UPDATE_DRY_RUN", "1");
        }

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().unwrap().port();
        let running = std::sync::Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let payload_bytes = b"MOCK_DEB_PACKAGE_BINARY_DATA_FOR_UNIT_TEST";
        let mut hasher = Sha256::new();
        hasher.update(payload_bytes);
        let hash_array: [u8; 32] = hasher.finalize().into();
        let mut expected_hex = String::with_capacity(64);
        for byte in hash_array {
            use std::fmt::Write as _;
            let _ = write!(&mut expected_hex, "{:02x}", byte);
        }

        let asset_name = "ultradesk-rust_9.9.9-1_amd64_noble.deb";
        let sums_content = format!("{}  ./deb-ubuntu-24.04/{}\n", expected_hex, asset_name);

        let release_json = serde_json::json!({
            "tag_name": "v9.9.9",
            "html_url": format!("http://127.0.0.1:{}/releases/v9.9.9", port),
            "body": "Mock release notes for 9.9.9",
            "assets": [
                {
                    "name": asset_name,
                    "browser_download_url": format!("http://127.0.0.1:{}/redirect-deb", port),
                    "size": payload_bytes.len()
                },
                {
                    "name": "SHA256SUMS.txt",
                    "browser_download_url": format!("http://127.0.0.1:{}/sha256sums", port),
                    "size": sums_content.len()
                }
            ]
        })
        .to_string();

        let server_thread = thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while running_clone.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buf = [0u8; 4096];
                        let n = stream.read(&mut buf).unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]);
                        if req.contains("GET /redirect-deb ") {
                            let resp = format!(
                                "HTTP/1.1 302 Found
Location: http://127.0.0.1:{}/real-deb
Content-Length: 0
Connection: close

",
                                port
                            );
                            let _ = stream.write_all(resp.as_bytes());
                        } else if req.contains("GET /real-deb ") {
                            let resp_head = format!(
                                "HTTP/1.1 200 OK
Content-Length: {}
Content-Type: application/octet-stream
Connection: close

",
                                payload_bytes.len()
                            );
                            let _ = stream.write_all(resp_head.as_bytes());
                            let _ = stream.write_all(payload_bytes);
                        } else if req.contains("GET /sha256sums ") {
                            let resp = format!(
                                "HTTP/1.1 200 OK
Content-Length: {}
Content-Type: text/plain
Connection: close

{}",
                                sums_content.len(),
                                sums_content
                            );
                            let _ = stream.write_all(resp.as_bytes());
                        } else {
                            let resp = format!(
                                "HTTP/1.1 200 OK
Content-Length: {}
Content-Type: application/json
Connection: close

{}",
                                release_json.len(),
                                release_json
                            );
                            let _ = stream.write_all(resp.as_bytes());
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        let mock_url = format!("http://127.0.0.1:{}/api/releases/latest", port);
        let engine = UpdateEngine::with_target_and_url(
            InstallTarget::UbuntuDeb {
                codename: "noble".into(),
                arch: "amd64".into(),
            },
            mock_url.clone(),
        );

        let snap_check = engine
            .check_for_updates()
            .expect("check updates should succeed");
        assert_eq!(snap_check.state, UpdateState::Available);
        assert_eq!(snap_check.latest_version.as_deref(), Some("9.9.9"));
        assert_eq!(snap_check.asset_name.as_deref(), Some(asset_name));

        engine.dismiss_update(Some("9.9.9"));
        let snap_dismissed = engine.snapshot();
        assert_eq!(snap_dismissed.dismissed_version.as_deref(), Some("9.9.9"));

        let apply_msg = engine
            .confirm_and_apply()
            .expect("confirm and apply should succeed");
        assert!(apply_msg.contains("Simulation mode"));
        let snap_applied = engine.snapshot();
        assert_eq!(snap_applied.state, UpdateState::Applied);

        running.store(false, Ordering::Relaxed);
        let _ = server_thread.join();
    }

    #[test]
    fn test_security_sanitization_and_ssrf_guards() {
        assert!(sanitize_asset_filename("ultradesk-rust_1.0.6-1_amd64_noble.deb").is_ok());
        assert!(sanitize_asset_filename("ultradesk-rust-v1.0.6-x86_64-linux.tar.gz").is_ok());
        assert!(sanitize_asset_filename("../etc/passwd").is_err());
        assert!(sanitize_asset_filename("/etc/shadow").is_err());
        assert!(sanitize_asset_filename("-oProxyCommand=id.deb").is_err());
        assert!(sanitize_asset_filename(".bashrc").is_err());
        assert!(sanitize_asset_filename("sub/file.deb").is_err());
        assert!(sanitize_asset_filename("").is_err());

        assert!(is_loopback_update_url("http://127.0.0.1:8080/api"));
        assert!(is_loopback_update_url("http://localhost:9000/api"));
        assert!(!is_loopback_update_url("http://localhost.evil.com/api"));
        assert!(!is_loopback_update_url("http://127.0.0.1.evil.com/api"));
        assert!(!is_loopback_update_url("https://127.0.0.1:8080/api"));

        assert!(validate_redirect_or_fetch_url(true, "http://127.0.0.1:4567/pkg.deb").is_ok());
        assert!(
            validate_redirect_or_fetch_url(true, "http://169.254.169.254/latest/meta-data")
                .is_err()
        );
        assert!(validate_redirect_or_fetch_url(true, "https://api.github.com/repos").is_err());

        assert!(validate_redirect_or_fetch_url(
            false,
            "https://api.github.com/repos/dtrh95/ultradesk/releases/latest"
        )
        .is_ok());
        assert!(validate_redirect_or_fetch_url(
            false,
            "https://objects.githubusercontent.com/github-production-release-asset/123"
        )
        .is_ok());
        assert!(validate_redirect_or_fetch_url(false, "http://api.github.com/repos").is_err());
        assert!(validate_redirect_or_fetch_url(false, "https://evil.com/malware.deb").is_err());
        assert!(validate_redirect_or_fetch_url(false, "http://127.0.0.1:8080/internal").is_err());
    }

    #[test]
    fn test_update_cache_dir_falls_back_when_home_unwritable() {
        let prev_xdg = env::var_os("XDG_CACHE_HOME");
        env::set_var("XDG_CACHE_HOME", "/homeless-shelter/.cache");
        let dir = get_private_update_cache_dir();
        match prev_xdg {
            Some(v) => env::set_var("XDG_CACHE_HOME", v),
            None => env::remove_var("XDG_CACHE_HOME"),
        }
        assert!(crate::profile::make_private_dir(&dir).is_ok());

        let nix_dir = resolve_private_update_cache_dir(None, Some("/homeless-shelter"));
        assert!(crate::profile::make_private_dir(&nix_dir).is_ok());
    }

    #[test]
    fn test_arch_pkg_rejects_unmanaged_tar_root_extraction() {
        let engine = UpdateEngine::with_target_and_url(
            InstallTarget::ArchPkg {
                arch: "x86_64".into(),
            },
            "http://127.0.0.1/releases".to_string(),
        );
        let plan = ReleasePlan {
            version: "1.0.0".to_string(),
            release_notes: String::new(),
            release_url: String::new(),
            asset: ReleaseAssetMeta {
                name: "ultradesk-rust-v1.0.0-x86_64-archlinux.tar.zst".to_string(),
                download_url: "http://127.0.0.1/archive.tar.zst".to_string(),
                expected_sha256: Some(
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
                ),
                size_bytes: 100,
            },
            target: InstallTarget::ArchPkg {
                arch: "x86_64".into(),
            },
        };
        let bad_pkg = Path::new("/tmp/ultradesk-rust-v1.0.0-x86_64-archlinux.tar.zst");
        let res = engine.execute_install_strategy(&plan, bad_pkg);
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .contains("Refusing unmanaged root extraction"));
    }

    #[test]
    fn test_confirm_and_apply_version_mismatch_and_missing_sha256() {
        let engine = UpdateEngine::with_target_and_url(
            InstallTarget::DebianDeb {
                codename: "bookworm".into(),
                arch: "amd64".into(),
            },
            "http://127.0.0.1/releases".to_string(),
        );

        let res = engine.confirm_and_apply_for_version(Some("9.9.9"));
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .contains("no update is currently available"));

        let plan_without_sha = ReleasePlan {
            version: "9.9.9".to_string(),
            release_notes: String::new(),
            release_url: "http://127.0.0.1/rel".to_string(),
            asset: ReleaseAssetMeta {
                name: "ultradesk.deb".to_string(),
                download_url: "http://127.0.0.1/deb".to_string(),
                expected_sha256: None,
                size_bytes: 10,
            },
            target: InstallTarget::DebianDeb {
                codename: "bookworm".into(),
                arch: "amd64".into(),
            },
        };
        *engine.cached_plan.lock().unwrap() = Some(plan_without_sha);
        engine.status.lock().unwrap().available_version = Some("9.9.9".to_string());

        let res_mismatch = engine.confirm_and_apply_for_version(Some("1.0.0"));
        assert!(res_mismatch.is_err());
        assert!(res_mismatch
            .unwrap_err()
            .contains("does not match available version"));

        let res_no_sha = engine.confirm_and_apply_for_version(Some("9.9.9"));
        assert!(res_no_sha.is_err());
        assert!(res_no_sha
            .unwrap_err()
            .contains("no expected SHA-256 checksum found"));
    }

    #[test]
    fn test_fallback_parent_symlink_rejection() {
        let base_tmp =
            std::env::temp_dir().join(format!("updater_symlink_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base_tmp);
        fs::create_dir_all(&base_tmp).unwrap();

        let real_target = base_tmp.join("real");
        fs::create_dir_all(&real_target).unwrap();

        #[cfg(unix)]
        {
            let symlink_parent = base_tmp.join("symlink_parent");
            std::os::unix::fs::symlink(&real_target, &symlink_parent).unwrap();
            let updates_child = symlink_parent.join("updates");
            let res = crate::profile::make_private_dir(&updates_child);
            assert!(res.is_err());
        }

        let _ = fs::remove_dir_all(&base_tmp);
    }
}

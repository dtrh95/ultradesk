use std::fs;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const CLIENT_FILE: &str = "profile";
pub const HOST_FILE: &str = "host-profile";
pub const MASTER_PASSWORD_FILE: &str = "master-password";
pub const MAX_FAILURES_BEFORE_LOCKOUT: u32 = 5;
pub const LOCKOUT_DURATION: Duration = Duration::from_secs(5 * 60);
pub const MIN_MASTER_PASSWORD: usize = 8;
pub const MAX_MASTER_PASSWORD: usize = 64;
pub const DEFAULT_DISPLAY_NAME: &str = "UltraDesk";
pub const MAX_PROFILE_BYTES: u64 = 65_536;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostProfile {
    pub device_id: String,
    pub temp_password: String,
    pub display_name: String,
    pub hardware_mac: String,
    pub local_ip: Ipv4Addr,
    pub master_password_proof: Option<String>,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientProfile {
    pub device_id: String,
    pub temp_password: String,
    pub display_name: String,
    pub synthetic_mac: String,
    pub local_ip: Ipv4Addr,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ProfileManager {
    config_dir: PathBuf,
}

impl ProfileManager {
    pub fn from_xdg() -> Result<Self, String> {
        let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            Some(x) => PathBuf::from(x),
            None => {
                let home = std::env::var_os("HOME")
                    .ok_or_else(|| "HOME environment variable is not set".to_string())?;
                PathBuf::from(home).join(".config")
            }
        };
        let new_dir = base.join("ultradesk-rust");
        let legacy_dir = base.join("ultraviewer-linux");
        if !new_dir.exists() && legacy_dir.exists() {
            let _ = make_private_dir(&new_dir);
            if let Ok(entries) = fs::read_dir(&legacy_dir) {
                for entry in entries.flatten() {
                    let src = entry.path();
                    if src.is_file() {
                        let dst = new_dir.join(entry.file_name());
                        if !dst.exists() {
                            let _ = fs::copy(&src, &dst);
                        }
                    }
                }
            }
        }
        Ok(Self {
            config_dir: new_dir,
        })
    }

    pub fn from_custom_dir(dir: PathBuf) -> Self {
        Self { config_dir: dir }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn load_or_create_host(&self) -> Result<HostProfile, String> {
        make_private_dir(&self.config_dir)?;
        let path = self.config_dir.join(HOST_FILE);
        let stored = load_or_generate_stored(&path)?;
        let local_ip = discover_primary_ipv4().unwrap_or(Ipv4Addr::new(127, 0, 0, 1));
        let hardware_mac = discover_hardware_mac(Some(local_ip))
            .unwrap_or_else(|| synthesize_client_mac(&stored.device_id));
        let master_password_proof = self.master_password_proof()?;
        write_private(&path, &stored.render())?;
        Ok(HostProfile {
            device_id: stored.device_id,
            temp_password: stored.temp_password,
            display_name: stored.display_name,
            hardware_mac,
            local_ip,
            master_password_proof,
            path,
        })
    }

    pub fn load_or_create_client(&self) -> Result<ClientProfile, String> {
        make_private_dir(&self.config_dir)?;
        let path = self.config_dir.join(CLIENT_FILE);
        let stored = load_or_generate_stored(&path)?;
        let local_ip = discover_primary_ipv4().unwrap_or(Ipv4Addr::new(127, 0, 0, 1));
        let synthetic_mac = synthesize_client_mac(&stored.device_id);
        write_private(&path, &stored.render())?;
        Ok(ClientProfile {
            device_id: stored.device_id,
            temp_password: stored.temp_password,
            display_name: stored.display_name,
            synthetic_mac,
            local_ip,
            path,
        })
    }

    pub fn set_master_password(&self, password: Option<&str>) -> Result<(), String> {
        make_private_dir(&self.config_dir)?;
        let file = self.config_dir.join(MASTER_PASSWORD_FILE);
        let Some(pw) = password else {
            if file.exists() {
                let _ = fs::remove_file(file);
            }
            return Ok(());
        };
        validate_master_password(pw)?;
        let proof = compute_master_password_proof(pw);
        write_private(&file, &format!("{}\n", proof))
    }

    pub fn master_password_proof(&self) -> Result<Option<String>, String> {
        let file = self.config_dir.join(MASTER_PASSWORD_FILE);
        if !file.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&file).map_err(|e| e.to_string())?;
        let proof = text.trim();
        if proof.len() == 32 && proof.chars().all(|c| c.is_ascii_hexdigit()) {
            Ok(Some(proof.to_ascii_lowercase()))
        } else {
            Err(format!(
                "Master password file {} is invalid",
                file.display()
            ))
        }
    }
}

pub fn validate_master_password(pw: &str) -> Result<(), String> {
    if !(MIN_MASTER_PASSWORD..=MAX_MASTER_PASSWORD).contains(&pw.len())
        || !pw.bytes().all(|b| (32..=126).contains(&b))
    {
        return Err(format!(
            "The master password must be {MIN_MASTER_PASSWORD}-{MAX_MASTER_PASSWORD} printable ASCII characters."
        ));
    }
    let bytes = pw.as_bytes();
    let all_same = bytes.iter().all(|&b| b == bytes[0]);
    let seq_asc = bytes.windows(2).all(|w| w[1] == w[0].wrapping_add(1));
    let seq_desc = bytes.windows(2).all(|w| w[0] == w[1].wrapping_add(1));
    const WEAK_LIST: [&str; 11] = [
        "1234567890",
        "12341234",
        "123123123",
        "password",
        "password1",
        "password123",
        "qwertyui",
        "qwerty123",
        "ultraviewer",
        "ultradesk",
        "admin123",
    ];
    let blacklisted = WEAK_LIST.iter().any(|w| pw.eq_ignore_ascii_case(w));
    if all_same || seq_asc || seq_desc || blacklisted {
        return Err(
            "The master password is too easy to guess and will be rejected with Status 99. Mix letters, numbers, or symbols.".to_string(),
        );
    }
    Ok(())
}

pub fn compute_master_password_proof(password: &str) -> String {
    crate::protocol::md5_hex_lower(format!("{}{}", " ".repeat(32), password).as_bytes())
}

pub fn verify_master_password_proof(candidate_hash: &str, stored_proof: &str) -> bool {
    if stored_proof.len() != 32 || !stored_proof.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    candidate_hash.eq_ignore_ascii_case(stored_proof)
        || compute_master_password_proof(candidate_hash).eq_ignore_ascii_case(stored_proof)
}

pub fn operating_system_wire_value(kernel_release: &str) -> String {
    let nums: Vec<u32> = kernel_release
        .split(|c: char| !c.is_ascii_digit())
        .take_while(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .take(3)
        .collect();
    let get = |i: usize| nums.get(i).copied().unwrap_or(0);
    format!("{}|{}|0|{}|0|0", get(2), get(0), get(1))
}

pub fn synthesize_client_mac(device_id: &str) -> String {
    let clean: String = device_id
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    let mut m = format!("{:0<12}", &clean[..clean.len().min(12)]).to_ascii_uppercase();
    if m.len() < 12 {
        m = format!("{:0<12}", m);
    }
    let b0 = u8::from_str_radix(&m[..2], 16).unwrap_or(0);
    let first = (b0 & 0xFC) | 0x02;
    m.replace_range(..2, &format!("{:02X}", first));
    m
}

pub fn normalize_display_name(name: &str) -> String {
    let reserved = "!#$&?:/\\*,.@<>;'\"=";
    let folded: String = name
        .chars()
        .map(|c| match c {
            'đ' => 'd',
            'Đ' => 'D',
            c => c,
        })
        .collect();
    let ascii: String = folded
        .chars()
        .filter(|c| (' '..='~').contains(c) && !reserved.contains(*c))
        .collect();
    let trimmed = ascii.trim();
    if trimmed.is_empty() {
        DEFAULT_DISPLAY_NAME.to_string()
    } else {
        trimmed
            .chars()
            .take(20)
            .collect::<String>()
            .trim()
            .to_string()
    }
}

pub fn discover_hardware_mac(_local_ip: Option<Ipv4Addr>) -> Option<String> {
    if let Ok(entries) = fs::read_dir("/sys/class/net") {
        let route = fs::read_to_string("/proc/net/route").unwrap_or_default();
        let default_if = route.lines().skip(1).find_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() > 2 && f[1] == "00000000").then(|| f[0].to_string())
        });

        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n != "lo")
            .collect();
        names.sort();

        if let Some(d) = default_if {
            names.retain(|n| *n != d);
            names.insert(0, d);
        }

        if let Some(mac) = names.iter().find_map(|n| {
            let path = format!("/sys/class/net/{n}/address");
            normalize_mac(&fs::read_to_string(path).ok()?)
        }) {
            return Some(mac);
        }
    }

    for prog in ["ifconfig", "/sbin/ifconfig"] {
        for args in [&["en0"][..], &["en1"][..], &[][..]] {
            if let Ok(out) = std::process::Command::new(prog).args(args).output() {
                if out.status.success() {
                    if let Some(mac) = parse_ifconfig_mac(&String::from_utf8_lossy(&out.stdout)) {
                        return Some(mac);
                    }
                }
            }
        }
    }
    None
}

pub fn parse_ifconfig_mac(ifconfig_text: &str) -> Option<String> {
    for line in ifconfig_text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("ether ") {
            if let Some(candidate) = rest.split_whitespace().next() {
                if let Some(mac) = normalize_mac(candidate) {
                    return Some(mac);
                }
            }
        }
    }
    None
}

fn normalize_mac(s: &str) -> Option<String> {
    let hex: String = s
        .trim()
        .chars()
        .filter(|c| *c != ':' && *c != '-')
        .collect::<String>()
        .to_ascii_uppercase();
    let ok = hex.len() == 12
        && hex.chars().all(|c| c.is_ascii_hexdigit())
        && hex != "000000000000"
        && hex != "020000000000";
    if !ok {
        return None;
    }
    let first_byte = u8::from_str_radix(&hex[..2], 16).ok()?;
    if first_byte & 1 != 0 {
        return None;
    }
    Some(hex)
}

pub fn discover_primary_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:53").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            let usable = o[0] != 0 && o[0] != 127 && o[0] < 224 && !(o[0] == 169 && o[1] == 254);
            if usable {
                Some(v4)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[derive(Debug, Default)]
pub struct LoginThrottle {
    failures: u32,
    locked_until: Option<Instant>,
}

impl LoginThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_locked(&self, now: Instant) -> bool {
        self.locked_until.is_some_and(|until| now < until)
    }

    pub fn record_failure(&mut self, now: Instant) -> bool {
        self.failures = self.failures.saturating_add(1);
        if self.failures >= MAX_FAILURES_BEFORE_LOCKOUT {
            self.locked_until = Some(now + LOCKOUT_DURATION);
            true
        } else {
            false
        }
    }

    pub fn record_success(&mut self) {
        self.failures = 0;
        self.locked_until = None;
    }

    pub fn remaining_lockout(&self, now: Instant) -> Option<Duration> {
        self.locked_until
            .and_then(|until| until.checked_duration_since(now))
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredProfile {
    device_id: String,
    temp_password: String,
    display_name: String,
}

impl StoredProfile {
    fn parse(text: &str) -> Option<Self> {
        let mut device_id = String::new();
        let mut temp_password = String::new();
        let mut display_name = String::new();

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line.split_once('=')?;
            match k.trim() {
                "device_id" | "device_identity" => device_id = v.trim().to_ascii_uppercase(),
                "temp_password" | "password" => temp_password = v.trim().to_string(),
                "display_name" => display_name = v.trim().to_string(),
                _ => {}
            }
        }

        let id_ok = device_id.len() == 32 && device_id.chars().all(|c| c.is_ascii_hexdigit());
        let pw_ok = temp_password.len() == 8 && temp_password.chars().all(|c| c.is_ascii_digit());
        if id_ok && pw_ok {
            Some(StoredProfile {
                device_id,
                temp_password,
                display_name,
            })
        } else {
            None
        }
    }

    fn render(&self) -> String {
        format!(
            "# UltraDesk registration profile. Deleting it gives this computer a new ID.\n\
             device_id={}\n\
             password={}\n\
             display_name={}\n",
            self.device_id, self.temp_password, self.display_name
        )
    }
}

pub fn default_computer_name() -> String {
    let env_name = std::env::var("ULTRADESK_NAME")
        .or_else(|_| std::env::var("ULTRAVIEWER_NAME"))
        .or_else(|_| std::env::var("LINUXVIEWER_NAME"))
        .unwrap_or_default();
    normalize_display_name(&env_name)
}

fn load_or_generate_stored(path: &Path) -> Result<StoredProfile, String> {
    if path.exists() {
        let meta = fs::metadata(path).map_err(|e| e.to_string())?;
        if meta.is_file() && meta.len() > 0 && meta.len() <= MAX_PROFILE_BYTES {
            if let Ok(content) = fs::read_to_string(path) {
                if let Some(parsed) = StoredProfile::parse(&content) {
                    return Ok(parsed);
                }
            }
        }
    }

    let device_id = generate_device_id();
    let temp_password = random_digit_string(8);
    let display_name = default_computer_name();

    Ok(StoredProfile {
        device_id,
        temp_password,
        display_name,
    })
}

pub fn make_private_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let meta = fs::symlink_metadata(dir).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(format!(
                "Refusing non-directory or symlink config path {}",
                dir.display()
            ));
        }
        let euid = unsafe { libc::geteuid() };
        if meta.uid() != euid {
            return Err(format!(
                "Refusing config directory {} owned by uid {} (expected {})",
                dir.display(),
                meta.uid(),
                euid
            ));
        }
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn write_private_bytes(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        make_private_dir(parent)?;
    }
    let nonce = crate::protocol::md5_hex_lower(&random_bytes(8));
    let tmp = path.with_extension(format!("tmp.{}.{}", std::process::id(), &nonce[..8]));
    let _ = fs::remove_file(&tmp);
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        f.write_all(content).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&tmp, content).map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })
}

fn write_private(path: &Path, content: &str) -> Result<(), String> {
    write_private_bytes(path, content.as_bytes())
}

pub fn fallback_private_config_dir() -> PathBuf {
    #[cfg(unix)]
    let uid = unsafe { libc::geteuid() };
    #[cfg(not(unix))]
    let uid = std::process::id();
    std::env::temp_dir().join(format!("ultradesk-rust-{}", uid))
}

pub fn generate_ephemeral_device_id() -> String {
    generate_device_id()
}

pub fn generate_ephemeral_temp_password() -> String {
    random_digit_string(8)
}

fn random_bytes(n: usize) -> Vec<u8> {
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        use std::io::Read;
        let mut b = vec![0u8; n];
        if f.read_exact(&mut b).is_ok() {
            return b;
        }
    }

    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let seed = format!("{now}:{pid}:{count}");
    let hash = crate::protocol::md5_hex_lower(seed.as_bytes());
    let mut out = hash.into_bytes();
    out.resize(n, 0x42);
    out
}

fn random_digit_string(len: usize) -> String {
    let mut s = String::with_capacity(len);
    while s.len() < len {
        let b = random_bytes(len * 2);
        for byte in b {
            if byte < 250 && s.len() < len {
                s.push((b'0' + (byte % 10)) as char);
            }
        }
    }
    s
}

fn generate_device_id() -> String {
    crate::protocol::md5_hex_lower(&random_bytes(16)).to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_master_password_proof_and_verification() {
        let proof = compute_master_password_proof("Secret123");
        assert_eq!(proof.len(), 32);
        assert!(verify_master_password_proof(&proof, &proof));
        assert!(verify_master_password_proof("Secret123", &proof));
        assert!(!verify_master_password_proof("WrongPass", &proof));
    }

    #[test]
    fn test_operating_system_wire_value() {
        assert_eq!(
            operating_system_wire_value("6.8.0-45-generic"),
            "0|6|0|8|0|0"
        );
        assert_eq!(operating_system_wire_value("24.1.0"), "0|24|0|1|0|0");
        assert_eq!(
            operating_system_wire_value("5.15.153.1-microsoft"),
            "153|5|0|15|0|0"
        );
    }

    #[test]
    fn test_synthetic_mac_generation() {
        let dev_id = "0123456789ABCDEF0123456789ABCDEF";
        let syn_mac = synthesize_client_mac(dev_id);
        assert_eq!(syn_mac.len(), 12);
        assert_eq!(syn_mac, "0223456789AB");
        let first_byte = u8::from_str_radix(&syn_mac[..2], 16).unwrap();
        assert_eq!(first_byte & 0x02, 0x02);
        assert_eq!(first_byte & 0x01, 0x00);
    }

    #[test]
    fn test_parse_ifconfig_mac_macos_format() {
        let sample = "en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\
                      \toptions=6463<RXCSUM,TXCSUM,TSO4,TSO6,CHANNEL_IO,PARTIAL_CSUM,ZEROINVERT_CSUM>\n\
                      \tether a4:83:e7:1b:2c:3d\n\
                      \tinet 192.168.1.42 netmask 0xffffff00 broadcast 192.168.1.255\n";
        assert_eq!(parse_ifconfig_mac(sample), Some("A483E71B2C3D".to_string()));
    }

    #[test]
    fn test_login_throttle_lockout() {
        let mut throttle = LoginThrottle::new();
        let start = Instant::now();

        assert!(!throttle.is_locked(start));
        for _ in 0..4 {
            assert!(!throttle.record_failure(start));
            assert!(!throttle.is_locked(start));
        }

        assert!(throttle.record_failure(start));
        assert!(throttle.is_locked(start));
        assert_eq!(throttle.failures(), 5);

        let after_4_mins = start + Duration::from_secs(240);
        assert!(throttle.is_locked(after_4_mins));

        let after_5_mins = start + Duration::from_secs(301);
        assert!(!throttle.is_locked(after_5_mins));

        throttle.record_success();
        assert_eq!(throttle.failures(), 0);
        assert!(!throttle.is_locked(start));
    }

    #[test]
    fn test_profile_manager_custom_dir() {
        let tmp = std::env::temp_dir().join(format!("uv_profile_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);

        let mgr = ProfileManager::from_custom_dir(tmp.clone());
        let host = mgr.load_or_create_host().unwrap();
        assert_eq!(host.device_id.len(), 32);
        assert_eq!(host.temp_password.len(), 8);
        assert_eq!(host.display_name, "UltraDesk");
        assert!(host.path.exists());

        let client = mgr.load_or_create_client().unwrap();
        assert_eq!(client.device_id.len(), 32);
        assert_eq!(client.synthetic_mac.len(), 12);
        assert_eq!(client.display_name, "UltraDesk");
        assert!(client.path.exists());

        mgr.set_master_password(Some("Pass123456")).unwrap();
        let proof = mgr.master_password_proof().unwrap();
        assert!(proof.is_some());
        assert_eq!(proof.as_ref().unwrap().len(), 32);

        let reloaded_host = mgr.load_or_create_host().unwrap();
        assert_eq!(reloaded_host.device_id, host.device_id);
        assert_eq!(reloaded_host.temp_password, host.temp_password);
        assert_eq!(reloaded_host.master_password_proof, proof);

        let _ = fs::remove_dir_all(&tmp);
    }
}

use std::collections::HashMap;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const MAX_DIRECT_CANDIDATES: usize = 4;
pub const MAX_DIRECT_FRAME_SIZE: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectCandidate {
    pub public_addr: SocketAddrV4,
    pub lan_addr: SocketAddrV4,
    pub direct_key: String,
}

impl DirectCandidate {
    pub fn parse(field16: &str) -> Option<Self> {
        let parts: Vec<&str> = field16.split('|').collect();
        if parts.len() != 5 {
            return None;
        }
        let pub_ip: Ipv4Addr = parts[0].parse().ok()?;
        let pub_port: u16 = parts[1].parse().ok()?;
        let lan_ip: Ipv4Addr = parts[2].parse().ok()?;
        let lan_port: u16 = parts[3].parse().ok()?;
        let direct_key = parts[4].to_string();
        if direct_key.is_empty() {
            return None;
        }
        Some(DirectCandidate {
            public_addr: SocketAddrV4::new(pub_ip, pub_port),
            lan_addr: SocketAddrV4::new(lan_ip, lan_port),
            direct_key,
        })
    }

    pub fn format(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.public_addr.ip(),
            self.public_addr.port(),
            self.lan_addr.ip(),
            self.lan_addr.port(),
            self.direct_key
        )
    }
}

pub fn is_lan_address(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 10
        || (o[0] == 172 && (16..=31).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168)
        || (o[0] == 169 && o[1] == 254)
}

pub struct DirectCodec {
    key: String,
}

impl DirectCodec {
    pub fn new(key: &str) -> Result<Self, String> {
        if key.is_empty() {
            return Err("LAN direct key cannot be empty".to_string());
        }
        crate::protocol::derive_p2p_aes_key(key)?;
        Ok(Self {
            key: key.to_string(),
        })
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn encode_frame(&self, format_flag: bool, payload: &[u8]) -> Result<Vec<u8>, String> {
        let iv = random_16_bytes();
        self.encode_frame_with_iv(format_flag, payload, &iv)
    }

    pub fn encode_frame_with_iv(
        &self,
        format_flag: bool,
        payload: &[u8],
        iv: &[u8; 16],
    ) -> Result<Vec<u8>, String> {
        crate::protocol::encode_p2p_frame(payload, format_flag, Some(&self.key), Some(iv))
    }

    pub fn decode_frame(&self, encrypted_body: &[u8]) -> Result<(bool, Vec<u8>), String> {
        let (payload, flag) =
            crate::protocol::decode_p2p_frame_body(encrypted_body, Some(&self.key))?;
        Ok((flag, payload))
    }
}

#[derive(Default, Debug)]
pub struct LanUpgradeState {
    pub expected_peer_ip: Option<Ipv4Addr>,
    pub candidates: HashMap<u64, Ipv4Addr>,
    pub login_candidate_id: Option<u64>,
    pub login_sent_id: Option<u64>,
    pub active_selected_id: Option<u64>,
}

impl LanUpgradeState {
    pub fn on_candidate_connected(&mut self, id: u64, ip: Ipv4Addr) -> bool {
        if self.active_selected_id.is_some()
            || self.candidates.len() >= MAX_DIRECT_CANDIDATES
            || self.expected_peer_ip.is_some_and(|expected| expected != ip)
        {
            return false;
        }
        self.candidates.insert(id, ip);
        true
    }

    pub fn expect_peer_ip(&mut self, ip: Ipv4Addr) -> Vec<u64> {
        self.expected_peer_ip = Some(ip);
        let rejected: Vec<u64> = self
            .candidates
            .iter()
            .filter(|(_, a)| **a != ip)
            .map(|(id, _)| *id)
            .collect();
        for id in &rejected {
            self.remove_candidate(*id);
        }
        rejected
    }

    pub fn begin_login(&mut self) -> Option<u64> {
        if self.active_selected_id.is_some() || self.login_candidate_id.is_some() {
            return None;
        }
        let expected = self.expected_peer_ip?;
        let mut ids: Vec<u64> = self
            .candidates
            .iter()
            .filter(|(_, a)| **a == expected)
            .map(|(id, _)| *id)
            .collect();
        ids.sort();
        let id = *ids.first()?;
        self.login_candidate_id = Some(id);
        Some(id)
    }

    pub fn mark_login_sent(&mut self, id: u64) -> bool {
        if self.login_candidate_id != Some(id)
            || self.expected_peer_ip.is_none()
            || self.candidates.get(&id) != self.expected_peer_ip.as_ref()
        {
            return false;
        }
        self.login_sent_id = Some(id);
        true
    }

    pub fn permits_initialization(&self, id: u64) -> bool {
        self.login_sent_id == Some(id)
            && self.expected_peer_ip.is_some()
            && self.candidates.get(&id) == self.expected_peer_ip.as_ref()
    }

    pub fn select_active(&mut self, id: u64) -> bool {
        if !self.permits_initialization(id) {
            return false;
        }
        self.active_selected_id = Some(id);
        let ip = self.candidates[&id];
        self.candidates.clear();
        self.candidates.insert(id, ip);
        true
    }

    pub fn remove_candidate(&mut self, id: u64) -> bool {
        self.candidates.remove(&id);
        if self.login_candidate_id == Some(id) {
            self.login_candidate_id = None;
        }
        if self.login_sent_id == Some(id) {
            self.login_sent_id = None;
        }
        let was_active = self.active_selected_id == Some(id);
        if was_active {
            self.active_selected_id = None;
        }
        was_active
    }
}

#[derive(Debug)]
pub enum DirectEvent {
    Connected {
        id: u64,
        peer_ip: Ipv4Addr,
        writer: DirectWriter,
    },
    Message {
        id: u64,
        format_flag: bool,
        payload: Vec<u8>,
    },
    Disconnected {
        id: u64,
    },
}

#[derive(Clone)]
pub struct DirectWriter {
    pub id: u64,
    stream: Arc<Mutex<TcpStream>>,
    codec: Arc<DirectCodec>,
}

impl std::fmt::Debug for DirectWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DirectWriter({})", self.id)
    }
}

impl DirectWriter {
    pub fn send(&self, format_flag: bool, payload: &[u8]) -> Result<(), String> {
        let frame = self.codec.encode_frame(format_flag, payload)?;
        let mut s = self.stream.lock().map_err(|e| e.to_string())?;
        s.write_all(&frame).map_err(|e| e.to_string())
    }

    pub fn shutdown(&self) {
        if let Ok(s) = self.stream.lock() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

pub struct DirectListener {
    pub local_ip: Ipv4Addr,
    pub port: u16,
    is_accepting: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    clients: Arc<Mutex<HashMap<u64, TcpStream>>>,
}

static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

impl DirectListener {
    pub fn start(
        local_ip: Ipv4Addr,
        key: &str,
        allow_loopback: bool,
        events: Sender<DirectEvent>,
    ) -> Result<Self, String> {
        if !(is_lan_address(local_ip) || (allow_loopback && local_ip.octets()[0] == 127)) {
            return Err("DirectListener requires a private LAN or loopback IP address".to_string());
        }

        let codec = Arc::new(DirectCodec::new(key)?);
        let listener = TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(local_ip, 0)))
            .map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();

        let is_accepting = Arc::new(AtomicBool::new(true));
        let closed = Arc::new(AtomicBool::new(false));
        let clients: Arc<Mutex<HashMap<u64, TcpStream>>> = Arc::default();

        let acc = is_accepting.clone();
        let cls = closed.clone();
        let cls_map = clients.clone();

        thread::spawn(move || {
            while acc.load(Ordering::SeqCst) && !cls.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, peer)) => {
                        let SocketAddr::V4(v4) = peer else {
                            continue;
                        };
                        let peer_ip = *v4.ip();
                        let ok_source = is_lan_address(peer_ip)
                            || (allow_loopback && peer_ip.octets()[0] == 127);

                        if !ok_source || cls_map.lock().unwrap().len() >= MAX_DIRECT_CANDIDATES {
                            let _ = stream.shutdown(Shutdown::Both);
                            continue;
                        }

                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_nodelay(true);
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(15)));

                        let (Ok(read_s), Ok(write_s), Ok(keep_s)) =
                            (stream.try_clone(), stream.try_clone(), stream.try_clone())
                        else {
                            continue;
                        };

                        let id = NEXT_CLIENT_ID.fetch_add(1, Ordering::SeqCst);
                        cls_map.lock().unwrap().insert(id, keep_s);
                        let writer = DirectWriter {
                            id,
                            stream: Arc::new(Mutex::new(write_s)),
                            codec: codec.clone(),
                        };

                        if events
                            .send(DirectEvent::Connected {
                                id,
                                peer_ip,
                                writer,
                            })
                            .is_err()
                        {
                            return;
                        }

                        spawn_reader(id, read_s, codec.clone(), events.clone(), cls_map.clone());
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(2));
                    }
                }
            }
        });

        Ok(DirectListener {
            local_ip,
            port,
            is_accepting,
            closed,
            clients,
        })
    }

    pub fn stop_accepting(&self) {
        self.is_accepting.store(false, Ordering::SeqCst);
    }

    pub fn select(&self, id: u64) {
        self.stop_accepting();
        let mut c = self.clients.lock().unwrap();
        for (other_id, s) in c.iter() {
            if *other_id != id {
                let _ = s.shutdown(Shutdown::Both);
            }
        }
        c.retain(|k, _| *k == id);
    }

    pub fn close_candidate(&self, id: u64) {
        if let Some(s) = self.clients.lock().unwrap().remove(&id) {
            let _ = s.shutdown(Shutdown::Both);
        }
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.stop_accepting();
        for (_, s) in self.clients.lock().unwrap().drain() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for DirectListener {
    fn drop(&mut self) {
        self.close();
    }
}

fn spawn_reader(
    id: u64,
    mut stream: TcpStream,
    codec: Arc<DirectCodec>,
    events: Sender<DirectEvent>,
    clients: Arc<Mutex<HashMap<u64, TcpStream>>>,
) {
    thread::spawn(move || {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];

        'outer: loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }

            while buf.len() >= 4 {
                let frame_len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                if frame_len > MAX_DIRECT_FRAME_SIZE {
                    break 'outer;
                }
                if buf.len() < 4 + frame_len {
                    break;
                }

                let decoded = codec.decode_frame(&buf[4..4 + frame_len]);
                buf.drain(..4 + frame_len);

                let Ok((format_flag, payload)) = decoded else {
                    break 'outer;
                };

                if events
                    .send(DirectEvent::Message {
                        id,
                        format_flag,
                        payload,
                    })
                    .is_err()
                {
                    break 'outer;
                }
            }
        }

        if let Some(s) = clients.lock().unwrap().remove(&id) {
            let _ = s.shutdown(Shutdown::Both);
        }
        let _ = events.send(DirectEvent::Disconnected { id });
    });
}

fn random_16_bytes() -> [u8; 16] {
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        use std::io::Read;
        let mut b = [0u8; 16];
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
    let seed = format!("{now}:{count}");
    let hex = crate::protocol::md5_hex_lower(seed.as_bytes());
    let mut out = [0u8; 16];
    for (i, chunk) in hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .take(16)
        .enumerate()
    {
        if let Ok(s) = std::str::from_utf8(chunk) {
            out[i] = u8::from_str_radix(s, 16).unwrap_or(0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn test_iv() -> [u8; 16] {
        core::array::from_fn(|i| i as u8)
    }

    #[test]
    fn test_candidate_parse_and_format() {
        let field16 = "203.0.113.10|55000|192.168.1.50|49152|direct-secret-key";
        let candidate = DirectCandidate::parse(field16).unwrap();
        assert_eq!(candidate.public_addr.ip(), &Ipv4Addr::new(203, 0, 113, 10));
        assert_eq!(candidate.public_addr.port(), 55000);
        assert_eq!(candidate.lan_addr.ip(), &Ipv4Addr::new(192, 168, 1, 50));
        assert_eq!(candidate.lan_addr.port(), 49152);
        assert_eq!(candidate.direct_key, "direct-secret-key");
        assert_eq!(candidate.format(), field16);
    }

    #[test]
    fn test_direct_codec_fixtures() {
        let codec = DirectCodec::new("password").unwrap();
        let payload = &[0x48, 0, 0x69, 0];
        let want =
            unhex("20000000000102030405060708090a0b0c0d0e0f8ee36820538aebf76ccad1f1fe63df42");
        let encoded = codec
            .encode_frame_with_iv(true, payload, &test_iv())
            .unwrap();
        assert_eq!(encoded, want);

        let (flag, decoded) = codec.decode_frame(&want[4..]).unwrap();
        assert!(flag);
        assert_eq!(decoded, payload);
    }

    #[test]
    fn test_lan_upgrade_state_machine() {
        let mut state = LanUpgradeState::default();
        let exp = Ipv4Addr::new(192, 168, 1, 100);
        let other = Ipv4Addr::new(192, 168, 1, 101);

        assert!(state.on_candidate_connected(1, exp));
        assert!(state.on_candidate_connected(2, other));
        assert_eq!(state.begin_login(), None);

        let rejected = state.expect_peer_ip(exp);
        assert_eq!(rejected, vec![2]);

        assert_eq!(state.begin_login(), Some(1));
        assert!(!state.permits_initialization(1));

        assert!(state.mark_login_sent(1));
        assert!(state.permits_initialization(1));

        assert!(state.select_active(1));
        assert_eq!(state.active_selected_id, Some(1));
        assert_eq!(state.candidates.len(), 1);
    }

    #[test]
    fn test_loopback_direct_listener_exchange() {
        let (tx, rx) = std::sync::mpsc::channel();
        let listener =
            DirectListener::start(Ipv4Addr::new(127, 0, 0, 1), "test-direct-key", true, tx)
                .unwrap();

        let mut client = TcpStream::connect(("127.0.0.1", listener.port)).unwrap();
        let DirectEvent::Connected {
            id,
            peer_ip,
            writer,
        } = rx.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("Expected Connected event");
        };
        assert_eq!(peer_ip, Ipv4Addr::new(127, 0, 0, 1));

        let client_codec = DirectCodec::new("test-direct-key").unwrap();
        let client_frame = client_codec.encode_frame(true, b"hello-direct").unwrap();
        client.write_all(&client_frame).unwrap();

        let DirectEvent::Message {
            id: mid,
            format_flag,
            payload,
        } = rx.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("Expected Message event");
        };
        assert_eq!(mid, id);
        assert!(format_flag);
        assert_eq!(payload, b"hello-direct");

        writer.send(false, b"reply-from-listener").unwrap();
        let mut len_buf = [0u8; 4];
        client.read_exact(&mut len_buf).unwrap();
        let body_len = u32::from_le_bytes(len_buf) as usize;
        let mut body_buf = vec![0u8; body_len];
        client.read_exact(&mut body_buf).unwrap();

        let (reply_flag, reply_payload) = client_codec.decode_frame(&body_buf).unwrap();
        assert!(!reply_flag);
        assert_eq!(reply_payload, b"reply-from-listener");

        listener.close();
        let disc = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(disc, DirectEvent::Disconnected { id: did } if did == id));
    }
}

use aes::cipher::{BlockCipherEncrypt, KeyInit};
use aes::Aes256;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const E2E_HANDSHAKE_PREFIX: &str = "E2E1|";
pub const E2E_FRAME_MAGIC: &[u8; 4] = b"UDE2";
pub const E2E_TAG_LEN: usize = 16;
pub const E2E_HEADER_LEN: usize = 5;
pub const E2E_OVERHEAD: usize = E2E_HEADER_LEN + E2E_TAG_LEN;
pub const E2E_CIPHER_NAME: &str = "X25519-AES256CTR-HMACSHA256";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum E2eRole {
    Host,
    Viewer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum E2eOfferPayload {
    ViewerPending {
        secret_scalar: [u8; 32],
        public_key: [u8; 32],
        nonce: String,
        password_proof: String,
        custom_password_proof: String,
    },
    HostPending {
        answer_token: Vec<u8>,
        cipher: E2eSessionCipher,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum E2eNegotiationState {
    #[default]
    Disabled,
    Offered {
        offer: E2eOfferPayload,
    },
    Established(E2eSessionCipher),
}

impl E2eNegotiationState {
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Established(_))
    }

    pub fn cipher_name(&self) -> &str {
        match self {
            Self::Established(_) => E2E_CIPHER_NAME,
            _ => "",
        }
    }

    pub fn fingerprint(&self) -> &str {
        match self {
            Self::Established(c) => &c.fingerprint,
            Self::Offered {
                offer: E2eOfferPayload::HostPending { cipher, .. },
            } => &cipher.fingerprint,
            _ => "",
        }
    }

    pub fn cipher_mut(&mut self) -> Option<&mut E2eSessionCipher> {
        match self {
            Self::Established(c) => Some(c),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct E2eSessionCipher {
    pub tx_key: [u8; 32],
    pub rx_key: [u8; 32],
    tx_aes: Aes256,
    rx_aes: Aes256,
    tx_ipad: [u8; 64],
    tx_opad: [u8; 64],
    rx_ipad: [u8; 64],
    rx_opad: [u8; 64],
    pub tx_seqnum: u64,
    pub rx_seqnum: u64,
    pub fingerprint: String,
}

impl fmt::Debug for E2eSessionCipher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("E2eSessionCipher")
            .field("tx_seqnum", &self.tx_seqnum)
            .field("rx_seqnum", &self.rx_seqnum)
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

impl PartialEq for E2eSessionCipher {
    fn eq(&self, other: &Self) -> bool {
        ct_bytes_eq(&self.tx_key, &other.tx_key)
            && ct_bytes_eq(&self.rx_key, &other.rx_key)
            && self.tx_seqnum == other.tx_seqnum
            && self.rx_seqnum == other.rx_seqnum
            && self.fingerprint == other.fingerprint
    }
}

impl Eq for E2eSessionCipher {}

impl E2eSessionCipher {
    pub fn new(base: &[u8; 32], fingerprint: String, role: E2eRole) -> Self {
        let (h2v, v2h) = derive_dual_keys(base);
        let (tx_key, rx_key) = match role {
            E2eRole::Host => (h2v, v2h),
            E2eRole::Viewer => (v2h, h2v),
        };
        Self::from_directional_keys(tx_key, rx_key, fingerprint)
    }

    pub fn from_directional_keys(tx_key: [u8; 32], rx_key: [u8; 32], fingerprint: String) -> Self {
        let tx_enc_key = hmac_sha256(&tx_key, b"ude2-aes256-ctr-key");
        let tx_mac_key = hmac_sha256(&tx_key, b"ude2-hmac-sha256-key");
        let rx_enc_key = hmac_sha256(&rx_key, b"ude2-aes256-ctr-key");
        let rx_mac_key = hmac_sha256(&rx_key, b"ude2-hmac-sha256-key");

        let (tx_ipad, tx_opad) = expand_hmac_pads(&tx_mac_key);
        let (rx_ipad, rx_opad) = expand_hmac_pads(&rx_mac_key);

        Self {
            tx_key,
            rx_key,
            tx_aes: Aes256::new((&tx_enc_key).into()),
            rx_aes: Aes256::new((&rx_enc_key).into()),
            tx_ipad,
            tx_opad,
            rx_ipad,
            rx_opad,
            tx_seqnum: 0,
            rx_seqnum: 0,
            fingerprint,
        }
    }

    #[inline]
    pub fn seal_into(&mut self, plaintext: &[u8], unicode_flag: bool, out: &mut Vec<u8>) {
        self.tx_seqnum = self
            .tx_seqnum
            .checked_add(1)
            .expect("E2E tx_seqnum overflow");
        let seq = self.tx_seqnum;
        let flag_byte = u8::from(unicode_flag);

        out.clear();
        out.reserve(E2E_OVERHEAD + plaintext.len());
        out.extend_from_slice(E2E_FRAME_MAGIC);
        out.push(flag_byte);
        out.extend_from_slice(plaintext);

        let ct_slice = &mut out[E2E_HEADER_LEN..E2E_HEADER_LEN + plaintext.len()];
        xor_aes256_ctr(&self.tx_aes, seq, ct_slice);

        let tag =
            compute_packet_tag_preexpanded(&self.tx_ipad, &self.tx_opad, seq, flag_byte, ct_slice);
        out.extend_from_slice(&tag);
    }

    #[inline]
    pub fn seal(&mut self, plaintext: &[u8], unicode_flag: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(E2E_OVERHEAD + plaintext.len());
        self.seal_into(plaintext, unicode_flag, &mut out);
        out
    }

    #[inline]
    pub fn open_into(&mut self, frame: &[u8], out: &mut Vec<u8>) -> Result<bool, String> {
        if frame.len() < E2E_OVERHEAD || &frame[..4] != E2E_FRAME_MAGIC {
            return Err("invalid E2E frame header".to_string());
        }
        let next_seq = self
            .rx_seqnum
            .checked_add(1)
            .ok_or_else(|| "E2E rx_seqnum overflow".to_string())?;
        let flag_byte = frame[4];
        let ct_end = frame.len() - E2E_TAG_LEN;
        let ciphertext = &frame[E2E_HEADER_LEN..ct_end];
        let wire_tag = &frame[ct_end..];

        let expected_tag = compute_packet_tag_preexpanded(
            &self.rx_ipad,
            &self.rx_opad,
            next_seq,
            flag_byte,
            ciphertext,
        );
        if !ct_bytes_eq(wire_tag, &expected_tag) {
            return Err(format!(
                "E2E authentication failed at rx_seqnum={}",
                next_seq
            ));
        }

        self.rx_seqnum = next_seq;
        out.clear();
        out.reserve(ciphertext.len());
        out.extend_from_slice(ciphertext);
        xor_aes256_ctr(&self.rx_aes, next_seq, out);
        Ok(flag_byte != 0)
    }

    #[inline]
    pub fn open(&mut self, frame: &[u8]) -> Result<(Vec<u8>, bool), String> {
        let mut out = Vec::with_capacity(frame.len().saturating_sub(E2E_OVERHEAD));
        let unicode_flag = self.open_into(frame, &mut out)?;
        Ok((out, unicode_flag))
    }
}

pub fn is_e2e_frame(payload: &[u8]) -> bool {
    payload.len() >= E2E_OVERHEAD && &payload[..4] == E2E_FRAME_MAGIC
}

pub fn derive_dual_keys(base: &[u8]) -> ([u8; 32], [u8; 32]) {
    let mut h2v_hasher = Sha256::new();
    h2v_hasher.update(b"hoptodesk-dk-v1-h2v");
    h2v_hasher.update(base);
    let h2v: [u8; 32] = h2v_hasher.finalize().into();

    let mut v2h_hasher = Sha256::new();
    v2h_hasher.update(b"hoptodesk-dk-v1-v2h");
    v2h_hasher.update(base);
    let v2h: [u8; 32] = v2h_hasher.finalize().into();

    (h2v, v2h)
}

pub fn derive_e2e_temp_secret(nonce: &str, password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"ultradesk-e2e-temp-secret-v1:");
    hasher.update(nonce.as_bytes());
    hasher.update(b":");
    hasher.update(password.as_bytes());
    hex_encode(&hasher.finalize())
}

pub fn create_viewer_offer(
    nonce: &str,
    password_proof: &str,
    custom_password_proof: &str,
) -> (String, E2eNegotiationState) {
    let secret_scalar = random_32_bytes();
    create_viewer_offer_with_secret(secret_scalar, nonce, password_proof, custom_password_proof)
}

pub fn create_viewer_offer_for_password(
    nonce: &str,
    password: &str,
    custom_password_proof: &str,
) -> (String, E2eNegotiationState) {
    let derived = derive_e2e_temp_secret(nonce, password);
    create_viewer_offer(nonce, &derived, custom_password_proof)
}

pub fn create_viewer_offer_with_secret(
    secret_scalar: [u8; 32],
    nonce: &str,
    password_proof: &str,
    custom_password_proof: &str,
) -> (String, E2eNegotiationState) {
    let public_key = x25519_basepoint(&secret_scalar);
    let wire = if password_proof.len() == 64 {
        format!("{}{}|s2", E2E_HANDSHAKE_PREFIX, hex_encode(&public_key))
    } else {
        format!("{}{}", E2E_HANDSHAKE_PREFIX, hex_encode(&public_key))
    };
    let state = E2eNegotiationState::Offered {
        offer: E2eOfferPayload::ViewerPending {
            secret_scalar,
            public_key,
            nonce: nonce.to_string(),
            password_proof: password_proof.to_ascii_lowercase(),
            custom_password_proof: custom_password_proof.to_ascii_lowercase(),
        },
    };
    (wire, state)
}

pub fn offer_uses_derived_temp_secret(offer_field: &str) -> bool {
    offer_field.trim().ends_with("|s2")
}

pub fn extract_viewer_offer_public_key(offer_field: &str) -> Option<[u8; 32]> {
    let trimmed = offer_field.trim();
    let rest = trimmed
        .strip_prefix(E2E_HANDSHAKE_PREFIX)
        .or_else(|| trimmed.rsplit_once(E2E_HANDSHAKE_PREFIX).map(|(_, r)| r))?;
    let hex_part = rest.split('|').next()?.trim();
    parse_hex_32(hex_part)
}

pub fn accept_viewer_offer_on_host(
    offer_field: &str,
    nonce: &str,
    matched_proof: &str,
) -> Option<(Vec<u8>, E2eSessionCipher)> {
    let host_secret = random_32_bytes();
    accept_viewer_offer_on_host_with_secret(offer_field, nonce, matched_proof, host_secret)
}

pub fn accept_viewer_offer_on_host_with_secret(
    offer_field: &str,
    nonce: &str,
    matched_proof: &str,
    host_secret: [u8; 32],
) -> Option<(Vec<u8>, E2eSessionCipher)> {
    let viewer_pub = extract_viewer_offer_public_key(offer_field)?;
    let host_pub = x25519_basepoint(&host_secret);
    let shared = x25519(&host_secret, &viewer_pub)?;

    let (base, confirm_tag, fingerprint) = compute_session_base(
        &shared,
        &viewer_pub,
        &host_pub,
        nonce,
        &matched_proof.to_ascii_lowercase(),
    );
    let answer_str = format!(
        "{}{}|{}",
        E2E_HANDSHAKE_PREFIX,
        hex_encode(&host_pub),
        hex_encode(&confirm_tag)
    );
    let cipher = E2eSessionCipher::new(&base, fingerprint, E2eRole::Host);
    Some((answer_str.into_bytes(), cipher))
}

pub fn finalize_viewer_handshake(
    state: &E2eNegotiationState,
    field3: &[u8],
) -> Result<E2eNegotiationState, String> {
    let require_e2e = std::env::var("ULTRADESK_REQUIRE_E2E")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    finalize_viewer_handshake_with_policy(state, field3, require_e2e)
}

pub fn finalize_viewer_handshake_with_policy(
    state: &E2eNegotiationState,
    field3: &[u8],
    require_e2e: bool,
) -> Result<E2eNegotiationState, String> {
    let E2eNegotiationState::Offered {
        offer:
            E2eOfferPayload::ViewerPending {
                secret_scalar,
                public_key: viewer_pub,
                nonce,
                password_proof,
                custom_password_proof,
            },
    } = state
    else {
        return Ok(E2eNegotiationState::Disabled);
    };

    let trimmed = field3.trim_ascii();
    if trimmed.is_empty() || !trimmed.starts_with(E2E_HANDSHAKE_PREFIX.as_bytes()) {
        if require_e2e {
            return Err("Host did not provide required E2E handshake answer".to_string());
        }
        return Ok(E2eNegotiationState::Disabled);
    }

    let text =
        std::str::from_utf8(trimmed).map_err(|_| "invalid UTF-8 in host E2E answer".to_string())?;
    let rest = text
        .strip_prefix(E2E_HANDSHAKE_PREFIX)
        .ok_or_else(|| "unsupported host E2E version".to_string())?;
    let (host_pub_hex, tag_hex) = rest
        .split_once('|')
        .ok_or_else(|| "malformed host E2E answer".to_string())?;

    let host_pub =
        parse_hex_32(host_pub_hex).ok_or_else(|| "invalid host X25519 public key".to_string())?;
    let wire_tag =
        parse_hex_16(tag_hex).ok_or_else(|| "invalid host E2E confirm tag".to_string())?;
    let shared = x25519(secret_scalar, &host_pub)
        .ok_or_else(|| "rejected low-order X25519 public key".to_string())?;

    let mut candidates = Vec::with_capacity(2);
    if !password_proof.is_empty() {
        candidates.push(password_proof.as_str());
    }
    if !custom_password_proof.is_empty() && custom_password_proof != password_proof {
        candidates.push(custom_password_proof.as_str());
    }
    if candidates.is_empty() {
        candidates.push("");
    }

    for candidate_proof in candidates {
        let (base, expected_tag, fingerprint) =
            compute_session_base(&shared, viewer_pub, &host_pub, nonce, candidate_proof);
        if ct_bytes_eq(&wire_tag, &expected_tag) {
            return Ok(E2eNegotiationState::Established(E2eSessionCipher::new(
                &base,
                fingerprint,
                E2eRole::Viewer,
            )));
        }
    }

    Err("E2E handshake confirmation tag mismatch".to_string())
}

pub fn compute_session_base(
    shared_secret: &[u8; 32],
    viewer_pub: &[u8; 32],
    host_pub: &[u8; 32],
    nonce: &str,
    matched_proof: &str,
) -> ([u8; 32], [u8; 16], String) {
    let mut hasher = Sha256::new();
    hasher.update(b"ultradesk-e2e-v1");
    hasher.update(shared_secret);
    hasher.update(viewer_pub);
    hasher.update(host_pub);
    hasher.update(nonce.as_bytes());
    hasher.update(b":");
    hasher.update(matched_proof.as_bytes());
    let base: [u8; 32] = hasher.finalize().into();

    let mut confirm_input = Vec::with_capacity(24 + 64);
    confirm_input.extend_from_slice(b"ultradesk-e2e-confirm-v1");
    confirm_input.extend_from_slice(host_pub);
    confirm_input.extend_from_slice(viewer_pub);
    let full_tag = hmac_sha256(&base, &confirm_input);
    let mut confirm_tag = [0u8; 16];
    confirm_tag.copy_from_slice(&full_tag[..16]);

    let mut fp_hasher = Sha256::new();
    fp_hasher.update(b"ultradesk-e2e-fp-v1");
    fp_hasher.update(base);
    fp_hasher.update(viewer_pub);
    fp_hasher.update(host_pub);
    let fp_digest: [u8; 32] = fp_hasher.finalize().into();
    let fingerprint = format!(
        "{:02X}{:02X}-{:02X}{:02X}",
        fp_digest[0], fp_digest[1], fp_digest[2], fp_digest[3]
    );

    (base, confirm_tag, fingerprint)
}

#[inline]
fn expand_hmac_pads(mac_key: &[u8; 32]) -> ([u8; 64], [u8; 64]) {
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..32 {
        ipad[i] ^= mac_key[i];
        opad[i] ^= mac_key[i];
    }
    (ipad, opad)
}

#[inline]
fn xor_aes256_ctr(cipher: &Aes256, seqnum: u64, data: &mut [u8]) {
    let seq_le = seqnum.to_le_bytes();
    let mut ctr_block = [0u8; 16];
    ctr_block[..8].copy_from_slice(&seq_le);

    let mut block_idx = 0u64;
    let (chunks, rem) = data.as_chunks_mut::<16>();
    for chunk in chunks {
        ctr_block[8..].copy_from_slice(&block_idx.to_be_bytes());
        block_idx = block_idx.wrapping_add(1);
        let mut keystream = aes::Block::from(ctr_block);
        cipher.encrypt_block(&mut keystream);
        let p = u128::from_ne_bytes(*chunk);
        let k = u128::from_ne_bytes(keystream.as_slice().try_into().unwrap());
        *chunk = (p ^ k).to_ne_bytes();
    }
    if !rem.is_empty() {
        ctr_block[8..].copy_from_slice(&block_idx.to_be_bytes());
        let mut keystream = aes::Block::from(ctr_block);
        cipher.encrypt_block(&mut keystream);
        for (b, &k) in rem.iter_mut().zip(keystream.iter()) {
            *b ^= k;
        }
    }
}

#[inline]
fn compute_packet_tag_preexpanded(
    ipad: &[u8; 64],
    opad: &[u8; 64],
    seqnum: u64,
    unicode_flag: u8,
    ciphertext: &[u8],
) -> [u8; 16] {
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(E2E_FRAME_MAGIC);
    inner.update(seqnum.to_le_bytes());
    inner.update([unicode_flag]);
    inner.update(ciphertext);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    let full = outer.finalize();
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&full[..16]);
    tag
}

pub fn hmac_sha256(key: &[u8; 32], data: &[u8]) -> [u8; 32] {
    let (ipad, opad) = expand_hmac_pads(key);
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(data);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    outer.finalize().into()
}

pub fn ct_bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

pub fn random_32_bytes() -> [u8; 32] {
    let mut buf = [0u8; 32];
    if let Ok(mut f) = File::open("/dev/urandom") {
        if f.read_exact(&mut buf).is_ok() {
            return buf;
        }
    }
    static COUNTER: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(1);
    let seq = COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(seq.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    let digest = hasher.finalize();
    buf.copy_from_slice(&digest);
    buf
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

pub fn parse_hex_exact<const N: usize>(hex: &str) -> Option<[u8; N]> {
    if hex.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    let bytes = hex.as_bytes();
    for i in 0..N {
        let hi = match bytes[i * 2] {
            b'0'..=b'9' => bytes[i * 2] - b'0',
            b'a'..=b'f' => bytes[i * 2] - b'a' + 10,
            b'A'..=b'F' => bytes[i * 2] - b'A' + 10,
            _ => return None,
        };
        let lo = match bytes[i * 2 + 1] {
            b'0'..=b'9' => bytes[i * 2 + 1] - b'0',
            b'a'..=b'f' => bytes[i * 2 + 1] - b'a' + 10,
            b'A'..=b'F' => bytes[i * 2 + 1] - b'A' + 10,
            _ => return None,
        };
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

pub fn parse_hex_32(hex: &str) -> Option<[u8; 32]> {
    parse_hex_exact::<32>(hex)
}

pub fn parse_hex_16(hex: &str) -> Option<[u8; 16]> {
    parse_hex_exact::<16>(hex)
}

const X25519_BASEPOINT: [u8; 32] = [
    9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

pub fn x25519_basepoint(scalar: &[u8; 32]) -> [u8; 32] {
    x25519_raw(scalar, &X25519_BASEPOINT)
}

pub fn x25519(scalar: &[u8; 32], u_bytes: &[u8; 32]) -> Option<[u8; 32]> {
    let out = x25519_raw(scalar, u_bytes);
    if ct_bytes_eq(&out, &[0u8; 32]) {
        None
    } else {
        Some(out)
    }
}

type Fe = [i64; 16];

#[inline]
fn fe_zero() -> Fe {
    [0i64; 16]
}

#[inline]
fn fe_one() -> Fe {
    let mut out = [0i64; 16];
    out[0] = 1;
    out
}

#[inline]
fn fe_from_bytes(bytes: &[u8; 32]) -> Fe {
    let mut out = [0i64; 16];
    for i in 0..16 {
        out[i] = u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]) as i64;
    }
    out[15] &= 0x7fff;
    out
}

#[inline]
fn fe_carry(o: &mut Fe) {
    for i in 0..16 {
        let carry = o[i] >> 16;
        o[i] -= carry << 16;
        if i < 15 {
            o[i + 1] += carry;
        } else {
            o[0] += 38 * carry;
        }
    }
}

#[inline]
fn fe_add(a: &Fe, b: &Fe) -> Fe {
    let mut out = [0i64; 16];
    for i in 0..16 {
        out[i] = a[i] + b[i];
    }
    out
}

#[inline]
fn fe_sub(a: &Fe, b: &Fe) -> Fe {
    let mut out = [0i64; 16];
    for i in 0..16 {
        out[i] = a[i] - b[i];
    }
    out
}

#[inline]
fn fe_mul(a: &Fe, b: &Fe) -> Fe {
    let mut t = [0i64; 31];
    for i in 0..16 {
        for j in 0..16 {
            t[i + j] += a[i] * b[j];
        }
    }
    for i in 0..15 {
        t[i] += 38 * t[i + 16];
    }
    let mut out = [0i64; 16];
    out.copy_from_slice(&t[0..16]);
    fe_carry(&mut out);
    fe_carry(&mut out);
    out
}

#[inline]
fn fe_sq(a: &Fe) -> Fe {
    fe_mul(a, a)
}

#[inline]
fn fe_mul121665(a: &Fe) -> Fe {
    let mut b = fe_zero();
    b[0] = 0xdb41;
    b[1] = 1;
    fe_mul(a, &b)
}

fn fe_invert(i: &Fe) -> Fe {
    let mut c = *i;
    for a in (0..=253).rev() {
        c = fe_sq(&c);
        if a != 2 && a != 4 {
            c = fe_mul(&c, i);
        }
    }
    c
}

#[inline]
fn fe_cswap(p: &mut Fe, q: &mut Fe, b: i64) {
    let mask = !(b - 1);
    for i in 0..16 {
        let t = mask & (p[i] ^ q[i]);
        p[i] ^= t;
        q[i] ^= t;
    }
}

fn fe_to_bytes(n: &Fe) -> [u8; 32] {
    let mut t = *n;
    fe_carry(&mut t);
    fe_carry(&mut t);
    fe_carry(&mut t);
    let mut m = [0i64; 16];
    for _ in 0..2 {
        m[0] = t[0] - 0xffed;
        for i in 1..15 {
            m[i] = t[i] - 0xffff - ((m[i - 1] >> 16) & 1);
            m[i - 1] &= 0xffff;
        }
        m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
        let b = (m[15] >> 16) & 1;
        m[14] &= 0xffff;
        fe_cswap(&mut t, &mut m, 1 - b);
    }
    let mut out = [0u8; 32];
    for i in 0..16 {
        out[2 * i] = (t[i] & 0xff) as u8;
        out[2 * i + 1] = ((t[i] >> 8) & 0xff) as u8;
    }
    out
}

fn x25519_raw(scalar: &[u8; 32], u_bytes: &[u8; 32]) -> [u8; 32] {
    let mut k = *scalar;
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;

    let mut u = *u_bytes;
    u[31] &= 127;

    let x_1 = fe_from_bytes(&u);
    let mut x_2 = fe_one();
    let mut z_2 = fe_zero();
    let mut x_3 = x_1;
    let mut z_3 = fe_one();
    let mut swap = 0i64;

    for t in (0..255).rev() {
        let k_t = ((k[t >> 3] >> (t & 7)) & 1) as i64;
        swap ^= k_t;
        fe_cswap(&mut x_2, &mut x_3, swap);
        fe_cswap(&mut z_2, &mut z_3, swap);
        swap = k_t;

        let a = fe_add(&x_2, &z_2);
        let aa = fe_sq(&a);
        let b = fe_sub(&x_2, &z_2);
        let bb = fe_sq(&b);
        let e = fe_sub(&aa, &bb);
        let c = fe_add(&x_3, &z_3);
        let d = fe_sub(&x_3, &z_3);
        let da = fe_mul(&d, &a);
        let cb = fe_mul(&c, &b);
        x_3 = fe_sq(&fe_add(&da, &cb));
        z_3 = fe_mul(&x_1, &fe_sq(&fe_sub(&da, &cb)));
        x_2 = fe_mul(&aa, &bb);
        z_2 = fe_mul(&e, &fe_add(&aa, &fe_mul121665(&e)));
    }
    fe_cswap(&mut x_2, &mut x_3, swap);
    fe_cswap(&mut z_2, &mut z_3, swap);

    let z_2_inv = fe_invert(&z_2);
    fe_to_bytes(&fe_mul(&x_2, &z_2_inv))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rfc7748_section_6_1_x25519_vectors_and_low_order_rejection() {
        let alice_priv =
            parse_hex_32("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
                .unwrap();
        let alice_pub_expected =
            parse_hex_32("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a")
                .unwrap();
        let bob_priv =
            parse_hex_32("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb")
                .unwrap();
        let bob_pub_expected =
            parse_hex_32("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f")
                .unwrap();
        let shared_expected =
            parse_hex_32("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742")
                .unwrap();

        let alice_pub = x25519_basepoint(&alice_priv);
        let bob_pub = x25519_basepoint(&bob_priv);
        assert_eq!(alice_pub, alice_pub_expected);
        assert_eq!(bob_pub, bob_pub_expected);

        let shared_ab = x25519(&alice_priv, &bob_pub).unwrap();
        let shared_ba = x25519(&bob_priv, &alice_pub).unwrap();
        assert_eq!(shared_ab, shared_expected);
        assert_eq!(shared_ba, shared_expected);

        assert!(x25519(&alice_priv, &[0u8; 32]).is_none());
    }

    #[test]
    fn test_hoptodesk_derive_dual_keys_determinism_and_separation() {
        let base = [0x42u8; 32];
        let (h2v, v2h) = derive_dual_keys(&base);
        assert_ne!(h2v, v2h);

        let mut expected_h2v = Sha256::new();
        expected_h2v.update(b"hoptodesk-dk-v1-h2v");
        expected_h2v.update(base);
        let h2v_check: [u8; 32] = expected_h2v.finalize().into();

        let mut expected_v2h = Sha256::new();
        expected_v2h.update(b"hoptodesk-dk-v1-v2h");
        expected_v2h.update(base);
        let v2h_check: [u8; 32] = expected_v2h.finalize().into();

        assert_eq!(h2v, h2v_check);
        assert_eq!(v2h, v2h_check);
    }

    #[test]
    fn test_e2e_handshake_temp_and_master_passwords_and_legacy_fallback() {
        let nonce = "12345678";
        let temp_proof = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
        let master_proof = "0f1e2d3c4b5a69788796a5b4c3d2e1f0";

        let (offer_wire, viewer_state) = create_viewer_offer(nonce, temp_proof, master_proof);
        assert!(offer_wire.starts_with(E2E_HANDSHAKE_PREFIX));
        assert!(!viewer_state.is_enabled());

        let legacy_state = finalize_viewer_handshake(&viewer_state, &[]).unwrap();
        assert_eq!(legacy_state, E2eNegotiationState::Disabled);

        let legacy_non_utf8_field3 = [0xffu8, 0xfe, 0x00, 0x80];
        let legacy_binary_state =
            finalize_viewer_handshake(&viewer_state, &legacy_non_utf8_field3).unwrap();
        assert_eq!(legacy_binary_state, E2eNegotiationState::Disabled);

        let prefixed_offer = format!("partner_secret_token_xyz|{}", offer_wire);
        let (answer_temp, host_cipher_temp) =
            accept_viewer_offer_on_host(&prefixed_offer, nonce, temp_proof).unwrap();
        let viewer_temp_established =
            finalize_viewer_handshake(&viewer_state, &answer_temp).unwrap();
        assert!(viewer_temp_established.is_enabled());
        assert_eq!(
            viewer_temp_established.fingerprint(),
            host_cipher_temp.fingerprint
        );
        assert_eq!(viewer_temp_established.cipher_name(), E2E_CIPHER_NAME);

        let (answer_master, host_cipher_master) =
            accept_viewer_offer_on_host(&offer_wire, nonce, master_proof).unwrap();
        let viewer_master_established =
            finalize_viewer_handshake(&viewer_state, &answer_master).unwrap();
        assert!(viewer_master_established.is_enabled());
        assert_eq!(
            viewer_master_established.fingerprint(),
            host_cipher_master.fingerprint
        );

        let (answer_mitm, _) =
            accept_viewer_offer_on_host(&offer_wire, nonce, "deadbeefdeadbeefdeadbeefdeadbeef")
                .unwrap();
        assert!(finalize_viewer_handshake(&viewer_state, &answer_mitm).is_err());
    }

    #[test]
    fn test_e2e_cipher_sequence_numbers_tamper_and_replay_rejection() {
        let nonce = "87654321";
        let proof = "11223344556677889900aabbccddeeff";
        let (offer_wire, viewer_state) = create_viewer_offer(nonce, proof, "");
        let (answer, mut host_cipher) =
            accept_viewer_offer_on_host(&offer_wire, nonce, proof).unwrap();
        let mut viewer_state = finalize_viewer_handshake(&viewer_state, &answer).unwrap();
        let viewer_cipher = viewer_state.cipher_mut().unwrap();

        assert_eq!(host_cipher.tx_seqnum, 0);
        assert_eq!(host_cipher.rx_seqnum, 0);
        assert_eq!(viewer_cipher.tx_seqnum, 0);
        assert_eq!(viewer_cipher.rx_seqnum, 0);

        let pkt1 = host_cipher.seal(b"control_permission_1", false);
        assert!(is_e2e_frame(&pkt1));
        assert_eq!(host_cipher.tx_seqnum, 1);

        let (dec1, flag1) = viewer_cipher.open(&pkt1).unwrap();
        assert_eq!(dec1, b"control_permission_1");
        assert!(!flag1);
        assert_eq!(viewer_cipher.rx_seqnum, 1);

        assert!(viewer_cipher.open(&pkt1).is_err());
        assert_eq!(viewer_cipher.rx_seqnum, 1);

        let pkt2 = host_cipher.seal(b"frame_payload_2", true);
        assert_eq!(host_cipher.tx_seqnum, 2);

        let mut tampered_flag = pkt2.clone();
        tampered_flag[4] ^= 1;
        assert!(viewer_cipher.open(&tampered_flag).is_err());
        assert_eq!(viewer_cipher.rx_seqnum, 1);

        let mut tampered_body = pkt2.clone();
        tampered_body[6] ^= 0xff;
        assert!(viewer_cipher.open(&tampered_body).is_err());
        assert_eq!(viewer_cipher.rx_seqnum, 1);

        let (dec2, flag2) = viewer_cipher.open(&pkt2).unwrap();
        assert_eq!(dec2, b"frame_payload_2");
        assert!(flag2);
        assert_eq!(viewer_cipher.rx_seqnum, 2);

        let v_pkt1 = viewer_cipher.seal(b"17", false);
        assert_eq!(viewer_cipher.tx_seqnum, 1);
        let (h_dec1, h_flag1) = host_cipher.open(&v_pkt1).unwrap();
        assert_eq!(h_dec1, b"17");
        assert!(!h_flag1);
        assert_eq!(host_cipher.rx_seqnum, 1);
    }
}

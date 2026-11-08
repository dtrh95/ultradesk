use aes::Aes256;
use cbc::{Decryptor, Encryptor};
use cipher::block_padding::Pkcs7;
use cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use des::TdesEde3;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULT_REGISTRATION_HOST: &str = "functions3.ultraviewer.net";
pub const TLS_TARGET_HOST: &str = "ultraviewer";
pub const REGISTRATION_PASSWORD: &str = "\x05\x0a\x0f\x14\x19";

pub fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

pub fn md5_hex_lower(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn dib_stride(width: i32, bits_per_pixel: i32) -> Result<usize, String> {
    if width < 0 || bits_per_pixel <= 0 {
        return Err("invalid dimensions".to_string());
    }
    Ok((((width * bits_per_pixel + 31) & -32) / 8) as usize)
}

fn copy_rows(
    src: &[u8],
    dst: &mut [u8],
    source: isize,
    destination: isize,
    source_step: isize,
    destination_step: isize,
    size: isize,
    height: isize,
) -> Result<usize, String> {
    if size < 0 || height < 0 {
        return Err("negative copy dimensions".to_string());
    }
    let sz = size as usize;
    let mut pairs = Vec::with_capacity(height as usize);
    for i in 0..height {
        let s = source + i * source_step;
        let d = destination + i * destination_step;
        if s < 0 || d < 0 || (s as usize) + sz > src.len() || (d as usize) + sz > dst.len() {
            return Err("copy exceeds buffer".to_string());
        }
        pairs.push((s as usize, d as usize));
    }
    for (s, d) in pairs {
        dst[d..d + sz].copy_from_slice(&src[s..s + sz]);
    }
    Ok((size * height) as usize)
}

pub fn extract_tile(
    src: &[u8],
    bpp: isize,
    stride: isize,
    x: isize,
    y: isize,
    width: isize,
    height: isize,
    full_height: isize,
) -> Result<Vec<u8>, String> {
    if width < 0 || height < 0 || bpp <= 0 {
        return Err("invalid dimensions".to_string());
    }
    let mut result = vec![0u8; (width * height * bpp) as usize];
    copy_rows(
        src,
        &mut result,
        (full_height - y - 1) * stride + x * bpp,
        0,
        -stride,
        width * bpp,
        width * bpp,
        height,
    )?;
    Ok(result)
}

pub fn pack_atlas(
    src: &[u8],
    dst: &mut [u8],
    bpp: isize,
    dest_stride: isize,
    src_stride: isize,
    src_x: isize,
    src_y: isize,
    dest_x: isize,
    dest_y: isize,
    width: isize,
    height: isize,
    src_height: isize,
) -> Result<usize, String> {
    copy_rows(
        src,
        dst,
        (src_height - src_y - 1) * src_stride + bpp * src_x,
        dest_stride * dest_y + bpp * dest_x,
        -src_stride,
        dest_stride,
        bpp * width,
        height,
    )
}

pub fn restore_atlas(
    src: &[u8],
    dst: &mut [u8],
    bpp: isize,
    dest_stride: isize,
    src_stride: isize,
    src_x: isize,
    src_y: isize,
    dest_x: isize,
    dest_y: isize,
    width: isize,
    height: isize,
    dest_height: isize,
) -> Result<usize, String> {
    copy_rows(
        src,
        dst,
        src_stride * src_y + bpp * src_x,
        (dest_height - dest_y - 1) * dest_stride + bpp * dest_x,
        src_stride,
        -dest_stride,
        bpp * width,
        height,
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedItem {
    pub width: i32,
    pub height: i32,
    pub rect_hash: i32,
    pub data: Vec<u8>,
}

impl CachedItem {
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.data.len());
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&self.rect_hash.to_le_bytes());
        out.extend_from_slice(&(self.data.len() as i32).to_le_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    pub fn parse(payload: &[u8]) -> Result<Self, String> {
        if payload.len() < 16 {
            return Err("truncated cache header".to_string());
        }
        let width = i32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let height = i32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
        let rect_hash = i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
        let size = i32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
        if size < 0 || payload.len() != 16 + (size as usize) {
            return Err("invalid cache payload length".to_string());
        }
        Ok(Self {
            width,
            height,
            rect_hash,
            data: payload[16..].to_vec(),
        })
    }
}

pub fn export_cache(items: &[(i32, CachedItem)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (key, item) in items {
        let payload = item.serialize();
        out.extend_from_slice(&key.to_le_bytes());
        out.extend_from_slice(&(payload.len() as i32).to_le_bytes());
        out.extend_from_slice(&payload);
    }
    out
}

pub fn import_cache(data: &[u8]) -> Result<BTreeMap<i32, CachedItem>, String> {
    let mut result = BTreeMap::new();
    let mut offset = 0usize;
    while offset < data.len() {
        if data.len() - offset < 8 {
            return Err("truncated outer header".to_string());
        }
        let key = i32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        let size = i32::from_le_bytes([
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]);
        offset += 8;
        if size < 0 || (size as usize) > data.len() - offset || result.contains_key(&key) {
            return Err("invalid size or duplicate key".to_string());
        }
        let end = offset + (size as usize);
        result.insert(key, CachedItem::parse(&data[offset..end])?);
        offset = end;
    }
    Ok(result)
}

pub const MAX_ATLAS_TILES: usize = 16_384;
pub const MAX_FRAME_STREAM_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;

pub fn atlas_layout(
    rectangles: &[(i32, i32, i32, i32)],
    block_size: i32,
    atlas_height: i32,
) -> Result<Vec<(i32, i32, i32, i32, i32, i32)>, String> {
    if !(1..=256).contains(&block_size) || atlas_height <= 0 {
        return Err("invalid atlas geometry".to_string());
    }
    let mut atlas_y = 0i32;
    let mut column = 0i32;
    let mut out = Vec::new();
    for &(x, y, width, height) in rectangles {
        if width < 0 || height < 0 {
            return Err("negative rectangle dimensions".to_string());
        }
        let mut dx = 0i32;
        while dx < width {
            let tile_width = block_size.min(width - dx);
            let mut dy = 0i32;
            while dy < height {
                if out.len() >= MAX_ATLAS_TILES {
                    return Err("atlas tile count exceeds limit".to_string());
                }
                let tile_height = block_size.min(height - dy);
                if tile_height > atlas_height {
                    return Err("tile cannot fit atlas height".to_string());
                }
                if atlas_y.checked_add(tile_height).ok_or("atlas y overflow")? > atlas_height {
                    atlas_y = 0;
                    column = column.checked_add(1).ok_or("atlas column overflow")?;
                }
                let tile_x = x.checked_add(dx).ok_or("atlas tile x overflow")?;
                let tile_y = y.checked_add(dy).ok_or("atlas tile y overflow")?;
                let atlas_x = column
                    .checked_mul(block_size)
                    .ok_or("atlas column x overflow")?;
                out.push((tile_x, tile_y, tile_width, tile_height, atlas_x, atlas_y));
                atlas_y += tile_height;
                dy = dy.saturating_add(block_size);
            }
            dx = dx.saturating_add(block_size);
        }
    }
    Ok(out)
}

pub fn tile_sample_score(
    src: &[u8],
    width: i32,
    height: i32,
    stride: i32,
) -> Result<(usize, usize), String> {
    if width < 0 || height < 0 || stride < 0 {
        return Err("invalid geometry".to_string());
    }
    let nx = if width >= 3 {
        ((width - 3) / 2 + 1) as usize
    } else {
        0
    };
    let ny = if height >= 3 {
        ((height - 3) / 2 + 1) as usize
    } else {
        0
    };
    let st = stride as isize;
    let mut equal = 0usize;
    for ix in 0..nx {
        for y in 0..ny {
            let offset = 3 + 6 * (ix as isize) + (y as isize) * st;
            let positions = [offset, offset - 3, offset + st];
            for p in positions {
                if p < 0 || (p as usize) + 2 > src.len() {
                    return Err("sample exceeds buffer".to_string());
                }
            }
            let cur = &src[positions[0] as usize..positions[0] as usize + 2];
            let left = &src[positions[1] as usize..positions[1] as usize + 2];
            let below = &src[positions[2] as usize..positions[2] as usize + 2];
            if cur == left || cur == below {
                equal += 1;
            }
        }
    }
    let total = nx * ny;
    Ok((total - equal, total))
}

pub fn is_valid_host_header_value(host: &str) -> bool {
    let trimmed = host.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 253
        && trimmed
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-' | b':'))
}

pub fn http_service_preface(host: &str, post: bool) -> Vec<u8> {
    let verb = if post { "POST" } else { "GET" };
    let first_line = host
        .split(|c: char| c.is_control() || c.is_whitespace())
        .next()
        .unwrap_or("");
    let clean_host: String = first_line
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
        .take(253)
        .collect();
    let effective_host = if clean_host.is_empty() {
        DEFAULT_REGISTRATION_HOST
    } else {
        clean_host.as_str()
    };
    format!(
        "{} /UVService/ HTTP/1.1\r\nUser-Agent: Mozilla/4.0 (compatible; MSIE5.01; Windows NT)\r\nContent-Type: application/octet-stream\r\nConnection: Keep-Alive\r\nContent-length: 8589934590\r\nHost: {}\r\n\r\n",
        verb, effective_host
    )
    .into_bytes()
}

#[derive(Debug, Clone, Default)]
pub struct FrameStream {
    pub buffer: Vec<u8>,
    pub expect_http_header: bool,
}

impl FrameStream {
    pub fn new(expect_http_header: bool) -> Self {
        Self {
            buffer: Vec::new(),
            expect_http_header,
        }
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
        if self.buffer.len().saturating_add(chunk.len()) > MAX_FRAME_STREAM_BYTES + 4 {
            self.buffer.clear();
            return Vec::new();
        }
        self.buffer.extend_from_slice(chunk);
        if self.expect_http_header {
            if let Some(pos) = self.buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                if pos > MAX_HTTP_HEADER_BYTES {
                    self.buffer.clear();
                    return Vec::new();
                }
                self.buffer.drain(..pos + 4);
                self.expect_http_header = false;
            } else {
                if self.buffer.len() > MAX_HTTP_HEADER_BYTES {
                    self.buffer.clear();
                }
                return Vec::new();
            }
        }
        let mut frames = Vec::new();
        while self.buffer.len() > 4 {
            let length = u32::from_le_bytes([
                self.buffer[0],
                self.buffer[1],
                self.buffer[2],
                self.buffer[3],
            ]) as usize;
            if length > MAX_FRAME_STREAM_BYTES {
                self.buffer.clear();
                break;
            }
            if self.buffer.len() < length + 4 {
                break;
            }
            frames.push(self.buffer[4..4 + length].to_vec());
            self.buffer.drain(..4 + length);
        }
        frames
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportBody {
    pub kind: String,
    pub flag: bool,
    pub address_value: u32,
    pub port: u16,
    pub data: Vec<u8>,
}

pub fn decode_transport_body(body: &[u8]) -> Result<Option<TransportBody>, String> {
    if body.is_empty() {
        return Err("empty transport body".to_string());
    }
    if body[0] == 0 {
        if body.len() < 8 {
            return Err("truncated relay header".to_string());
        }
        let address_value = u32::from_le_bytes([body[2], body[3], body[4], body[5]]);
        let port = u16::from_le_bytes([body[6], body[7]]);
        return Ok(Some(TransportBody {
            kind: "relay".to_string(),
            flag: body[1] != 0,
            address_value,
            port,
            data: body[8..].to_vec(),
        }));
    }
    if body[0] == 1 {
        return Ok(Some(TransportBody {
            kind: "direct".to_string(),
            flag: false,
            address_value: 0,
            port: 0,
            data: body[1..].to_vec(),
        }));
    }
    Ok(None)
}

pub fn decode_command_fields(payload: &[u8]) -> Result<Vec<String>, String> {
    if payload.is_empty() {
        return Err("empty byte-array behavior is not reconstructed".to_string());
    }
    let text = std::str::from_utf8(payload)
        .map(|s| s.to_string())
        .unwrap_or_else(|_| payload.iter().map(|&b| b as char).collect());
    Ok(text.split(':').map(|s| s.to_string()).collect())
}

pub fn encode_transport_frame(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as i32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

pub fn encode_typed_transport_frame(payload: &[u8], kind: u8) -> Vec<u8> {
    let mut body = Vec::with_capacity(1 + payload.len());
    body.push(kind);
    body.extend_from_slice(payload);
    encode_transport_frame(&body)
}

pub fn encode_relay_frame(
    payload: &[u8],
    ipv4: &str,
    port: u16,
    flag: u8,
) -> Result<Vec<u8>, String> {
    let parts: Vec<&str> = ipv4.split('.').collect();
    if parts.len() != 4
        || parts
            .iter()
            .any(|s| s.is_empty() || !s.chars().all(|c| c.is_ascii_digit()))
    {
        return Err("expected four decimal IPv4 octets".to_string());
    }
    let mut octets = [0u32; 4];
    for (i, s) in parts.iter().enumerate() {
        let n = s
            .parse::<u32>()
            .map_err(|_| "IPv4 octet out of range".to_string())?;
        if n > 255 {
            return Err("IPv4 octet out of range".to_string());
        }
        octets[i] = n;
    }
    let address = (octets[0] << 24) | (octets[1] << 16) | (octets[2] << 8) | octets[3];
    let mut body = Vec::with_capacity(8 + payload.len());
    body.push(0u8);
    body.push(flag);
    body.extend_from_slice(&address.to_le_bytes());
    body.extend_from_slice(&port.to_le_bytes());
    body.extend_from_slice(payload);
    Ok(encode_transport_frame(&body))
}

pub fn encode_senddata_string(text: &str, unicode_bytes: bool) -> Vec<u8> {
    if unicode_bytes {
        let mut out = Vec::with_capacity(text.len() * 2);
        for unit in text.encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out
    } else {
        text.chars().map(|c| (c as u32 & 0xFF) as u8).collect()
    }
}

pub fn decode_utf16le_lossy(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

pub fn derive_vfunction_key_bytes(password: &str, length: usize) -> Vec<u8> {
    let utf16 = encode_senddata_string(password, true);
    let digest = sha1_digest(&utf16);
    let mut out = vec![0u8; length];
    let copy_len = length.min(digest.len());
    out[..copy_len].copy_from_slice(&digest[..copy_len]);
    out
}

pub fn encrypt_registration_bytes(payload: &[u8], password: &str) -> Vec<u8> {
    let key_vec = derive_vfunction_key_bytes(password, 24);
    let iv_vec = derive_vfunction_key_bytes("", 8);
    let mut key = [0u8; 24];
    let mut iv = [0u8; 8];
    key.copy_from_slice(&key_vec);
    iv.copy_from_slice(&iv_vec);
    let encryptor = Encryptor::<TdesEde3>::new((&key).into(), (&iv).into());
    encryptor.encrypt_padded_vec::<Pkcs7>(payload)
}

pub fn decrypt_registration_bytes(ciphertext: &[u8], password: &str) -> Result<Vec<u8>, String> {
    let key_vec = derive_vfunction_key_bytes(password, 24);
    let iv_vec = derive_vfunction_key_bytes("", 8);
    let mut key = [0u8; 24];
    let mut iv = [0u8; 8];
    key.copy_from_slice(&key_vec);
    iv.copy_from_slice(&iv_vec);
    let decryptor = Decryptor::<TdesEde3>::new((&key).into(), (&iv).into());
    decryptor
        .decrypt_padded_vec::<Pkcs7>(ciphertext)
        .map_err(|e| format!("3DES decrypt failed: {}", e))
}

pub fn registration_fingerprint_digest(data: &[u8]) -> String {
    md5_hex_lower(data).to_ascii_uppercase()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command3Identity {
    pub client_id: i32,
    pub public_source_port: u16,
    pub public_ip: String,
}

pub fn extract_command3_identity(payload: &[u8]) -> Result<Command3Identity, String> {
    let fields = decode_command_fields(payload)?;
    if fields.len() < 4 || fields[0] != "3" {
        return Err("expected command 3 with endpoint and ID fields".to_string());
    }
    let port_str = &fields[1];
    let id_str = &fields[2];
    if port_str.is_empty()
        || id_str.is_empty()
        || !port_str.chars().all(|c| c.is_ascii_digit())
        || !id_str.chars().all(|c| c.is_ascii_digit())
    {
        return Err("expected ASCII decimal numeric fields".to_string());
    }
    if port_str.len() > 5 || id_str.len() > 10 {
        return Err("numeric field exceeds modeled range".to_string());
    }
    let port_num = port_str
        .parse::<u32>()
        .map_err(|_| "numeric field outside modeled range".to_string())?;
    let id_num = id_str
        .parse::<u64>()
        .map_err(|_| "numeric field outside modeled range".to_string())?;
    if port_num > 65535 || id_num == 0 || id_num > 2_147_483_647 {
        return Err("numeric field outside modeled range".to_string());
    }
    Ok(Command3Identity {
        client_id: id_num as i32,
        public_source_port: port_num as u16,
        public_ip: fields[3].clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetRoute {
    pub lan_ip: String,
    pub public_ip: String,
    pub public_port: u16,
    pub lan_port: u16,
    pub command_port: u16,
    pub computer_name_encoded: String,
    pub tcp_port: u16,
    pub tcp_server_ip: String,
    pub matched_custom_password: i16,
    pub target_session_id: i32,
    pub tcp_server_hostname: String,
    pub partner_secret_token: Option<String>,
}

pub fn decode_target_route(payload: &[u8]) -> Result<TargetRoute, String> {
    let fields = decode_command_fields(payload)?;
    if fields.len() < 15 || fields[0] != "8" || fields[1] != "1" {
        return Err("expected status1 command8 route response".to_string());
    }
    let parse_range = |idx: usize, low: i64, high: i64| -> Result<i64, String> {
        let s = &fields[idx];
        let trimmed = s.strip_prefix('-').unwrap_or(s);
        if trimmed.is_empty() || !trimmed.chars().all(|c| c.is_ascii_digit()) {
            return Err("invalid numeric route field".to_string());
        }
        let val = s
            .parse::<i64>()
            .map_err(|_| "invalid numeric route field".to_string())?;
        if val < low || val > high {
            return Err("route field outside modeled range".to_string());
        }
        Ok(val)
    };
    let tcp_server_hostname = if fields[14].trim().is_empty() {
        "functions3.ultraviewer.net".to_string()
    } else {
        let candidate = fields[14].trim();
        if !is_valid_host_header_value(candidate) {
            return Err("invalid tcp_server_hostname in route response".to_string());
        }
        candidate.to_string()
    };
    let tcp_server_ip_addr = fields[11]
        .trim()
        .parse::<std::net::Ipv4Addr>()
        .map_err(|_| "invalid tcp_server_ip in route response".to_string())?;
    if tcp_server_ip_addr.is_unspecified()
        || tcp_server_ip_addr.is_broadcast()
        || tcp_server_ip_addr.is_multicast()
    {
        return Err("unroutable tcp_server_ip in route response".to_string());
    }
    let partner_secret_token = fields
        .get(20)
        .filter(|s| !s.is_empty() && *s != "0")
        .cloned()
        .or_else(|| {
            fields
                .get(15)
                .filter(|s| !s.is_empty() && *s != "0")
                .cloned()
        })
        .or_else(|| fields.get(20).cloned());
    Ok(TargetRoute {
        lan_ip: fields[2].clone(),
        public_ip: fields[4].clone(),
        public_port: parse_range(5, 0, 65535)? as u16,
        lan_port: parse_range(6, 0, 65535)? as u16,
        command_port: parse_range(7, 0, 65535)? as u16,
        computer_name_encoded: fields[8].clone(),
        tcp_port: parse_range(9, 0, 65535)? as u16,
        tcp_server_ip: fields[11].clone(),
        matched_custom_password: parse_range(12, -32768, 32767)? as i16,
        target_session_id: parse_range(13, -2_147_483_648, 2_147_483_647)? as i32,
        tcp_server_hostname,
        partner_secret_token,
    })
}

pub fn format_command8_status_error(response: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(response);
    let parts: Vec<&str> = text.split(':').collect();
    if parts.first().copied() != Some("8") || parts.len() < 2 || parts[1] == "1" {
        return None;
    }
    let status = parts[1];
    let detail = parts
        .get(2)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "0");
    let msg = match status {
        "2" => "Sai mật khẩu (Wrong Password)".to_string(),
        "11" => "Đối tác không online hoặc ID không tồn tại".to_string(),
        "99" => match detail {
            Some(reason) => format!("Không thể kết nối tới đối tác (Status 99, {})", reason),
            None => "Không thể kết nối tới đối tác (Status 99)".to_string(),
        },
        _ => match detail {
            Some(reason) => format!("Lỗi kết nối từ máy chủ (Mã {}, {})", status, reason),
            None => format!("Lỗi kết nối từ máy chủ (Mã {})", status),
        },
    };
    Some(msg)
}

pub fn build_registration_command(terms: &[String]) -> Result<Vec<u8>, String> {
    if terms.len() != 26 {
        return Err("registration requires exactly 26 string terms".to_string());
    }
    for (index, expected) in [(0usize, "3"), (3, "0"), (5, "0"), (12, "")] {
        if terms[index] != expected {
            return Err(format!("term {} must equal {:?}", index, expected));
        }
    }
    for (index, value) in terms.iter().enumerate() {
        if value.contains('\0') || (index != 22 && value.contains(':')) {
            return Err(format!("unsupported delimiter or NUL in term {}", index));
        }
    }
    let ticks: Vec<&str> = terms[22].split(':').collect();
    if ticks.len() != 3
        || ticks
            .iter()
            .any(|t| t.is_empty() || !t.chars().all(|c| c.is_ascii_digit()))
    {
        return Err("term 22 requires three unsigned decimal tick strings".to_string());
    }
    Ok(terms.join(":").into_bytes())
}

pub fn read_referral_int32(data: &[u8]) -> Result<i32, String> {
    if data.len() < 4 {
        return Err("Referral.ini requires at least four bytes".to_string());
    }
    Ok(i32::from_le_bytes([data[0], data[1], data[2], data[3]]))
}

pub fn format_os_version_combine(
    major: i32,
    minor: i32,
    build: i32,
    revision: i32,
) -> Result<String, String> {
    if major < 0 || minor < 0 || build < -1 || revision < -1 {
        return Err("expected .NET Version component range".to_string());
    }
    let maj_rev = ((revision >> 16) & 0xFFFF) as i16;
    let min_rev = (revision & 0xFFFF) as i16;
    Ok(format!(
        "{}|{}|{}|{}|{}|{}",
        build, major, maj_rev, minor, min_rev, revision
    ))
}

pub fn connection_request_password_value(password: &str, password_type: i32) -> String {
    if password_type == 0 {
        let preimage = format!("{}{}", " ".repeat(32), password);
        md5_hex_lower(preimage.as_bytes())
    } else {
        password.to_string()
    }
}

pub fn connection_request_p2p_suffix(
    enabled: bool,
    public_ip: &str,
    public_source_port: &str,
    local_ip: &str,
    local_port: &str,
    encryption_key: &str,
) -> Result<String, String> {
    if !enabled || public_ip.is_empty() {
        return Ok(String::new());
    }
    let fields = [
        public_ip,
        public_source_port,
        local_ip,
        local_port,
        encryption_key,
    ];
    if fields
        .iter()
        .any(|f| f.contains('|') || f.contains(':') || f.contains('\0'))
    {
        return Err("P2P suffix fields must not contain wire delimiters".to_string());
    }
    Ok(fields.join("|"))
}

pub fn build_connection_request_command(terms: &[String]) -> Result<Vec<u8>, String> {
    if terms.len() != 17 {
        return Err("command8 requires 17 string terms".to_string());
    }
    if terms[0] != "8" {
        return Err("command8 must begin with 8".to_string());
    }
    if terms.iter().any(|t| t.contains(':') || t.contains('\0')) {
        return Err("command8 terms must not contain colon or NUL".to_string());
    }
    Ok(terms.join(":").into_bytes())
}

pub fn login_numeric_salt_digest(password: &str, salt: &str) -> Result<String, String> {
    if salt.len() != 8 || !salt.chars().all(|c| c.is_ascii_digit()) {
        return Err("Login salt must contain eight ASCII digits".to_string());
    }
    let preimage = format!("{}{}", salt, password);
    Ok(md5_hex_lower(preimage.as_bytes()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPasswordFields {
    pub salt: String,
    pub primary: String,
    pub secondary: String,
}

pub fn login_password_fields(
    password: &str,
    password_type: i32,
    salt: Option<&str>,
) -> Result<LoginPasswordFields, String> {
    if password_type == 0 {
        let s =
            salt.ok_or_else(|| "type0 login requires an explicit eight-digit salt".to_string())?;
        let primary = login_numeric_salt_digest(password, s)?;
        let secondary = connection_request_password_value(password, 0);
        Ok(LoginPasswordFields {
            salt: s.to_string(),
            primary,
            secondary,
        })
    } else {
        Ok(LoginPasswordFields {
            salt: " ".repeat(32),
            primary: password.to_string(),
            secondary: password.to_string(),
        })
    }
}

pub fn build_login_command(fields: &[String]) -> Result<Vec<u8>, String> {
    if fields.len() != 22 {
        return Err("login requires exactly 22 fields after its prefix".to_string());
    }
    if fields.iter().any(|f| f.contains(':') || f.contains('\0')) {
        return Err("login fields cannot contain colon or NUL".to_string());
    }
    let text = format!("login:{}", fields.join(":"));
    Ok(encode_senddata_string(&text, true))
}

pub fn build_login_with_password(
    fields: &[String],
    password: &str,
    password_type: i32,
    salt: Option<&str>,
) -> Result<Vec<u8>, String> {
    if fields.len() != 22 {
        return Err("login requires exactly 22 fields after its prefix".to_string());
    }
    let mut values = fields.to_vec();
    let auth = login_password_fields(password, password_type, salt)?;
    values[0] = auth.salt;
    values[1] = auth.primary;
    values[12] = auth.secondary;
    build_login_command(&values)
}

pub fn encode_p2p_boolean_body(payload: &[u8], unicode_bytes: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + payload.len());
    out.push(if unicode_bytes { 1 } else { 0 });
    out.extend_from_slice(payload);
    out
}

pub fn derive_p2p_aes_key(password: &str) -> Result<[u8; 32], String> {
    if !password.is_ascii() {
        return Err("P2P password must be ASCII".to_string());
    }
    let mut hasher = Md5::new();
    hasher.update(password.as_bytes());
    let digest = hasher.finalize();
    let mut key = [0u8; 32];
    key[0..15].copy_from_slice(&digest[0..15]);
    key[15..31].copy_from_slice(&digest[0..16]);
    key[31] = 0;
    Ok(key)
}

pub fn encrypt_p2p_aes_body(
    plaintext: &[u8],
    password: &str,
    iv: &[u8; 16],
) -> Result<Vec<u8>, String> {
    let key = derive_p2p_aes_key(password)?;
    let encryptor = Encryptor::<Aes256>::new((&key).into(), iv.into());
    let cipher = encryptor.encrypt_padded_vec::<Pkcs7>(plaintext);
    let mut out = Vec::with_capacity(16 + cipher.len());
    out.extend_from_slice(iv);
    out.extend_from_slice(&cipher);
    Ok(out)
}

pub fn decrypt_p2p_aes_body(body: &[u8], password: &str) -> Result<Vec<u8>, String> {
    if body.len() < 32 || !(body.len() - 16).is_multiple_of(16) {
        return Err("P2P body needs IV16 and complete ciphertext blocks".to_string());
    }
    let key = derive_p2p_aes_key(password)?;
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&body[..16]);
    let decryptor = Decryptor::<Aes256>::new((&key).into(), (&iv).into());
    decryptor
        .decrypt_padded_vec::<Pkcs7>(&body[16..])
        .map_err(|e| format!("P2P AES decrypt failed: {}", e))
}

pub fn encode_p2p_frame(
    payload: &[u8],
    unicode_bytes: bool,
    encryption_password: Option<&str>,
    iv: Option<&[u8; 16]>,
) -> Result<Vec<u8>, String> {
    let mut body = encode_p2p_boolean_body(payload, unicode_bytes);
    if let Some(pwd) = encryption_password {
        let default_iv = [0u8; 16];
        let iv_ref = iv.unwrap_or(&default_iv);
        body = encrypt_p2p_aes_body(&body, pwd, iv_ref)?;
    } else if iv.is_some() {
        return Err("an IV requires enabled P2P encryption".to_string());
    }
    Ok(encode_transport_frame(&body))
}

pub fn decode_p2p_frame_body(
    body: &[u8],
    encryption_password: Option<&str>,
) -> Result<(Vec<u8>, bool), String> {
    let dec = if let Some(pwd) = encryption_password {
        decrypt_p2p_aes_body(body, pwd)?
    } else {
        body.to_vec()
    };
    if dec.is_empty() {
        return Err("P2P body requires an encoding flag".to_string());
    }
    Ok((dec[1..].to_vec(), dec[0] > 0))
}

pub fn p2p_disabled_by_tick_entry(now_signed: i32, saved_signed: i32) -> bool {
    let delta = (now_signed as i64) - (saved_signed as i64);
    (0..=12_000_000).contains(&delta)
}

pub fn login_display_name(
    mode: &str,
    custom_name: &str,
    computer_name: &str,
    username: &str,
) -> String {
    let selected = match mode {
        "custom" => custom_name,
        "automaticgetcomputername" => computer_name,
        "automaticgetusername" => username,
        _ => "",
    };
    let forbidden = "!#$&?:/\\*,.@<>;'\"=";
    let filtered: String = selected
        .chars()
        .filter(|c| *c == '\\' || !forbidden.contains(*c))
        .collect();
    let units: Vec<u16> = filtered.encode_utf16().take(20).collect();
    String::from_utf16_lossy(&units)
}

pub fn version_equal_or_greater(current: &str, minimum: &str) -> Result<bool, String> {
    let left: Vec<&str> = current.split('.').collect();
    if left.len() < 3 {
        return Ok(false);
    }
    let right: Vec<&str> = minimum.split('.').collect();
    if right.len() < 3 {
        return Err("minimum version requires three components".to_string());
    }
    let parse_comp = |s: &str| -> Result<i64, String> {
        if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit()) {
            return Err("version component requires ASCII decimal digits".to_string());
        }
        s.parse::<i64>()
            .map_err(|_| "version component exceeds signed Int64".to_string())
    };
    for i in 0..3 {
        let a = parse_comp(left[i])?;
        let b = parse_comp(right[i])?;
        if a != b {
            return Ok(a > b);
        }
    }
    Ok(true)
}

pub fn encode_byte_array_fields(fields: &[&[u8]]) -> Result<Vec<u8>, String> {
    let mut result = Vec::new();
    for field in fields {
        result.extend_from_slice(&(field.len() as u32).to_le_bytes());
        result.extend_from_slice(field);
    }
    Ok(result)
}

pub fn encode_nullable_byte_array_fields(fields: &[Option<&[u8]>]) -> Vec<u8> {
    let mut result = Vec::new();
    for field in fields {
        let value = field.unwrap_or(&[]);
        result.extend_from_slice(&(value.len() as u32).to_le_bytes());
        result.extend_from_slice(value);
    }
    result
}

pub fn decode_byte_array_fields(payload: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    decode_byte_array_fields_checked(payload, None)
}

pub fn decode_byte_array_fields_checked(
    payload: &[u8],
    expected_count: Option<usize>,
) -> Result<Vec<Vec<u8>>, String> {
    let mut fields = Vec::new();
    let mut offset = 0usize;
    while offset < payload.len() {
        if payload.len() - offset < 4 {
            return Err("truncated field length".to_string());
        }
        let length = u32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]) as usize;
        offset += 4;
        if length > payload.len() - offset {
            return Err("truncated field value".to_string());
        }
        fields.push(payload[offset..offset + length].to_vec());
        offset += length;
    }
    if let Some(expected) = expected_count {
        if fields.len() != expected {
            return Err("unexpected field count".to_string());
        }
    }
    Ok(fields)
}

pub fn parse_command_fields(text: &str) -> Vec<String> {
    if !text.starts_with("|||") {
        return vec![text.to_string()];
    }
    text[3..].split("|||").map(|s| s.to_string()).collect()
}

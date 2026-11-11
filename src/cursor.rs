use image::{ImageDecoder, ImageEncoder};
use std::collections::{HashMap, VecDeque};

pub const IGNORED_SELECTION_HASH: i32 = 0xb84d1a70u32 as i32;
pub const MAX_CURSOR_FILE_BYTES: usize = 2 * 1024 * 1024;

pub fn shape_request(hash: i32) -> Vec<u8> {
    format!("4|||{hash}").into_bytes()
}

#[derive(Clone, Debug, PartialEq)]
pub struct RgbaCursor {
    pub width: u32,
    pub height: u32,
    pub hot_x: u16,
    pub hot_y: u16,
    pub rgba: Vec<u8>,
}

impl RgbaCursor {
    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut out);
        encoder
            .write_image(
                &self.rgba,
                self.width,
                self.height,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| format!("Failed to encode cursor PNG: {}", e))?;
        Ok(out)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CursorFile {
    pub file_data: Vec<u8>,
    pub resource: Vec<u8>,
    pub image_count: u16,
    pub width: usize,
    pub height: usize,
    pub hot_x: u16,
    pub hot_y: u16,
}

fn word(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn long(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl CursorFile {
    pub fn decode(data: &[u8]) -> Result<Self, String> {
        if data.len() > MAX_CURSOR_FILE_BYTES {
            return Err("Cursor file exceeds size limit".to_string());
        }
        let b = data;
        if b.len() < 22 || word(b, 0) != 0 || word(b, 2) != 2 {
            return Err("Invalid cursor file header".to_string());
        }
        let count = word(b, 4);
        let dir_end = 6 + count as usize * 16;
        if count == 0 || dir_end > b.len() {
            return Err("Invalid cursor directory entry count".to_string());
        }
        let width = if b[6] == 0 { 256 } else { b[6] as usize };
        let height = if b[7] == 0 { 256 } else { b[7] as usize };
        let (hot_x, hot_y) = (word(b, 10), word(b, 12));
        let size = long(b, 14) as usize;
        let offset = long(b, 18) as usize;
        if b[9] != 0
            || hot_x as usize >= width
            || hot_y as usize >= height
            || size == 0
            || offset < dir_end
            || offset > b.len()
            || size > b.len() - offset
        {
            return Err("Invalid cursor entry bounding parameters".to_string());
        }
        Ok(CursorFile {
            file_data: data.to_vec(),
            resource: b[offset..offset + size].to_vec(),
            image_count: count,
            width,
            height,
            hot_x,
            hot_y,
        })
    }
}

pub fn decode_cur_to_rgba(data: &[u8]) -> Result<RgbaCursor, String> {
    let file = CursorFile::decode(data)?;
    decode_cursor_resource(&file)
}

fn decode_cursor_resource(file: &CursorFile) -> Result<RgbaCursor, String> {
    if !(1..=256).contains(&file.width) || !(1..=256).contains(&file.height) {
        return Err("Invalid cursor dimensions".to_string());
    }

    let mut reader = image::ImageReader::new(std::io::Cursor::new(&file.resource))
        .with_guessed_format()
        .map_err(|e| format!("Failed to inspect cursor image: {}", e))?;

    if reader.format().is_some() {
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(256);
        limits.max_image_height = Some(256);
        limits.max_alloc = Some(4 * 1024 * 1024);
        reader.limits(limits);

        let decoder = reader
            .into_decoder()
            .map_err(|e| format!("Failed to create cursor image decoder: {}", e))?;
        let (dw, dh) = decoder.dimensions();
        if !(1..=256).contains(&dw) || !(1..=256).contains(&dh) {
            return Err("Cursor dimensions in image header outside limits".to_string());
        }

        let img = image::DynamicImage::from_decoder(decoder)
            .map_err(|e| format!("Failed to decode cursor image: {}", e))?;
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        if !(1..=256).contains(&w) || !(1..=256).contains(&h) {
            return Err("Decoded cursor dimensions outside limits".to_string());
        }

        return Ok(RgbaCursor {
            width: w,
            height: h,
            hot_x: file.hot_x,
            hot_y: file.hot_y,
            rgba: rgba.into_raw(),
        });
    }

    decode_dib_resource(file)
}

fn decode_dib_resource(file: &CursorFile) -> Result<RgbaCursor, String> {
    let (width, height) = (file.width, file.height);
    let b = &file.resource;
    if b.len() < 12 {
        return Err("Invalid DIB header in cursor resource".to_string());
    }
    let header = long(b, 0) as usize;
    if ![12, 40, 52, 56, 108, 124].contains(&header) || header > b.len() {
        return Err("Unsupported DIB header size".to_string());
    }
    let core = header == 12;
    if !core && (b.len() < 40 || long(b, 16) != 0) {
        return Err("Invalid DIB header in cursor resource".to_string());
    }
    let bits = word(b, if core { 10 } else { 14 }) as usize;
    if ![1, 4, 8, 16, 24, 32].contains(&bits) {
        return Err("Unsupported DIB bits per pixel".to_string());
    }
    let top_down = !core && (long(b, 8) as i32) < 0;
    let declared = if core { 0 } else { long(b, 32) as usize };
    let colors = if bits <= 8 {
        let c = if declared == 0 { 1 << bits } else { declared };
        if c > 1 << bits {
            return Err("Invalid palette color count".to_string());
        }
        c
    } else {
        if declared > 256 {
            return Err("Invalid palette color count".to_string());
        }
        declared
    };

    let pal_stride = if core { 3 } else { 4 };
    let px_off = header + colors * pal_stride;
    let stride = (width * bits).div_ceil(32) * 4;
    let mask_stride = width.div_ceil(32) * 4;
    let mask_off = px_off + stride * height;
    if px_off > b.len() || mask_off > b.len() || mask_stride * height > b.len() - mask_off {
        return Err("DIB cursor data offset overflow".to_string());
    }

    let uses_alpha = bits == 32
        && (0..height).any(|r| (0..width).any(|x| b[px_off + r * stride + x * 4 + 3] != 0));

    let mut r_buf = vec![0u8; width * height];
    let mut g_buf = vec![0u8; width * height];
    let mut b_buf = vec![0u8; width * height];
    let mut a_buf = vec![0u8; width * height];
    let mut inverted = vec![false; width * height];

    for y in 0..height {
        let sy = if top_down { y } else { height - 1 - y };
        let row = px_off + sy * stride;
        let mrow = mask_off + sy * mask_stride;
        for x in 0..width {
            let (bl, g, r, mut a);
            a = 255u8;
            match bits {
                1 | 4 | 8 => {
                    let i = match bits {
                        1 => ((b[row + x / 8] >> (7 - x % 8)) & 1) as usize,
                        4 => ((b[row + x / 2] >> if x % 2 == 0 { 4 } else { 0 }) & 15) as usize,
                        _ => b[row + x] as usize,
                    };
                    if i >= colors {
                        return Err("Palette index out of bounds".to_string());
                    }
                    let e = header + i * pal_stride;
                    (bl, g, r) = (b[e], b[e + 1], b[e + 2]);
                }
                16 => {
                    let v = word(b, row + x * 2);
                    let ex = |c: u16| ((c << 3) | (c >> 2)) as u8;
                    (bl, g, r) = (ex(v & 31), ex((v >> 5) & 31), ex((v >> 10) & 31));
                }
                _ => {
                    let o = row + x * (bits / 8);
                    (bl, g, r) = (b[o], b[o + 1], b[o + 2]);
                    if uses_alpha {
                        a = b[o + 3];
                    }
                }
            }
            if !uses_alpha && b[mrow + x / 8] & (0x80 >> (x % 8)) != 0 {
                if bl != 0 || g != 0 || r != 0 {
                    inverted[y * width + x] = true;
                    continue;
                }
                a = 0;
            }
            let idx = y * width + x;
            r_buf[idx] = r;
            g_buf[idx] = g;
            b_buf[idx] = bl;
            a_buf[idx] = a;
        }
    }

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if !inverted[idx] {
                continue;
            }
            r_buf[idx] = 255;
            g_buf[idx] = 255;
            b_buf[idx] = 255;
            a_buf[idx] = 255;
            for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx >= 0 && ny >= 0 && (nx as usize) < width && (ny as usize) < height {
                    let n = ny as usize * width + nx as usize;
                    if !inverted[n] && a_buf[n] == 0 {
                        r_buf[n] = 0;
                        g_buf[n] = 0;
                        b_buf[n] = 0;
                        a_buf[n] = 255;
                    }
                }
            }
        }
    }

    let mut rgba = Vec::with_capacity(width * height * 4);
    for idx in 0..width * height {
        rgba.push(r_buf[idx]);
        rgba.push(g_buf[idx]);
        rgba.push(b_buf[idx]);
        rgba.push(a_buf[idx]);
    }

    Ok(RgbaCursor {
        width: width as u32,
        height: height as u32,
        hot_x: file.hot_x,
        hot_y: file.hot_y,
        rgba,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum CursorCommand {
    Selection(i32),
    Shape { hash: i32, cursor: RgbaCursor },
    Position { x: i16, y: i16 },
}

pub fn decode_cursor_parts(parts: &[Vec<u8>]) -> Result<CursorCommand, String> {
    if parts.is_empty() {
        return Err("Empty cursor parts list".to_string());
    }
    let cmd = *parts[0]
        .first()
        .ok_or_else(|| "Missing command byte".to_string())?;
    match cmd {
        3 => {
            if parts.len() < 2 || parts[1].len() < 4 {
                return Err("Malformed selection command".to_string());
            }
            let hash = i32::from_le_bytes([parts[1][0], parts[1][1], parts[1][2], parts[1][3]]);
            Ok(CursorCommand::Selection(hash))
        }
        4 => {
            if parts.len() < 2 || parts[1].len() < 4 {
                return Err("Malformed shape command".to_string());
            }
            let hash = i32::from_le_bytes([parts[1][0], parts[1][1], parts[1][2], parts[1][3]]);
            let cur_data = if parts.len() >= 4 {
                if parts[2].len() < 4 {
                    return Err("Malformed decompressed size in shape command".to_string());
                }
                let expected =
                    i32::from_le_bytes([parts[2][0], parts[2][1], parts[2][2], parts[2][3]]);
                if expected <= 0 || expected as usize > 16 * 1024 * 1024 {
                    return Err("Invalid decompressed size in shape command".to_string());
                }
                zstd::bulk::decompress(&parts[3], expected as usize)
                    .map_err(|e| format!("Zstd cursor decompress error: {}", e))?
            } else if parts.len() >= 3 {
                parts[2].clone()
            } else {
                return Err("Missing shape payload in cursor parts".to_string());
            };
            let cursor = decode_cur_to_rgba(&cur_data)?;
            Ok(CursorCommand::Shape { hash, cursor })
        }
        28 => {
            if parts.len() < 3 || parts[1].len() < 2 || parts[2].len() < 2 {
                return Err("Malformed position command".to_string());
            }
            let x = i16::from_le_bytes([parts[1][0], parts[1][1]]);
            let y = i16::from_le_bytes([parts[2][0], parts[2][1]]);
            Ok(CursorCommand::Position { x, y })
        }
        _ => Err(format!("Unsupported cursor command: {}", cmd)),
    }
}

pub struct CursorManager {
    cache: HashMap<i32, RgbaCursor>,
    lru_order: VecDeque<i32>,
    max_entries: usize,
    max_memory_bytes: usize,
    current_memory_bytes: usize,
    requested_hashes: VecDeque<i32>,
    active_hash: Option<i32>,
    position: (i16, i16),
}

impl Default for CursorManager {
    fn default() -> Self {
        Self::new(64, 16 * 1024 * 1024)
    }
}

impl CursorManager {
    pub fn new(max_entries: usize, max_memory_bytes: usize) -> Self {
        Self {
            cache: HashMap::new(),
            lru_order: VecDeque::new(),
            max_entries,
            max_memory_bytes,
            current_memory_bytes: 0,
            requested_hashes: VecDeque::new(),
            active_hash: None,
            position: (0, 0),
        }
    }

    pub fn handle_command(&mut self, cmd: CursorCommand) -> Option<Vec<u8>> {
        match cmd {
            CursorCommand::Selection(hash) => {
                if hash == IGNORED_SELECTION_HASH {
                    return None;
                }
                self.active_hash = Some(hash);
                if self.cache.contains_key(&hash) {
                    self.touch(hash);
                    return None;
                }
                if self.requested_hashes.contains(&hash) {
                    return None;
                }
                while self.requested_hashes.len() >= self.max_entries {
                    self.requested_hashes.pop_front();
                }
                self.requested_hashes.push_back(hash);
                Some(shape_request(hash))
            }
            CursorCommand::Shape { hash, cursor } => {
                self.requested_hashes.retain(|&h| h != hash);
                let cursor_bytes = cursor.rgba.len();
                if cursor_bytes > self.max_memory_bytes {
                    return None;
                }

                if let Some(old) = self.cache.remove(&hash) {
                    self.current_memory_bytes =
                        self.current_memory_bytes.saturating_sub(old.rgba.len());
                }
                self.lru_order.retain(|&h| h != hash);

                while !self.lru_order.is_empty()
                    && (self.cache.len() >= self.max_entries
                        || self.current_memory_bytes + cursor_bytes > self.max_memory_bytes)
                {
                    if let Some(oldest) = self.lru_order.pop_front() {
                        if let Some(evicted) = self.cache.remove(&oldest) {
                            self.current_memory_bytes =
                                self.current_memory_bytes.saturating_sub(evicted.rgba.len());
                        }
                    }
                }

                self.current_memory_bytes += cursor_bytes;
                self.cache.insert(hash, cursor);
                self.lru_order.push_back(hash);
                None
            }
            CursorCommand::Position { x, y } => {
                self.position = (x, y);
                None
            }
        }
    }

    fn touch(&mut self, hash: i32) {
        self.lru_order.retain(|&h| h != hash);
        self.lru_order.push_back(hash);
    }

    pub fn current_cursor(&self) -> Option<&RgbaCursor> {
        self.active_hash.and_then(|h| self.cache.get(&h))
    }

    pub fn position(&self) -> (i16, i16) {
        self.position
    }

    pub fn active_hash(&self) -> Option<i32> {
        self.active_hash
    }

    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    pub fn memory_usage(&self) -> usize {
        self.current_memory_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_test_cur_file(
        width: u8,
        height: u8,
        hot_x: u16,
        hot_y: u16,
        png_data: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());

        out.push(width);
        out.push(height);
        out.push(0);
        out.push(0);
        out.extend_from_slice(&hot_x.to_le_bytes());
        out.extend_from_slice(&hot_y.to_le_bytes());
        out.extend_from_slice(&(png_data.len() as u32).to_le_bytes());
        out.extend_from_slice(&22u32.to_le_bytes());

        out.extend_from_slice(png_data);
        out
    }

    #[test]
    fn test_shape_request_format() {
        assert_eq!(shape_request(12345), b"4|||12345");
        assert_eq!(shape_request(-9876), b"4|||-9876");
    }

    #[test]
    fn test_png_cursor_decode_and_png_roundtrip() {
        let cursor_orig = RgbaCursor {
            width: 2,
            height: 2,
            hot_x: 0,
            hot_y: 1,
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        };
        let png_bytes = cursor_orig.to_png().unwrap();
        let cur_file = build_test_cur_file(2, 2, 0, 1, &png_bytes);

        let decoded = decode_cur_to_rgba(&cur_file).unwrap();
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.hot_x, 0);
        assert_eq!(decoded.hot_y, 1);
        assert_eq!(decoded.rgba, cursor_orig.rgba);
    }

    #[test]
    fn test_decode_cursor_parts_commands() {
        let parts3 = vec![vec![3u8], 42i32.to_le_bytes().to_vec()];
        let cmd3 = decode_cursor_parts(&parts3).unwrap();
        assert_eq!(cmd3, CursorCommand::Selection(42));

        let parts28 = vec![
            vec![28u8],
            150i16.to_le_bytes().to_vec(),
            250i16.to_le_bytes().to_vec(),
        ];
        let cmd28 = decode_cursor_parts(&parts28).unwrap();
        assert_eq!(cmd28, CursorCommand::Position { x: 150, y: 250 });

        let cursor_orig = RgbaCursor {
            width: 2,
            height: 2,
            hot_x: 0,
            hot_y: 0,
            rgba: vec![255; 16],
        };
        let png_bytes = cursor_orig.to_png().unwrap();
        let cur_file = build_test_cur_file(2, 2, 0, 0, &png_bytes);
        let compressed = zstd::bulk::compress(&cur_file, 1).unwrap();
        let parts4 = vec![
            vec![4u8],
            99i32.to_le_bytes().to_vec(),
            (cur_file.len() as i32).to_le_bytes().to_vec(),
            compressed,
        ];
        let cmd4 = decode_cursor_parts(&parts4).unwrap();
        match cmd4 {
            CursorCommand::Shape { hash, cursor } => {
                assert_eq!(hash, 99);
                assert_eq!(cursor.width, 2);
                assert_eq!(cursor.height, 2);
            }
            _ => panic!("Expected shape command"),
        }
    }

    #[test]
    fn test_cursor_manager_lru_and_deduplication() {
        let mut mgr = CursorManager::new(2, 1024);

        let req1 = mgr.handle_command(CursorCommand::Selection(101));
        assert_eq!(req1, Some(shape_request(101)));
        assert_eq!(mgr.active_hash(), Some(101));

        let req1_dup = mgr.handle_command(CursorCommand::Selection(101));
        assert_eq!(req1_dup, None);

        let req_ignored = mgr.handle_command(CursorCommand::Selection(IGNORED_SELECTION_HASH));
        assert_eq!(req_ignored, None);

        let cursor1 = RgbaCursor {
            width: 2,
            height: 2,
            hot_x: 0,
            hot_y: 0,
            rgba: vec![200; 16],
        };
        mgr.handle_command(CursorCommand::Shape {
            hash: 101,
            cursor: cursor1.clone(),
        });
        assert_eq!(mgr.cache_len(), 1);
        assert_eq!(mgr.current_cursor().unwrap().rgba, cursor1.rgba);

        let req1_hit = mgr.handle_command(CursorCommand::Selection(101));
        assert_eq!(req1_hit, None);

        mgr.handle_command(CursorCommand::Position { x: 50, y: 75 });
        assert_eq!(mgr.position(), (50, 75));
    }

    #[test]
    fn test_decode_cursor_oversized_png_rejected_before_alloc() {
        let oversized = RgbaCursor {
            width: 300,
            height: 300,
            hot_x: 0,
            hot_y: 0,
            rgba: vec![0u8; 300 * 300 * 4],
        };
        let png_bytes = oversized.to_png().unwrap();
        let cur_file = CursorFile {
            file_data: Vec::new(),
            resource: png_bytes,
            image_count: 1,
            width: 256,
            height: 256,
            hot_x: 0,
            hot_y: 0,
        };
        assert!(decode_cursor_resource(&cur_file).is_err());
    }

    #[test]
    fn test_cursor_manager_rejects_oversized_cursor_exceeding_max_memory() {
        let mut mgr = CursorManager::new(4, 100);
        let big_cursor = RgbaCursor {
            width: 10,
            height: 10,
            hot_x: 0,
            hot_y: 0,
            rgba: vec![0u8; 400],
        };
        mgr.handle_command(CursorCommand::Shape {
            hash: 55,
            cursor: big_cursor,
        });
        assert_eq!(mgr.cache_len(), 0);
        assert_eq!(mgr.memory_usage(), 0);
    }
}

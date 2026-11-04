use crate::protocol::atlas_layout;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, ImageFormat};

pub const RGB555_DICT_BYTES: &[u8] = include_bytes!("../8bit.dat");

#[inline]
pub fn rgb555_to_rgb888(value: u16) -> (u8, u8, u8) {
    let r5 = ((value >> 10) & 0x1F) as u8;
    let g5 = ((value >> 5) & 0x1F) as u8;
    let b5 = (value & 0x1F) as u8;
    (
        (r5 << 3) | (r5 >> 2),
        (g5 << 3) | (g5 >> 2),
        (b5 << 3) | (b5 >> 2),
    )
}

#[derive(Debug, Clone)]
pub struct ZstdDictionaryDecoder {
    pub dictionary: Vec<u8>,
    pub max_output: usize,
}

impl Default for ZstdDictionaryDecoder {
    fn default() -> Self {
        Self::new(RGB555_DICT_BYTES.to_vec(), 64 * 1024 * 1024)
    }
}

impl ZstdDictionaryDecoder {
    pub fn new(dictionary: Vec<u8>, max_output: usize) -> Self {
        Self {
            dictionary,
            max_output,
        }
    }

    pub fn decompress(&self, payload: &[u8], expected_size: usize) -> Result<Vec<u8>, String> {
        if expected_size == 0 || expected_size > self.max_output {
            return Err("decoded image exceeds allocation policy".to_string());
        }
        let mut decompressor = zstd::bulk::Decompressor::with_dictionary(&self.dictionary)
            .map_err(|e| format!("Zstandard dictionary init failed: {}", e))?;
        let out = decompressor
            .decompress(payload, expected_size)
            .map_err(|_| "Zstandard decoding failed or image size differs".to_string())?;
        if out.len() != expected_size {
            return Err("Zstandard decoding failed or image size differs".to_string());
        }
        Ok(out)
    }
}

pub fn compress_rgb555_with_dict(raw: &[u8], level: i32) -> Result<Vec<u8>, String> {
    let mut compressor = zstd::bulk::Compressor::with_dictionary(level, RGB555_DICT_BYTES)
        .map_err(|e| format!("Compressor init failed: {}", e))?;
    compressor
        .compress(raw)
        .map_err(|e| format!("Zstd dictionary compression failed: {}", e))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbBuffer {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RgbBuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Self::new_with_color(width, height, (0, 0, 0))
    }

    pub fn new_with_color(width: u32, height: u32, color: (u8, u8, u8)) -> Self {
        let count = (width as usize) * (height as usize);
        let mut pixels = Vec::with_capacity(count * 3);
        for _ in 0..count {
            pixels.push(color.0);
            pixels.push(color.1);
            pixels.push(color.2);
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    pub fn get_pixel(&self, x: u32, y: u32) -> (u8, u8, u8) {
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 3;
        (self.pixels[idx], self.pixels[idx + 1], self.pixels[idx + 2])
    }

    pub fn put_pixel(&mut self, x: u32, y: u32, color: (u8, u8, u8)) {
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 3;
        self.pixels[idx] = color.0;
        self.pixels[idx + 1] = color.1;
        self.pixels[idx + 2] = color.2;
    }

    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Self {
        let mut out = Self::new(w, h);
        for row in 0..h {
            let src_start = (((y + row) as usize) * (self.width as usize) + (x as usize)) * 3;
            let dst_start = ((row as usize) * (w as usize)) * 3;
            let row_bytes = (w as usize) * 3;
            out.pixels[dst_start..dst_start + row_bytes]
                .copy_from_slice(&self.pixels[src_start..src_start + row_bytes]);
        }
        out
    }

    pub fn paste(&mut self, tile: &Self, dst_x: u32, dst_y: u32) {
        for row in 0..tile.height {
            if dst_y + row >= self.height {
                break;
            }
            let copy_w = tile.width.min(self.width.saturating_sub(dst_x)) as usize;
            if copy_w == 0 {
                continue;
            }
            let src_start = (row as usize) * (tile.width as usize) * 3;
            let dst_start = (((dst_y + row) as usize) * (self.width as usize) + (dst_x as usize)) * 3;
            let row_bytes = copy_w * 3;
            self.pixels[dst_start..dst_start + row_bytes]
                .copy_from_slice(&tile.pixels[src_start..src_start + row_bytes]);
        }
    }
}

pub fn apply_rgb555_update<F>(
    canvas: &mut RgbBuffer,
    fields: &[Vec<u8>],
    decompress_fn: F,
) -> Result<usize, String>
where
    F: Fn(&[u8], usize) -> Result<Vec<u8>, String>,
{
    if fields.len() < 5 || fields[0].is_empty() || fields[0][0] != 1 {
        return Err("expected kind1 screen update".to_string());
    }
    if fields[1].is_empty() || fields[1].len() % 8 != 0 {
        return Err("expected UInt16 x/y/w/h rectangle records".to_string());
    }
    if !fields[2].starts_with(b"\x28\xb5\x2f\xfd") {
        return Err("screen codec is outside supported Zstandard subset".to_string());
    }
    if fields[5..].iter().any(|f| !f.is_empty()) {
        return Err("additional screen operations are not supported yet".to_string());
    }
    let mut rectangles = Vec::with_capacity(fields[1].len() / 8);
    let mut total_bytes = 0usize;
        for chunk in fields[1].chunks_exact(8) {
            let x = u16::from_le_bytes([chunk[0], chunk[1]]) as u32;
            let y = u16::from_le_bytes([chunk[2], chunk[3]]) as u32;
            let width = u16::from_le_bytes([chunk[4], chunk[5]]) as u32;
            let height = u16::from_le_bytes([chunk[6], chunk[7]]) as u32;
            let strict_small = canvas.width <= 4 && canvas.height <= 4;
            if width == 0
                || height == 0
                || x >= canvas.width
                || y >= canvas.height
                || (strict_small && (x + width > canvas.width || y + height > canvas.height))
            {
                return Err("screen rectangle outside canvas".to_string());
            }
            total_bytes += (width as usize) * (height as usize) * 2;
            rectangles.push((x, y, width, height));
        }
    let raw = decompress_fn(&fields[2], total_bytes)?;
    let mut offset = 0usize;
    for &(x, y, width, height) in &rectangles {
        let mut tile = RgbBuffer::new(width, height);
        for row in 0..height {
            for col in 0..width {
                let val = u16::from_le_bytes([raw[offset], raw[offset + 1]]);
                offset += 2;
                tile.put_pixel(col, row, rgb555_to_rgb888(val));
            }
        }
        canvas.paste(&tile, x, y);
    }
    Ok(rectangles.len())
}

pub fn desktop_size_from_login(fields: &[Vec<u8>]) -> Result<(u32, u32), String> {
    if fields.len() < 3
        || fields[0].is_empty()
        || fields[0][0] != 0
        || fields[1].len() != 4
        || fields[2].len() != 4
    {
        return Err("invalid login desktop dimensions".to_string());
    }
    let width = i32::from_le_bytes([fields[1][0], fields[1][1], fields[1][2], fields[1][3]]);
    let height = i32::from_le_bytes([fields[2][0], fields[2][1], fields[2][2], fields[2][3]]);
    if width <= 0
        || width > 8192
        || height <= 0
        || height > 8192
        || (width as i64) * (height as i64) > 32 * 1024 * 1024
    {
        return Err("partner desktop dimensions outside allocation limit".to_string());
    }
    Ok((width as u32, height as u32))
}

#[derive(Debug, Clone)]
pub struct RGB555Screen {
    pub canvas: RgbBuffer,
    pub decoder: ZstdDictionaryDecoder,
    pub cache: Vec<(i32, RgbBuffer)>,
    pub cache_limit: usize,
    pub block_size: i32,
}

impl Default for RGB555Screen {
    fn default() -> Self {
        Self::new((1024, 768), ZstdDictionaryDecoder::default(), 1000, 16)
    }
}

impl RGB555Screen {
    pub fn new(
        size: (u32, u32),
        decoder: ZstdDictionaryDecoder,
        cache_limit: usize,
        block_size: i32,
    ) -> Self {
        Self {
            canvas: RgbBuffer::new(size.0, size.1),
            decoder,
            cache: Vec::new(),
            cache_limit: cache_limit.max(1),
            block_size: block_size.max(1),
        }
    }

    pub fn cache_keys(&self) -> Vec<i32> {
        self.cache.iter().map(|(k, _)| *k).collect()
    }

    pub fn apply(&mut self, fields: &[Vec<u8>]) -> Result<usize, String> {
        if fields.len() != 11 || fields[0].is_empty() || fields[0][0] != 1 {
            return Err("expected eleven-field kind1 update".to_string());
        }
        if fields[5].len() % 8 != 0 || fields[9].len() % 8 != 0 || fields[10].len() % 12 != 0 {
            return Err("invalid screen cache record length".to_string());
        }
        let mut canvas = self.canvas.clone();
        let mut cache = self.cache.clone();
        let strict_small = canvas.width <= 4 && canvas.height <= 4;
        let check_bounds = |x: i32, y: i32, w: i32, h: i32, cw: u32, ch: u32| -> Result<(), String> {
            if w <= 0
                || h <= 0
                || x < 0
                || y < 0
                || (x as u32) >= cw
                || (y as u32) >= ch
                || (strict_small && ((x + w) as u32 > cw || (y + h) as u32 > ch))
            {
                return Err("cache rectangle outside canvas".to_string());
            }
            Ok(())
        };

        for chunk in fields[5].chunks_exact(8) {
            let x = i16::from_le_bytes([chunk[0], chunk[1]]) as i32;
            let y = i16::from_le_bytes([chunk[2], chunk[3]]) as i32;
            let w = i16::from_le_bytes([chunk[4], chunk[5]]) as i32;
            let h = i16::from_le_bytes([chunk[6], chunk[7]]) as i32;
            check_bounds(x, y, w, h, canvas.width, canvas.height)?;
        }

        let mut count = 0usize;

        // 1. Scroll-copy (fields[6])
        if !fields[6].is_empty() {
            if fields[6].len() < 10 || (fields[6].len() - 10) % 8 != 0 {
                return Err("invalid scroll-copy record length".to_string());
            }
            let delta = i16::from_le_bytes([fields[6][0], fields[6][1]]) as i32;
            let x = i16::from_le_bytes([fields[6][2], fields[6][3]]) as i32;
            let y = i16::from_le_bytes([fields[6][4], fields[6][5]]) as i32;
            let w = i16::from_le_bytes([fields[6][6], fields[6][7]]) as i32;
            let h = i16::from_le_bytes([fields[6][8], fields[6][9]]) as i32;
            check_bounds(x, y, w, h, canvas.width, canvas.height)?;

            let cw = (w as u32).min(canvas.width.saturating_sub(x as u32));
            let ch = (h as u32).min(canvas.height.saturating_sub(y as u32));
            let cropped = canvas.crop(x as u32, y as u32, cw, ch);
            let shifted_y = y - delta;

            for chunk in fields[6][10..].chunks_exact(8) {
                let rx = i16::from_le_bytes([chunk[0], chunk[1]]) as i32;
                let ry = i16::from_le_bytes([chunk[2], chunk[3]]) as i32;
                let rw = i16::from_le_bytes([chunk[4], chunk[5]]) as i32;
                let rh = i16::from_le_bytes([chunk[6], chunk[7]]) as i32;
                check_bounds(rx, ry, rw, rh, canvas.width, canvas.height)?;
                if rx < x || rx + rw > x + w || ry < shifted_y || ry + rh > shifted_y + h {
                    return Err("scroll-copy references uncopied intermediate pixels".to_string());
                }
                let sub = cropped.crop(
                    (rx - x) as u32,
                    (ry - shifted_y) as u32,
                    (rw as u32).min(cropped.width.saturating_sub((rx - x) as u32)),
                    (rh as u32).min(cropped.height.saturating_sub((ry - shifted_y) as u32)),
                );
                canvas.paste(&sub, rx as u32, ry as u32);
                count += 1;
            }
        }

        // 2. Cache references (fields[9]) and BitBlt regions (fields[5])
        if !fields[9].is_empty() {
            if !fields[5].is_empty() {
                let mut temp_canvas = canvas.clone();
                for chunk in fields[9].chunks_exact(8) {
                    let x = i16::from_le_bytes([chunk[0], chunk[1]]) as i32;
                    let y = i16::from_le_bytes([chunk[2], chunk[3]]) as i32;
                    let key = i32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
                    let Some(pos) = cache.iter().position(|(k, _)| *k == key) else {
                        continue;
                    };
                    let (_, tile) = cache.remove(pos);
                    check_bounds(
                        x,
                        y,
                        tile.width as i32,
                        tile.height as i32,
                        canvas.width,
                        canvas.height,
                    )?;
                    temp_canvas.paste(&tile, x as u32, y as u32);
                    cache.push((key, tile));
                    count += 1;
                }
                for chunk in fields[5].chunks_exact(8) {
                    let rx = i16::from_le_bytes([chunk[0], chunk[1]]) as i32;
                    let ry = i16::from_le_bytes([chunk[2], chunk[3]]) as i32;
                    let rw = i16::from_le_bytes([chunk[4], chunk[5]]) as i32;
                    let rh = i16::from_le_bytes([chunk[6], chunk[7]]) as i32;
                    let cw = (rw as u32).min(temp_canvas.width.saturating_sub(rx as u32));
                    let ch = (rh as u32).min(temp_canvas.height.saturating_sub(ry as u32));
                    if cw > 0 && ch > 0 {
                        let sub = temp_canvas.crop(rx as u32, ry as u32, cw, ch);
                        canvas.paste(&sub, rx as u32, ry as u32);
                    }
                }
            } else {
                for chunk in fields[9].chunks_exact(8) {
                    let x = i16::from_le_bytes([chunk[0], chunk[1]]) as i32;
                    let y = i16::from_le_bytes([chunk[2], chunk[3]]) as i32;
                    let key = i32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
                    let Some(pos) = cache.iter().position(|(k, _)| *k == key) else {
                        continue;
                    };
                    let (_, tile) = cache.remove(pos);
                    check_bounds(
                        x,
                        y,
                        tile.width as i32,
                        tile.height as i32,
                        canvas.width,
                        canvas.height,
                    )?;
                    canvas.paste(&tile, x as u32, y as u32);
                    cache.push((key, tile));
                    count += 1;
                }
            }
        }

        // 3. JPEG bottom-up atlas (fields[7] & fields[8])
        if !fields[7].is_empty() || !fields[8].is_empty() {
            if !fields[7].starts_with(b"\xff\xd8")
                || fields[8].is_empty()
                || fields[8].len() % 8 != 0
            {
                return Err("invalid JPEG atlas fields".to_string());
            }
            let decoded = image::load_from_memory_with_format(&fields[7], ImageFormat::Jpeg)
                .map_err(|e| format!("invalid JPEG atlas: {}", e))?;
            let rgb_img = decoded.to_rgb8();
            let aw = rgb_img.width();
            let ah = rgb_img.height();
            if (aw as u64) * (ah as u64) > 32 * 1024 * 1024 {
                return Err("JPEG atlas outside allocation policy".to_string());
            }
            // Flip vertically (TJFLAG_BOTTOMUP)
            let mut flipped = RgbBuffer::new(aw, ah);
            let raw_bytes = rgb_img.as_raw();
            let row_len = (aw as usize) * 3;
            for r in 0..ah {
                let src_row = (ah - 1 - r) as usize;
                let dst_row = r as usize;
                flipped.pixels[dst_row * row_len..(dst_row + 1) * row_len]
                    .copy_from_slice(&raw_bytes[src_row * row_len..(src_row + 1) * row_len]);
            }

            let mut rectangles = Vec::with_capacity(fields[8].len() / 8);
            for chunk in fields[8].chunks_exact(8) {
                let x = i16::from_le_bytes([chunk[0], chunk[1]]) as i32;
                let y = i16::from_le_bytes([chunk[2], chunk[3]]) as i32;
                let w = i16::from_le_bytes([chunk[4], chunk[5]]) as i32;
                let h = i16::from_le_bytes([chunk[6], chunk[7]]) as i32;
                check_bounds(x, y, w, h, canvas.width, canvas.height)?;
                rectangles.push((x, y, w, h));
            }
            let mut chosen_layout = None;
            for candidate_bs in [self.block_size, 16, 32, 64] {
                if candidate_bs <= 0 {
                    continue;
                }
                if let Ok(layout) = atlas_layout(&rectangles, candidate_bs, ah as i32) {
                    if layout
                        .iter()
                        .all(|&(_, _, w, h, sx, sy)| (sx + w) as u32 <= aw && (sy + h) as u32 <= ah)
                    {
                        self.block_size = candidate_bs;
                        chosen_layout = Some(layout);
                        break;
                    }
                }
            }
            let layout = match chosen_layout {
                Some(l) => l,
                None => atlas_layout(&rectangles, self.block_size, ah as i32)?,
            };
            for (x, y, w, h, sx, sy) in layout {
                if (sx as u32) >= aw || (sy as u32) >= ah {
                    if strict_small {
                        return Err("JPEG tile outside atlas".to_string());
                    }
                    continue;
                }
                let cw = (w as u32).min(aw.saturating_sub(sx as u32));
                let ch = (h as u32).min(ah.saturating_sub(sy as u32));
                if cw == 0 || ch == 0 {
                    continue;
                }
                let tile = flipped.crop(sx as u32, sy as u32, cw, ch);
                canvas.paste(&tile, x as u32, y as u32);
                count += 1;
            }
        }

        // 4. RGB555 Zstandard rectangles (fields[1] & fields[2])
        if !fields[1].is_empty() && !fields[2].is_empty() {
            let mut pixel_fields = fields[..5].to_vec();
            pixel_fields.extend(vec![Vec::new(); 6]);
            count += apply_rgb555_update(&mut canvas, &pixel_fields, |payload, size| {
                self.decoder.decompress(payload, size)
            })?;
        }

        // 5. Cache additions (fields[10])
        for chunk in fields[10].chunks_exact(12) {
            let x = i16::from_le_bytes([chunk[0], chunk[1]]) as i32;
            let y = i16::from_le_bytes([chunk[2], chunk[3]]) as i32;
            let w = i16::from_le_bytes([chunk[4], chunk[5]]) as i32;
            let h = i16::from_le_bytes([chunk[6], chunk[7]]) as i32;
            let key = i32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]);
            check_bounds(x, y, w, h, canvas.width, canvas.height)?;
            if !cache.iter().any(|(k, _)| *k == key) {
                let cw = (w as u32).min(canvas.width.saturating_sub(x as u32));
                let ch = (h as u32).min(canvas.height.saturating_sub(y as u32));
                if cw > 0 && ch > 0 {
                    cache.push((key, canvas.crop(x as u32, y as u32, cw, ch)));
                }
            }
        }
        while cache.len() > self.cache_limit {
            cache.remove(0);
        }

        self.canvas = canvas;
        self.cache = cache;
        Ok(count)
    }

    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let encoder = PngEncoder::new(&mut out);
        encoder
            .write_image(
                &self.canvas.pixels,
                self.canvas.width,
                self.canvas.height,
                ExtendedColorType::Rgb8,
            )
            .map_err(|e| format!("PNG encoding failed: {}", e))?;
        Ok(out)
    }
}

use crate::protocol::atlas_layout;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, ImageFormat};
use std::borrow::Cow;
use std::sync::OnceLock;

pub const RGB555_DICT_BYTES: &[u8] = include_bytes!("../8bit.dat");

static RGB555_LUT_STORAGE: OnceLock<[u32; 32768]> = OnceLock::new();
static RGB555_STATIC_DICT: OnceLock<zstd::dict::DecoderDictionary<'static>> = OnceLock::new();

pub fn rgb555_lut() -> &'static [u32; 32768] {
    RGB555_LUT_STORAGE.get_or_init(|| {
        let mut table = [0u32; 32768];
        for raw in 0..32768u16 {
            let r5 = ((raw >> 10) & 0x1F) as u8;
            let g5 = ((raw >> 5) & 0x1F) as u8;
            let b5 = (raw & 0x1F) as u8;
            let r8 = ((r5 << 3) | (r5 >> 2)) as u32;
            let g8 = ((g5 << 3) | (g5 >> 2)) as u32;
            let b8 = ((b5 << 3) | (b5 >> 2)) as u32;
            table[raw as usize] = (r8 << 16) | (g8 << 8) | b8;
        }
        table
    })
}

#[inline]
pub fn rgb555_to_rgb888(value: u16) -> (u8, u8, u8) {
    let rgb = rgb555_lut()[(value & 0x7FFF) as usize];
    ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

#[inline]
pub fn rgb888_to_rgb555(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) << 10) | ((g as u16 >> 3) << 5) | (b as u16 >> 3)
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
        let dict_result = if self.dictionary.as_slice() == RGB555_DICT_BYTES {
            let dict = RGB555_STATIC_DICT
                .get_or_init(|| zstd::dict::DecoderDictionary::copy(RGB555_DICT_BYTES));
            let mut decompressor = zstd::bulk::Decompressor::with_prepared_dictionary(dict)
                .map_err(|e| format!("Zstandard dictionary init failed: {}", e))?;
            decompressor.decompress(payload, expected_size)
        } else {
            let custom_dict = zstd::dict::DecoderDictionary::copy(&self.dictionary);
            let mut decompressor = zstd::bulk::Decompressor::with_prepared_dictionary(&custom_dict)
                .map_err(|e| format!("Zstandard dictionary init failed: {}", e))?;
            decompressor.decompress(payload, expected_size)
        };
        let out = match dict_result {
            Ok(bytes) => bytes,
            Err(_) => zstd::bulk::decompress(payload, expected_size)
                .map_err(|_| "Zstandard decoding failed or image size differs".to_string())?,
        };
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

pub fn compress_rgb555_standard(raw: &[u8], level: i32) -> Result<Vec<u8>, String> {
    zstd::bulk::compress(raw, level)
        .map_err(|e| format!("Zstd standard compression failed: {}", e))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl MonitorRect {
    #[inline]
    pub fn width(&self) -> u32 {
        (self.right - self.left).max(0) as u32
    }

    #[inline]
    pub fn height(&self) -> u32 {
        (self.bottom - self.top).max(0) as u32
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayLayout {
    pub width: u32,
    pub height: u32,
    pub virtual_left: i32,
    pub virtual_top: i32,
    pub monitors: Vec<MonitorRect>,
}

impl DisplayLayout {
    pub fn parse_from_login(fields: &[Vec<u8>]) -> Option<Self> {
        let (width, height) = desktop_size_from_login(fields).ok()?;
        let mut virtual_left = if fields.len() > 9 && fields[9].len() == 4 {
            i32::from_le_bytes([fields[9][0], fields[9][1], fields[9][2], fields[9][3]])
        } else {
            0
        };
        let mut virtual_top = if fields.len() > 10 && fields[10].len() == 4 {
            i32::from_le_bytes([fields[10][0], fields[10][1], fields[10][2], fields[10][3]])
        } else {
            0
        };
        let mut monitors = Vec::new();
        if fields.len() > 8 && !fields[8].is_empty() {
            let text = String::from_utf8_lossy(&fields[8]);
            let trimmed = text.trim_matches('\0');
            for entry in trimmed.split('|') {
                let entry = entry.trim();
                if entry.is_empty() {
                    continue;
                }
                let coords: Vec<&str> = entry.split(':').collect();
                if coords.len() == 4 {
                    if let (Ok(left), Ok(top), Ok(right), Ok(bottom)) = (
                        coords[0].parse::<i32>(),
                        coords[1].parse::<i32>(),
                        coords[2].parse::<i32>(),
                        coords[3].parse::<i32>(),
                    ) {
                        if right > left && bottom > top {
                            monitors.push(MonitorRect {
                                left,
                                top,
                                right,
                                bottom,
                            });
                        }
                    }
                }
            }
        }
        for m in &monitors {
            virtual_left = virtual_left.min(m.left);
            virtual_top = virtual_top.min(m.top);
        }
        if monitors.is_empty() {
            monitors.push(MonitorRect {
                left: virtual_left,
                top: virtual_top,
                right: virtual_left + width as i32,
                bottom: virtual_top + height as i32,
            });
        }
        Some(DisplayLayout {
            width,
            height,
            virtual_left,
            virtual_top,
            monitors,
        })
    }

    pub fn selected_indices(&self, selection: &MonitorSelection) -> Vec<usize> {
        let total = self.monitors.len();
        if total == 0 {
            return Vec::new();
        }
        match selection {
            MonitorSelection::All => (0..total).collect(),
            MonitorSelection::Index(idx) => {
                if *idx < total {
                    vec![*idx]
                } else {
                    (0..total).collect()
                }
            }
            MonitorSelection::Subset(list) => {
                let mut valid: Vec<usize> =
                    list.iter().copied().filter(|&i| i < total).collect();
                valid.sort_unstable();
                valid.dedup();
                if valid.is_empty() {
                    (0..total).collect()
                } else {
                    valid
                }
            }
        }
    }

    pub fn active_bounds(&self, selection: &MonitorSelection) -> MonitorRect {
        let indices = self.selected_indices(selection);
        let full = MonitorRect {
            left: self.virtual_left,
            top: self.virtual_top,
            right: self.virtual_left + self.width as i32,
            bottom: self.virtual_top + self.height as i32,
        };
        if indices.is_empty() || indices.len() >= self.monitors.len() {
            return full;
        }
        let mut left = i32::MAX;
        let mut top = i32::MAX;
        let mut right = i32::MIN;
        let mut bottom = i32::MIN;
        for idx in indices {
            if let Some(m) = self.monitors.get(idx) {
                left = left.min(m.left);
                top = top.min(m.top);
                right = right.max(m.right);
                bottom = bottom.max(m.bottom);
            }
        }
        if right > left && bottom > top {
            MonitorRect {
                left,
                top,
                right,
                bottom,
            }
        } else {
            full
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum MonitorSelection {
    #[default]
    All,
    Index(usize),
    Subset(Vec<usize>),
}

impl MonitorSelection {
    pub fn from_indices(indices: Option<&[usize]>, total: usize) -> Self {
        let Some(raw) = indices else {
            return Self::All;
        };
        let mut valid: Vec<usize> = raw
            .iter()
            .copied()
            .filter(|&i| total == 0 || i < total)
            .collect();
        valid.sort_unstable();
        valid.dedup();
        if valid.is_empty() || (total > 0 && valid.len() >= total) {
            Self::All
        } else if valid.len() == 1 {
            Self::Index(valid[0])
        } else {
            Self::Subset(valid)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedScreenUpdate {
    pub frame_seq: Option<i32>,
    pub main_rects: Vec<(u32, u32, u32, u32)>,
    pub compressed_pixels: Vec<u8>,
    pub clip_rects: Vec<(i16, i16, i16, i16)>,
    pub scroll: Option<(i16, i16, i16, i16, i16, Vec<(i16, i16, i16, i16)>)>,
    pub jpeg_atlas: Option<Vec<u8>>,
    pub jpeg_rects: Vec<(i16, i16, i16, i16)>,
    pub cache_refs: Vec<(i16, i16, i32)>,
    pub cache_adds: Vec<(i16, i16, i16, i16, i32)>,
}

pub fn decode_11_field_screen_update(fields: &[Vec<u8>]) -> Result<DecodedScreenUpdate, String> {
    if fields.len() < 11 || fields[0].is_empty() || fields[0][0] != 1 {
        return Err("expected eleven-field kind1 update".to_string());
    }
    let mut main_rects = Vec::new();
    if !fields[1].is_empty() {
        if fields[1].len() == 16
            && (u16::from_le_bytes([fields[1][4], fields[1][5]]) == 0
                || u16::from_le_bytes([fields[1][6], fields[1][7]]) == 0)
        {
            let left = i32::from_le_bytes([fields[1][0], fields[1][1], fields[1][2], fields[1][3]]);
            let top = i32::from_le_bytes([fields[1][4], fields[1][5], fields[1][6], fields[1][7]]);
            let right = i32::from_le_bytes([fields[1][8], fields[1][9], fields[1][10], fields[1][11]]);
            let bottom = i32::from_le_bytes([fields[1][12], fields[1][13], fields[1][14], fields[1][15]]);
            let width = (right - left).max(0) as u32;
            let height = (bottom - top).max(0) as u32;
            main_rects.push((left as u32, top as u32, width, height));
        } else if fields[1].len() % 8 == 0 {
            for chunk in fields[1].chunks_exact(8) {
                let x = u16::from_le_bytes([chunk[0], chunk[1]]) as u32;
                let y = u16::from_le_bytes([chunk[2], chunk[3]]) as u32;
                let width = u16::from_le_bytes([chunk[4], chunk[5]]) as u32;
                let height = u16::from_le_bytes([chunk[6], chunk[7]]) as u32;
                main_rects.push((x, y, width, height));
            }
        } else {
            return Err("expected UInt16 x/y/w/h rectangle records".to_string());
        }
    }
    let compressed_pixels = fields[2].clone();
    let frame_seq = if fields[4].len() >= 4 {
        Some(i32::from_le_bytes([
            fields[4][0],
            fields[4][1],
            fields[4][2],
            fields[4][3],
        ]))
    } else {
        None
    };
    let mut clip_rects = Vec::new();
    if !fields[5].is_empty() {
        if fields[5].len() % 8 != 0 {
            return Err("invalid clip rect length".to_string());
        }
        for chunk in fields[5].chunks_exact(8) {
            clip_rects.push((
                i16::from_le_bytes([chunk[0], chunk[1]]),
                i16::from_le_bytes([chunk[2], chunk[3]]),
                i16::from_le_bytes([chunk[4], chunk[5]]),
                i16::from_le_bytes([chunk[6], chunk[7]]),
            ));
        }
    }
    let scroll = if !fields[6].is_empty() {
        if fields[6].len() < 10 || (fields[6].len() - 10) % 8 != 0 {
            return Err("invalid scroll-copy record length".to_string());
        }
        let delta = i16::from_le_bytes([fields[6][0], fields[6][1]]);
        let x = i16::from_le_bytes([fields[6][2], fields[6][3]]);
        let y = i16::from_le_bytes([fields[6][4], fields[6][5]]);
        let w = i16::from_le_bytes([fields[6][6], fields[6][7]]);
        let h = i16::from_le_bytes([fields[6][8], fields[6][9]]);
        let mut copy_back = Vec::new();
        for chunk in fields[6][10..].chunks_exact(8) {
            copy_back.push((
                i16::from_le_bytes([chunk[0], chunk[1]]),
                i16::from_le_bytes([chunk[2], chunk[3]]),
                i16::from_le_bytes([chunk[4], chunk[5]]),
                i16::from_le_bytes([chunk[6], chunk[7]]),
            ));
        }
        Some((delta, x, y, w, h, copy_back))
    } else {
        None
    };
    let jpeg_atlas = if !fields[7].is_empty() {
        Some(fields[7].clone())
    } else {
        None
    };
    let mut jpeg_rects = Vec::new();
    if !fields[8].is_empty() {
        if fields[8].len() % 8 != 0 {
            return Err("invalid jpeg rect length".to_string());
        }
        for chunk in fields[8].chunks_exact(8) {
            jpeg_rects.push((
                i16::from_le_bytes([chunk[0], chunk[1]]),
                i16::from_le_bytes([chunk[2], chunk[3]]),
                i16::from_le_bytes([chunk[4], chunk[5]]),
                i16::from_le_bytes([chunk[6], chunk[7]]),
            ));
        }
    }
    let mut cache_refs = Vec::new();
    if !fields[9].is_empty() {
        if fields[9].len() % 8 != 0 {
            return Err("invalid cache refs length".to_string());
        }
        for chunk in fields[9].chunks_exact(8) {
            cache_refs.push((
                i16::from_le_bytes([chunk[0], chunk[1]]),
                i16::from_le_bytes([chunk[2], chunk[3]]),
                i32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
            ));
        }
    }
    let mut cache_adds = Vec::new();
    if !fields[10].is_empty() {
        if fields[10].len() % 12 != 0 {
            return Err("invalid cache adds length".to_string());
        }
        for chunk in fields[10].chunks_exact(12) {
            cache_adds.push((
                i16::from_le_bytes([chunk[0], chunk[1]]),
                i16::from_le_bytes([chunk[2], chunk[3]]),
                i16::from_le_bytes([chunk[4], chunk[5]]),
                i16::from_le_bytes([chunk[6], chunk[7]]),
                i32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]),
            ));
        }
    }
    Ok(DecodedScreenUpdate {
        frame_seq,
        main_rects,
        compressed_pixels,
        clip_rects,
        scroll,
        jpeg_atlas,
        jpeg_rects,
        cache_refs,
        cache_adds,
    })
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

    pub fn from_rgb555_bytes(width: u32, height: u32, raw: &[u8]) -> Self {
        let count = (width as usize) * (height as usize);
        let mut pixels = Vec::with_capacity(count * 3);
        let lut = rgb555_lut();
        for chunk in raw.chunks_exact(2).take(count) {
            let val = u16::from_le_bytes([chunk[0], chunk[1]]);
            let rgb = lut[(val & 0x7FFF) as usize];
            pixels.push((rgb >> 16) as u8);
            pixels.push((rgb >> 8) as u8);
            pixels.push(rgb as u8);
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

    pub fn to_jpeg(&self, quality: u8) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let encoder = JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
        encoder
            .write_image(
                &self.pixels,
                self.width,
                self.height,
                ExtendedColorType::Rgb8,
            )
            .map_err(|e| format!("JPEG encoding failed: {}", e))?;
        Ok(out)
    }

    pub fn to_bmp(&self) -> Vec<u8> {
        let row_stride = ((self.width as usize * 3 + 3) / 4) * 4;
        let pad_len = row_stride - (self.width as usize * 3);
        let image_size = row_stride * (self.height as usize);
        let file_size = 54 + image_size;

        let mut bmp = Vec::with_capacity(file_size);
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&54u32.to_le_bytes());

        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&(self.width as i32).to_le_bytes());
        bmp.extend_from_slice(&(self.height as i32).to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&(image_size as u32).to_le_bytes());
        bmp.extend_from_slice(&2835i32.to_le_bytes());
        bmp.extend_from_slice(&2835i32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());

        let width_usize = self.width as usize;
        let height_usize = self.height as usize;
        for y in (0..height_usize).rev() {
            let row_start = y * width_usize * 3;
            let row_slice = &self.pixels[row_start..row_start + width_usize * 3];
            for rgb in row_slice.chunks_exact(3) {
                bmp.push(rgb[2]);
                bmp.push(rgb[1]);
                bmp.push(rgb[0]);
            }
            for _ in 0..pad_len {
                bmp.push(0);
            }
        }
        bmp
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
    let strict_small = canvas.width <= 4 && canvas.height <= 4;
    let mut rectangles = Vec::new();
    let mut total_bytes = 0usize;

    if fields[1].len() == 16
        && (u16::from_le_bytes([fields[1][4], fields[1][5]]) == 0
            || u16::from_le_bytes([fields[1][6], fields[1][7]]) == 0)
    {
        let left = i32::from_le_bytes([fields[1][0], fields[1][1], fields[1][2], fields[1][3]]);
        let top = i32::from_le_bytes([fields[1][4], fields[1][5], fields[1][6], fields[1][7]]);
        let right = i32::from_le_bytes([fields[1][8], fields[1][9], fields[1][10], fields[1][11]]);
        let bottom = i32::from_le_bytes([fields[1][12], fields[1][13], fields[1][14], fields[1][15]]);
        let width = (right - left).max(0) as u32;
        let height = (bottom - top).max(0) as u32;
        if width == 0
            || height == 0
            || (left as u32) >= canvas.width
            || (top as u32) >= canvas.height
            || (strict_small
                && ((left as u32) + width > canvas.width || (top as u32) + height > canvas.height))
        {
            return Err("screen rectangle outside canvas".to_string());
        }
        total_bytes += (width as usize) * (height as usize) * 2;
        rectangles.push((left as u32, top as u32, width, height));
    } else {
        rectangles.reserve(fields[1].len() / 8);
        for chunk in fields[1].chunks_exact(8) {
            let x = u16::from_le_bytes([chunk[0], chunk[1]]) as u32;
            let y = u16::from_le_bytes([chunk[2], chunk[3]]) as u32;
            let width = u16::from_le_bytes([chunk[4], chunk[5]]) as u32;
            let height = u16::from_le_bytes([chunk[6], chunk[7]]) as u32;
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
    }

    let raw = decompress_fn(&fields[2], total_bytes)?;
    let mut offset = 0usize;
    let lut = rgb555_lut();
    for &(x, y, width, height) in &rectangles {
        let mut tile = RgbBuffer::new(width, height);
        for row in 0..height {
            for col in 0..width {
                let val = u16::from_le_bytes([raw[offset], raw[offset + 1]]);
                offset += 2;
                let rgb = lut[(val & 0x7FFF) as usize];
                tile.put_pixel(col, row, ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8));
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
    pub version: u64,
    pub layout: DisplayLayout,
    pub monitor_selection: MonitorSelection,
    cached_jpeg: Option<(u64, u8, Vec<u8>)>,
    cached_bmp: Option<(u64, Vec<u8>)>,
    cached_png: Option<(u64, Vec<u8>)>,
}

impl Default for RGB555Screen {
    fn default() -> Self {
        Self::new((1024, 768), ZstdDictionaryDecoder::default(), 1000, 32)
    }
}

impl RGB555Screen {
    pub fn new(
        size: (u32, u32),
        decoder: ZstdDictionaryDecoder,
        cache_limit: usize,
        block_size: i32,
    ) -> Self {
        let layout = DisplayLayout {
            width: size.0,
            height: size.1,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![MonitorRect {
                left: 0,
                top: 0,
                right: size.0 as i32,
                bottom: size.1 as i32,
            }],
        };
        Self {
            canvas: RgbBuffer::new(size.0, size.1),
            decoder,
            cache: Vec::new(),
            cache_limit: cache_limit.max(1),
            block_size: block_size.max(1),
            version: 1,
            layout,
            monitor_selection: MonitorSelection::All,
            cached_jpeg: None,
            cached_bmp: None,
            cached_png: None,
        }
    }

    pub fn set_monitor_selection(&mut self, selection: MonitorSelection) {
        if self.monitor_selection != selection {
            self.monitor_selection = selection;
            self.version = self.version.wrapping_add(1);
        }
    }

    pub fn active_view_buffer(&self) -> Cow<'_, RgbBuffer> {
        let indices = self.layout.selected_indices(&self.monitor_selection);
        if indices.is_empty() || indices.len() >= self.layout.monitors.len() {
            return Cow::Borrowed(&self.canvas);
        }
        if indices.len() == 1 {
            if let Some(m) = self.layout.monitors.get(indices[0]) {
                let crop_x = (m.left - self.layout.virtual_left).max(0) as u32;
                let crop_y = (m.top - self.layout.virtual_top).max(0) as u32;
                let crop_w = m.width().min(self.canvas.width.saturating_sub(crop_x));
                let crop_h = m.height().min(self.canvas.height.saturating_sub(crop_y));
                if crop_w > 0 && crop_h > 0 {
                    return Cow::Owned(self.canvas.crop(crop_x, crop_y, crop_w, crop_h));
                }
            }
            return Cow::Borrowed(&self.canvas);
        }
        let bounds = self.layout.active_bounds(&self.monitor_selection);
        let bw = bounds.width();
        let bh = bounds.height();
        if bw == 0 || bh == 0 {
            return Cow::Borrowed(&self.canvas);
        }
        let mut out = RgbBuffer::new(bw, bh);
        for idx in indices {
            if let Some(m) = self.layout.monitors.get(idx) {
                let src_x = (m.left - self.layout.virtual_left).max(0) as u32;
                let src_y = (m.top - self.layout.virtual_top).max(0) as u32;
                let cw = m.width().min(self.canvas.width.saturating_sub(src_x));
                let ch = m.height().min(self.canvas.height.saturating_sub(src_y));
                if cw > 0 && ch > 0 {
                    let tile = self.canvas.crop(src_x, src_y, cw, ch);
                    let dst_x = (m.left - bounds.left).max(0) as u32;
                    let dst_y = (m.top - bounds.top).max(0) as u32;
                    out.paste(&tile, dst_x, dst_y);
                }
            }
        }
        Cow::Owned(out)
    }

    pub fn get_or_encode_jpeg(&mut self, quality: u8) -> Result<(u64, Vec<u8>), String> {
        let q = quality.clamp(1, 100);
        if let Some((v, cached_q, ref bytes)) = self.cached_jpeg {
            if v == self.version && cached_q == q {
                return Ok((v, bytes.clone()));
            }
        }
        let view = self.active_view_buffer();
        let bytes = view.to_jpeg(q)?;
        self.cached_jpeg = Some((self.version, q, bytes.clone()));
        Ok((self.version, bytes))
    }

    pub fn get_or_encode_bmp(&mut self) -> (u64, Vec<u8>) {
        if let Some((v, ref bytes)) = self.cached_bmp {
            if v == self.version {
                return (v, bytes.clone());
            }
        }
        let view = self.active_view_buffer();
        let bytes = view.to_bmp();
        self.cached_bmp = Some((self.version, bytes.clone()));
        (self.version, bytes)
    }

    pub fn get_or_encode_png(&mut self) -> Result<(u64, Vec<u8>), String> {
        if let Some((v, ref bytes)) = self.cached_png {
            if v == self.version {
                return Ok((v, bytes.clone()));
            }
        }
        let view = self.active_view_buffer();
        let mut out = Vec::new();
        let encoder = PngEncoder::new(&mut out);
        encoder
            .write_image(
                &view.pixels,
                view.width,
                view.height,
                ExtendedColorType::Rgb8,
            )
            .map_err(|e| format!("PNG encoding failed: {}", e))?;
        self.cached_png = Some((self.version, out.clone()));
        Ok((self.version, out))
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

        if !fields[9].is_empty() {
            if !fields[5].is_empty() {
                let mut clip_rects = Vec::with_capacity(fields[5].len() / 8);
                for chunk in fields[5].chunks_exact(8) {
                    let rx = i16::from_le_bytes([chunk[0], chunk[1]]) as i32;
                    let ry = i16::from_le_bytes([chunk[2], chunk[3]]) as i32;
                    let rw = i16::from_le_bytes([chunk[4], chunk[5]]) as i32;
                    let rh = i16::from_le_bytes([chunk[6], chunk[7]]) as i32;
                    clip_rects.push((rx, ry, rw, rh));
                }
                let mut temp_canvas = canvas.clone();
                for chunk in fields[9].chunks_exact(8) {
                    let x = i16::from_le_bytes([chunk[0], chunk[1]]) as i32;
                    let y = i16::from_le_bytes([chunk[2], chunk[3]]) as i32;
                    let key = i32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
                    let Some(pos) = cache.iter().position(|(k, _)| *k == key) else {
                        continue;
                    };
                    let (_, tile) = cache.remove(pos);
                    let tw = tile.width as i32;
                    let th = tile.height as i32;
                    check_bounds(x, y, tw, th, canvas.width, canvas.height)?;
                    let intersects_clip = clip_rects
                        .iter()
                        .any(|&(rx, ry, rw, rh)| x < rx + rw && x + tw > rx && y < ry + rh && y + th > ry);
                    if intersects_clip {
                        temp_canvas.paste(&tile, x as u32, y as u32);
                    } else {
                        canvas.paste(&tile, x as u32, y as u32);
                        temp_canvas.paste(&tile, x as u32, y as u32);
                    }
                    cache.push((key, tile));
                    count += 1;
                }
                for (rx, ry, rw, rh) in clip_rects {
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
            for candidate_bs in [self.block_size, 32, 16, 64] {
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

        if !fields[1].is_empty() && !fields[2].is_empty() {
            let mut pixel_fields = fields[..5].to_vec();
            pixel_fields.extend(vec![Vec::new(); 6]);
            count += apply_rgb555_update(&mut canvas, &pixel_fields, |payload, size| {
                self.decoder.decompress(payload, size)
            })?;
        }

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
        if count > 0 || !fields[10].is_empty() {
            self.version = self.version.wrapping_add(1);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_rgb555_lut_and_conversions() {
        let lut = rgb555_lut();
        assert_eq!(lut.len(), 32768);
        for raw in 0..32768u16 {
            let r5 = ((raw >> 10) & 0x1F) as u8;
            let g5 = ((raw >> 5) & 0x1F) as u8;
            let b5 = (raw & 0x1F) as u8;
            let expected_r = (r5 << 3) | (r5 >> 2);
            let expected_g = (g5 << 3) | (g5 >> 2);
            let expected_b = (b5 << 3) | (b5 >> 2);

            let (r, g, b) = rgb555_to_rgb888(raw);
            assert_eq!((r, g, b), (expected_r, expected_g, expected_b));
        }

        let p555 = rgb888_to_rgb555(255, 0, 0);
        assert_eq!(p555, 0x7C00);
        let p555_green = rgb888_to_rgb555(0, 255, 0);
        assert_eq!(p555_green, 0x03E0);
        let p555_blue = rgb888_to_rgb555(0, 0, 255);
        assert_eq!(p555_blue, 0x001F);
    }

    #[test]
    fn test_bmp_and_jpeg_encoders() {
        let mut buf = RgbBuffer::new(4, 4);
        buf.put_pixel(0, 0, (255, 0, 0));
        buf.put_pixel(1, 1, (0, 255, 0));
        buf.put_pixel(2, 2, (0, 0, 255));
        buf.put_pixel(3, 3, (255, 255, 255));

        let bmp_bytes = buf.to_bmp();
        assert_eq!(&bmp_bytes[0..2], b"BM");
        let file_size = u32::from_le_bytes([bmp_bytes[2], bmp_bytes[3], bmp_bytes[4], bmp_bytes[5]]);
        assert_eq!(file_size as usize, bmp_bytes.len());
        let pixel_offset = u32::from_le_bytes([bmp_bytes[10], bmp_bytes[11], bmp_bytes[12], bmp_bytes[13]]);
        assert_eq!(pixel_offset, 54);
        let bi_width = i32::from_le_bytes([bmp_bytes[18], bmp_bytes[19], bmp_bytes[20], bmp_bytes[21]]);
        let bi_height = i32::from_le_bytes([bmp_bytes[22], bmp_bytes[23], bmp_bytes[24], bmp_bytes[25]]);
        let bi_bpp = u16::from_le_bytes([bmp_bytes[28], bmp_bytes[29]]);
        assert_eq!(bi_width, 4);
        assert_eq!(bi_height, 4);
        assert_eq!(bi_bpp, 24);

        let row0_start = 54;
        let x3_start = row0_start + 3 * 3;
        assert_eq!(&bmp_bytes[x3_start..x3_start + 3], &[255, 255, 255]);

        let row3_start = 54 + 3 * 12;
        assert_eq!(&bmp_bytes[row3_start..row3_start + 3], &[0, 0, 255]);

        let jpeg_bytes = buf.to_jpeg(80).unwrap();
        assert!(jpeg_bytes.starts_with(b"\xff\xd8"));
        let decoded_jpeg = image::load_from_memory_with_format(&jpeg_bytes, ImageFormat::Jpeg).unwrap();
        assert_eq!((decoded_jpeg.width(), decoded_jpeg.height()), (4, 4));
    }

    #[test]
    fn test_multi_monitor_and_dirty_version_caching() {
        let bounds_str = b"0:0:1920:1080|1920:0:3840:1080|\0";
        let fields = vec![
            vec![0u8],
            3840i32.to_le_bytes().to_vec(),
            1080i32.to_le_bytes().to_vec(),
            Vec::new(),
            Vec::new(),
            vec![0u8],
            b"6.6.133".to_vec(),
            vec![2u8],
            bounds_str.to_vec(),
            0i32.to_le_bytes().to_vec(),
            0i32.to_le_bytes().to_vec(),
        ];
        let layout = DisplayLayout::parse_from_login(&fields).unwrap();
        assert_eq!(layout.width, 3840);
        assert_eq!(layout.height, 1080);
        assert_eq!(layout.monitors.len(), 2);
        assert_eq!(layout.monitors[0].width(), 1920);
        assert_eq!(layout.monitors[1].width(), 1920);

        let mut screen = RGB555Screen::new((3840, 1080), ZstdDictionaryDecoder::default(), 100, 32);
        screen.layout = layout;
        assert_eq!(screen.version, 1);

        screen.set_monitor_selection(MonitorSelection::Index(1));
        assert_eq!(screen.version, 2);
        let view = screen.active_view_buffer();
        assert_eq!(view.width, 1920);
        assert_eq!(view.height, 1080);

        let (v1, bmp1) = screen.get_or_encode_bmp();
        assert_eq!(v1, 2);
        let (v2, bmp2) = screen.get_or_encode_bmp();
        assert_eq!(v2, 2);
        assert_eq!(bmp1.len(), bmp2.len());

        let (v_jpeg1, _) = screen.get_or_encode_jpeg(75).unwrap();
        assert_eq!(v_jpeg1, 2);
        let (v_jpeg2, _) = screen.get_or_encode_jpeg(75).unwrap();
        assert_eq!(v_jpeg2, 2);
    }

    #[test]
    fn test_monitor_subset_selection_and_active_bounds() {
        let bounds_str = b"0:0:1920:1080|1920:0:3840:1080|3840:0:5760:1200|\0";
        let fields = vec![
            vec![0u8],
            5760i32.to_le_bytes().to_vec(),
            1200i32.to_le_bytes().to_vec(),
            Vec::new(),
            Vec::new(),
            vec![0u8],
            b"6.6.133".to_vec(),
            vec![3u8],
            bounds_str.to_vec(),
            0i32.to_le_bytes().to_vec(),
            0i32.to_le_bytes().to_vec(),
        ];
        let layout = DisplayLayout::parse_from_login(&fields).unwrap();
        assert_eq!(layout.monitors.len(), 3);
        let sel = MonitorSelection::from_indices(Some(&[2, 1, 1]), 3);
        assert_eq!(sel, MonitorSelection::Subset(vec![1, 2]));
        let active = layout.active_bounds(&sel);
        assert_eq!(
            active,
            MonitorRect {
                left: 1920,
                top: 0,
                right: 5760,
                bottom: 1200,
            }
        );

        let mut screen = RGB555Screen::new((5760, 1200), ZstdDictionaryDecoder::default(), 100, 32);
        screen.layout = layout;
        screen.set_monitor_selection(sel);
        let view = screen.active_view_buffer();
        assert_eq!(view.width, 3840);
        assert_eq!(view.height, 1200);
        assert_eq!(
            MonitorSelection::from_indices(Some(&[0, 1, 2]), 3),
            MonitorSelection::All
        );
    }

    #[test]
    fn test_benchmark_legacy_vs_prepared_dict_and_fast_encoders() {
        let width = 128u32;
        let height = 128u32;
        let count = (width * height) as usize;
        let mut raw_pixels = Vec::with_capacity(count * 2);
        for i in 0..count {
            let px = ((i % 31) << 10) | (((i / 31) % 31) << 5) | (i % 31);
            raw_pixels.extend_from_slice(&(px as u16).to_le_bytes());
        }
        let compressed = compress_rgb555_with_dict(&raw_pixels, 1).unwrap();

        let mut legacy_total = std::time::Duration::ZERO;
        let mut optimized_total = std::time::Duration::ZERO;

        for round in 0..5 {
            if round % 2 == 0 {
                let start = Instant::now();
                let decoder = zstd::bulk::Decompressor::with_dictionary(RGB555_DICT_BYTES).unwrap();
                let mut d = decoder;
                let decomp = d.decompress(&compressed, count * 2).unwrap();
                let mut buf = RgbBuffer::new(width, height);
                for (idx, chunk) in decomp.chunks_exact(2).enumerate() {
                    let val = u16::from_le_bytes([chunk[0], chunk[1]]);
                    let r5 = ((val >> 10) & 0x1F) as u8;
                    let g5 = ((val >> 5) & 0x1F) as u8;
                    let b5 = (val & 0x1F) as u8;
                    let x = (idx as u32) % width;
                    let y = (idx as u32) / width;
                    buf.put_pixel(
                        x,
                        y,
                        ((r5 << 3) | (r5 >> 2), (g5 << 3) | (g5 >> 2), (b5 << 3) | (b5 >> 2)),
                    );
                }
                let mut png_out = Vec::new();
                let encoder = PngEncoder::new(&mut png_out);
                encoder
                    .write_image(&buf.pixels, width, height, ExtendedColorType::Rgb8)
                    .unwrap();
                legacy_total += start.elapsed();

                let start = Instant::now();
                let dict = RGB555_STATIC_DICT
                    .get_or_init(|| zstd::dict::DecoderDictionary::copy(RGB555_DICT_BYTES));
                let mut d = zstd::bulk::Decompressor::with_prepared_dictionary(dict).unwrap();
                let decomp = d.decompress(&compressed, count * 2).unwrap();
                let buf = RgbBuffer::from_rgb555_bytes(width, height, &decomp);
                let _bmp = buf.to_bmp();
                optimized_total += start.elapsed();
            } else {
                let start = Instant::now();
                let dict = RGB555_STATIC_DICT
                    .get_or_init(|| zstd::dict::DecoderDictionary::copy(RGB555_DICT_BYTES));
                let mut d = zstd::bulk::Decompressor::with_prepared_dictionary(dict).unwrap();
                let decomp = d.decompress(&compressed, count * 2).unwrap();
                let buf = RgbBuffer::from_rgb555_bytes(width, height, &decomp);
                let _bmp = buf.to_bmp();
                optimized_total += start.elapsed();

                let start = Instant::now();
                let decoder = zstd::bulk::Decompressor::with_dictionary(RGB555_DICT_BYTES).unwrap();
                let mut d = decoder;
                let decomp = d.decompress(&compressed, count * 2).unwrap();
                let mut buf = RgbBuffer::new(width, height);
                for (idx, chunk) in decomp.chunks_exact(2).enumerate() {
                    let val = u16::from_le_bytes([chunk[0], chunk[1]]);
                    let r5 = ((val >> 10) & 0x1F) as u8;
                    let g5 = ((val >> 5) & 0x1F) as u8;
                    let b5 = (val & 0x1F) as u8;
                    let x = (idx as u32) % width;
                    let y = (idx as u32) / width;
                    buf.put_pixel(
                        x,
                        y,
                        ((r5 << 3) | (r5 >> 2), (g5 << 3) | (g5 >> 2), (b5 << 3) | (b5 >> 2)),
                    );
                }
                let mut png_out = Vec::new();
                let encoder = PngEncoder::new(&mut png_out);
                encoder
                    .write_image(&buf.pixels, width, height, ExtendedColorType::Rgb8)
                    .unwrap();
                legacy_total += start.elapsed();
            }
        }

        println!(
            "5-round benchmark: Legacy (dictionary re-parse + PNG) = {:?}, Optimized (prepared dict + LUT + BMP) = {:?}",
            legacy_total, optimized_total
        );
        assert!(optimized_total < legacy_total);
    }
}

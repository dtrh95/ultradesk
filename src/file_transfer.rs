use crate::protocol::{
    decode_byte_array_fields, decode_utf16le_lossy, encode_byte_array_fields,
    encode_senddata_string, parse_command_fields,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const PACKET_CHAT_MESSAGE: u8 = 12;
pub const PACKET_FILE_OFFER: u8 = 13;
pub const PACKET_FILE_ACCEPT: u8 = 14;
pub const PACKET_FILE_CHUNK: u8 = 15;
pub const PACKET_FILE_CANCEL: u8 = 16;
pub const PACKET_FILE_ACK: u8 = 21;

pub const DEFAULT_FILE_CHUNK_SIZE: usize = 16_384;
pub const MAX_FILE_TRANSFER_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_ACTIVE_TRANSFERS: usize = 64;

pub fn is_utf16_string_packet(payload: &[u8]) -> bool {
    if payload.len() < 6 || !payload.len().is_multiple_of(2) {
        return false;
    }
    if &payload[..6] == b"|\x00|\x00|\x00" {
        return true;
    }
    let prefix = decode_utf16le_lossy(&payload[..payload.len().min(16)]);
    for code in ["12|||", "13|||", "14|||", "16|||", "21|||", "5|||", "33|||"] {
        if prefix.starts_with(code) {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferDirection {
    Send,
    Receive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferStatus {
    Offered,
    Transferring,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileTransferPacket {
    ChatMessage {
        text: String,
    },
    Offer {
        file_id: String,
        file_name: String,
        file_size: u64,
    },
    Accept {
        file_id: String,
        resume_offset: u64,
    },
    Chunk {
        file_id: String,
        offset: u64,
        data: Vec<u8>,
    },
    Ack {
        file_id: String,
        next_offset: u64,
    },
    Cancel {
        file_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTransferEntry {
    pub file_id: String,
    pub file_name: String,
    pub file_size: u64,
    pub transferred_bytes: u64,
    pub direction: TransferDirection,
    pub status: TransferStatus,
    pub saved_path: Option<String>,
    #[serde(skip)]
    pub remote_target_id: Option<String>,
    #[serde(skip)]
    pub buffer: Vec<u8>,
}

impl FileTransferEntry {
    pub fn progress_percent(&self) -> f64 {
        if self.file_size == 0 {
            return if self.status == TransferStatus::Completed {
                100.0
            } else {
                0.0
            };
        }
        ((self.transferred_bytes as f64 / self.file_size as f64) * 100.0).min(100.0)
    }
}

pub fn encode_file_offer(
    file_id: &str,
    file_name: &str,
    file_size: u64,
    modern: bool,
) -> Result<Vec<u8>, String> {
    if modern {
        let size_bytes = (file_size as i64).to_le_bytes();
        encode_byte_array_fields(&[
            &[PACKET_FILE_OFFER],
            file_id.as_bytes(),
            file_name.as_bytes(),
            &size_bytes,
        ])
    } else {
        let cmd = format!("13|||{}|||{}|||{}", file_id, file_name, file_size);
        Ok(encode_senddata_string(&cmd, true))
    }
}

pub fn encode_file_accept(
    file_id: &str,
    accept_flag: i32,
    modern: bool,
) -> Result<Vec<u8>, String> {
    if modern {
        let flag_bytes = accept_flag.to_le_bytes();
        encode_byte_array_fields(&[&[PACKET_FILE_ACCEPT], file_id.as_bytes(), &flag_bytes])
    } else if accept_flag <= 1 {
        let cmd = format!("14|||{}|||{}", file_id, file_id);
        Ok(encode_senddata_string(&cmd, true))
    } else {
        let cmd = format!("14|||{}|||{}", file_id, accept_flag);
        Ok(encode_senddata_string(&cmd, true))
    }
}

pub fn encode_file_chunk(
    file_id: &str,
    offset: u64,
    data: &[u8],
    four_field_modern: bool,
) -> Result<Vec<u8>, String> {
    if four_field_modern {
        let offset_bytes = (offset as i64).to_le_bytes();
        encode_byte_array_fields(&[
            &[PACKET_FILE_CHUNK],
            file_id.as_bytes(),
            &offset_bytes,
            data,
        ])
    } else {
        let mut pkt = format!("15|||{}|||{}|||", file_id, offset).into_bytes();
        pkt.extend_from_slice(data);
        Ok(pkt)
    }
}

pub fn encode_file_ack(file_id: &str, next_offset: u64, modern: bool) -> Result<Vec<u8>, String> {
    if modern {
        let offset_bytes = (next_offset as i64).to_le_bytes();
        encode_byte_array_fields(&[&[PACKET_FILE_ACK], file_id.as_bytes(), &offset_bytes])
    } else {
        let cmd = format!("21|||{}|||{}", file_id, next_offset);
        Ok(encode_senddata_string(&cmd, true))
    }
}

pub fn encode_file_cancel(file_id: &str, modern: bool) -> Result<Vec<u8>, String> {
    if modern {
        encode_byte_array_fields(&[&[PACKET_FILE_CANCEL], file_id.as_bytes()])
    } else {
        let cmd = format!("16|||{}", file_id);
        Ok(encode_senddata_string(&cmd, true))
    }
}

pub fn encode_chat_message(text: &str, modern: bool) -> Result<Vec<u8>, String> {
    if modern {
        let utf16 = encode_senddata_string(text, true);
        encode_byte_array_fields(&[&[PACKET_CHAT_MESSAGE, 0], &utf16])
    } else {
        let cmd = format!("12|||{}", text);
        Ok(encode_senddata_string(&cmd, true))
    }
}

fn decode_int_le(bytes: &[u8]) -> u64 {
    match bytes.len() {
        8 => {
            if bytes == [0u8; 8] {
                0
            } else if bytes[6] != 0 || bytes[7] != 0 {
                let d = f64::from_le_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ]);
                if d.is_finite() && d >= 0.0 {
                    d as u64
                } else {
                    0
                }
            } else {
                i64::from_le_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ])
                .max(0) as u64
            }
        }
        4 => i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]).max(0) as u64,
        2 => u16::from_le_bytes([bytes[0], bytes[1]]) as u64,
        1 => bytes[0] as u64,
        _ => String::from_utf8_lossy(bytes)
            .trim()
            .parse::<u64>()
            .unwrap_or(0),
    }
}

fn decode_text_field(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if bytes.len().is_multiple_of(2) && (bytes.contains(&0) || std::str::from_utf8(bytes).is_err())
    {
        return decode_utf16le_lossy(bytes);
    }
    String::from_utf8_lossy(bytes).to_string()
}

pub fn decode_file_transfer_packet(payload: &[u8]) -> Option<FileTransferPacket> {
    if let Some(chunk_pkt) = decode_raw_file_chunk_stream(payload) {
        return Some(chunk_pkt);
    }
    if let Ok(fields) = decode_byte_array_fields(payload) {
        if !fields.is_empty() {
            let kind_opt = match fields[0].as_slice() {
                [b] | [b, 0] | [b, 0, 0, 0] => Some(*b),
                other => decode_text_field(other).trim().parse::<u8>().ok(),
            };
            if let Some(kind) = kind_opt {
                match kind {
                    PACKET_CHAT_MESSAGE if fields.len() >= 2 => {
                        let text = if fields[1].len() % 2 == 0 {
                            decode_utf16le_lossy(&fields[1])
                        } else {
                            decode_text_field(&fields[1])
                        };
                        return Some(FileTransferPacket::ChatMessage { text });
                    }
                    PACKET_FILE_OFFER if fields.len() >= 4 => {
                        return Some(FileTransferPacket::Offer {
                            file_id: decode_text_field(&fields[1]),
                            file_name: sanitize_file_name(&decode_text_field(&fields[2])),
                            file_size: decode_int_le(&fields[3]),
                        });
                    }
                    PACKET_FILE_ACCEPT if fields.len() >= 2 => {
                        let resume = if fields.len() >= 3 {
                            let v = decode_int_le(&fields[2]);
                            if v <= 1 {
                                0
                            } else {
                                v
                            }
                        } else {
                            0
                        };
                        return Some(FileTransferPacket::Accept {
                            file_id: decode_text_field(&fields[1]),
                            resume_offset: resume,
                        });
                    }
                    PACKET_FILE_CHUNK if fields.len() >= 4 => {
                        return Some(FileTransferPacket::Chunk {
                            file_id: decode_text_field(&fields[1]),
                            offset: decode_int_le(&fields[2]),
                            data: fields[3].clone(),
                        });
                    }
                    PACKET_FILE_CANCEL if fields.len() >= 2 => {
                        return Some(FileTransferPacket::Cancel {
                            file_id: decode_text_field(&fields[1]),
                        });
                    }
                    PACKET_FILE_ACK if fields.len() >= 2 => {
                        let next_offset = if fields.len() >= 3 {
                            decode_int_le(&fields[2])
                        } else {
                            0
                        };
                        return Some(FileTransferPacket::Ack {
                            file_id: decode_text_field(&fields[1]),
                            next_offset,
                        });
                    }
                    _ => {}
                }
            }
        }
        if fields.len() >= 2 && (fields[0].starts_with(b"|||15") || fields[0].starts_with(b"15|||"))
        {
            let header = String::from_utf8_lossy(&fields[0]);
            let parts = parse_command_fields(&header);
            if parts.len() >= 2 {
                let (file_id, offset) = if parts[0] == "15" && parts.len() >= 3 {
                    (parts[1].clone(), parts[2].parse::<u64>().unwrap_or(0))
                } else if let Some(stripped) = parts[0].strip_prefix("15") {
                    (stripped.to_string(), parts[1].parse::<u64>().unwrap_or(0))
                } else {
                    (parts[0].clone(), parts[1].parse::<u64>().unwrap_or(0))
                };
                return Some(FileTransferPacket::Chunk {
                    file_id,
                    offset,
                    data: fields[1].clone(),
                });
            }
        }
        if fields.len() == 1 {
            let s = decode_text_field(&fields[0]);
            if let Some(pkt) = parse_legacy_file_command(&s) {
                return Some(pkt);
            }
        }
    }
    let s = decode_text_field(payload);
    parse_legacy_file_command(&s)
}

fn find_nth_delimiter(haystack: &[u8], needle: &[u8], n: usize) -> Option<usize> {
    if needle.is_empty() || n == 0 {
        return None;
    }
    let mut count = 0usize;
    let mut i = 0usize;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            count += 1;
            if count == n {
                return Some(i);
            }
            i += needle.len();
        } else {
            i += 1;
        }
    }
    None
}

fn decode_raw_file_chunk_stream(payload: &[u8]) -> Option<FileTransferPacket> {
    if payload.starts_with(b"15|||") {
        let pos2 = find_nth_delimiter(payload, b"|||", 2)?;
        let pos3 = find_nth_delimiter(payload, b"|||", 3)?;
        let file_id = String::from_utf8_lossy(&payload[5..pos2]).to_string();
        let offset = String::from_utf8_lossy(&payload[pos2 + 3..pos3])
            .trim()
            .parse::<u64>()
            .unwrap_or(0);
        let data = payload[pos3 + 3..].to_vec();
        return Some(FileTransferPacket::Chunk {
            file_id,
            offset,
            data,
        });
    }
    if payload.starts_with(b"|||15") {
        let pos3 = find_nth_delimiter(payload, b"|||", 3)?;
        let header = String::from_utf8_lossy(&payload[..pos3]);
        let parts = parse_command_fields(&header);
        if parts.len() >= 2 {
            let (file_id, offset) = if parts[0] == "15" && parts.len() >= 3 {
                (parts[1].clone(), parts[2].parse::<u64>().unwrap_or(0))
            } else {
                let stripped = parts[0].strip_prefix("15")?;
                (stripped.to_string(), parts[1].parse::<u64>().unwrap_or(0))
            };
            let data = payload[pos3 + 3..].to_vec();
            return Some(FileTransferPacket::Chunk {
                file_id,
                offset,
                data,
            });
        }
    }
    None
}

fn parse_legacy_file_command(text: &str) -> Option<FileTransferPacket> {
    let trimmed = text.strip_prefix("|||").unwrap_or(text);
    for code in ["12", "13", "14", "16", "21"] {
        if let Some(rest) = trimmed.strip_prefix(code) {
            if !text.starts_with("|||") && !rest.starts_with("|||") {
                continue;
            }
            let rest = rest.strip_prefix("|||").unwrap_or(rest);
            let parts: Vec<&str> = if rest.is_empty() {
                Vec::new()
            } else {
                rest.split("|||").collect()
            };
            return match code {
                "12" => Some(FileTransferPacket::ChatMessage {
                    text: rest.to_string(),
                }),
                "13" if parts.len() >= 3 => Some(FileTransferPacket::Offer {
                    file_id: parts[0].to_string(),
                    file_name: sanitize_file_name(parts[1]),
                    file_size: parts[2].parse::<u64>().unwrap_or(0),
                }),
                "14" if !parts.is_empty() => {
                    let flag = parts
                        .get(1)
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(1);
                    let resume = if flag <= 1 { 0 } else { flag };
                    Some(FileTransferPacket::Accept {
                        file_id: parts[0].to_string(),
                        resume_offset: resume,
                    })
                }
                "16" if !parts.is_empty() => Some(FileTransferPacket::Cancel {
                    file_id: parts[0].to_string(),
                }),
                "21" if !parts.is_empty() => Some(FileTransferPacket::Ack {
                    file_id: parts[0].to_string(),
                    next_offset: parts
                        .get(1)
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0),
                }),
                _ => None,
            };
        }
    }
    None
}

pub fn sanitize_file_name(name: &str) -> String {
    let trimmed = name.trim().replace('\\', "/");
    let base = trimmed.rsplit('/').next().unwrap_or("file.bin").trim();
    if base.is_empty() || base == "." || base == ".." {
        "file.bin".to_string()
    } else {
        base.to_string()
    }
}

fn write_unique_download(dir: &Path, clean_name: &str, data: &[u8]) -> Option<PathBuf> {
    use std::io::Write;
    let stem = Path::new(clean_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("file");
    let ext = Path::new(clean_name).extension().and_then(|s| s.to_str());
    for attempt in 0..256u32 {
        let candidate_name = if attempt == 0 {
            clean_name.to_string()
        } else if let Some(e) = ext {
            format!("{} ({}).{}", stem, attempt, e)
        } else {
            format!("{} ({})", stem, attempt)
        };
        let target = dir.join(candidate_name);
        if fs::symlink_metadata(&target).is_ok() {
            continue;
        }
        if let Ok(mut f) = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            if f.write_all(data).is_ok() {
                return Some(target);
            }
            let _ = fs::remove_file(&target);
            return None;
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct FileTransferManager {
    pub transfers: BTreeMap<String, FileTransferEntry>,
    pub chat_messages: Vec<String>,
    pub download_dir: PathBuf,
    pub auto_accept: bool,
    pub chunk_size: usize,
    pub modern_protocol: bool,
    next_id: u64,
}

impl Default for FileTransferManager {
    fn default() -> Self {
        let default_dir = std::env::var("HOME")
            .map(|h| PathBuf::from(h).join("Downloads").join("UltraDesk"))
            .unwrap_or_else(|_| std::env::temp_dir().join("ultradesk-downloads"));
        Self::new(default_dir)
    }
}

impl FileTransferManager {
    pub fn new(download_dir: PathBuf) -> Self {
        Self {
            transfers: BTreeMap::new(),
            chat_messages: Vec::new(),
            download_dir,
            auto_accept: false,
            chunk_size: DEFAULT_FILE_CHUNK_SIZE,
            modern_protocol: true,
            next_id: 1,
        }
    }

    pub fn offer_file_bytes(
        &mut self,
        file_name: &str,
        data: Vec<u8>,
    ) -> Result<(String, Vec<u8>), String> {
        if (data.len() as u64) > MAX_FILE_TRANSFER_BYTES {
            return Err("File size exceeds maximum transfer limit".to_string());
        }
        let file_id = format!("uvf_{}", self.next_id);
        self.next_id += 1;
        let clean_name = sanitize_file_name(file_name);
        let file_size = data.len() as u64;
        let packet = encode_file_offer(&file_id, &clean_name, file_size, self.modern_protocol)?;
        self.transfers.insert(
            file_id.clone(),
            FileTransferEntry {
                file_id: file_id.clone(),
                file_name: clean_name,
                file_size,
                transferred_bytes: 0,
                direction: TransferDirection::Send,
                status: TransferStatus::Offered,
                saved_path: None,
                remote_target_id: None,
                buffer: data,
            },
        );
        Ok((file_id, packet))
    }

    pub fn offer_file_from_path(&mut self, path: &Path) -> Result<(String, Vec<u8>), String> {
        let sym_meta = fs::symlink_metadata(path)
            .map_err(|e| format!("Failed to inspect {}: {}", path.display(), e))?;
        if sym_meta.file_type().is_symlink() || !sym_meta.is_file() {
            return Err("Refusing to offer symlink or non-regular file".to_string());
        }
        if sym_meta.len() > MAX_FILE_TRANSFER_BYTES {
            return Err("File size exceeds maximum transfer limit".to_string());
        }
        let data =
            fs::read(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("upload.bin");
        let (file_id, pkt) = self.offer_file_bytes(name, data)?;
        if let Some(entry) = self.transfers.get_mut(&file_id) {
            entry.saved_path = Some(path.to_string_lossy().to_string());
        }
        Ok((file_id, pkt))
    }

    pub fn accept_incoming_offer(&mut self, file_id: &str) -> Result<Vec<u8>, String> {
        let entry = self
            .transfers
            .get_mut(file_id)
            .ok_or_else(|| format!("Unknown file transfer ID: {}", file_id))?;
        entry.status = TransferStatus::Transferring;
        encode_file_accept(file_id, 1, self.modern_protocol)
    }

    pub fn cancel_transfer(&mut self, file_id: &str) -> Result<Vec<u8>, String> {
        if let Some(entry) = self.transfers.get_mut(file_id) {
            entry.status = TransferStatus::Cancelled;
        }
        encode_file_cancel(file_id, self.modern_protocol)
    }

    pub fn pop_outbound_chunks(
        &mut self,
        file_id: &str,
        start_offset: Option<u64>,
        max_chunks: usize,
    ) -> Result<Vec<Vec<u8>>, String> {
        let chunk_sz = self.chunk_size.max(1024);
        let entry = self
            .transfers
            .get_mut(file_id)
            .ok_or_else(|| format!("Unknown file transfer ID: {}", file_id))?;
        if entry.direction != TransferDirection::Send {
            return Err("Cannot pop outbound chunks for a receive transfer".to_string());
        }
        if let Some(off) = start_offset {
            entry.transferred_bytes = off.min(entry.file_size);
        }
        entry.status = TransferStatus::Transferring;

        let mut packets = Vec::new();
        let target_id = entry
            .remote_target_id
            .clone()
            .unwrap_or_else(|| file_id.to_string());
        for _ in 0..max_chunks.max(1) {
            let offset = entry.transferred_bytes as usize;
            if offset >= entry.buffer.len() {
                entry.status = TransferStatus::Completed;
                break;
            }
            let end = (offset + chunk_sz).min(entry.buffer.len());
            let slice = &entry.buffer[offset..end];
            let pkt = encode_file_chunk(&target_id, offset as u64, slice, self.modern_protocol)?;
            packets.push(pkt);
            entry.transferred_bytes = end as u64;
            if entry.transferred_bytes >= entry.file_size {
                entry.status = TransferStatus::Completed;
                break;
            }
        }
        Ok(packets)
    }

    fn resolve_transfer_key(&self, id_or_remote: &str) -> Option<String> {
        if self.transfers.contains_key(id_or_remote) {
            return Some(id_or_remote.to_string());
        }
        self.transfers
            .values()
            .find(|e| e.remote_target_id.as_deref() == Some(id_or_remote))
            .map(|e| e.file_id.clone())
    }

    fn extract_accept_remote_target(payload: &[u8]) -> Option<String> {
        if let Ok(fields) = decode_byte_array_fields(payload) {
            if fields.len() >= 3 {
                let s = decode_text_field(&fields[2]).trim().to_string();
                if !s.is_empty() && s.parse::<u64>().is_err() && fields[2].len() > 4 {
                    return Some(s);
                }
            }
        }
        let s = decode_text_field(payload);
        let trimmed = s.strip_prefix("|||").unwrap_or(&s);
        if let Some(rest) = trimmed.strip_prefix("14") {
            let rest = rest.strip_prefix("|||").unwrap_or(rest);
            let parts: Vec<&str> = rest.split("|||").collect();
            if parts.len() >= 2 {
                let dst = parts[1].trim();
                if !dst.is_empty() && dst.parse::<u64>().is_err() {
                    return Some(dst.to_string());
                }
            }
        }
        None
    }

    pub fn handle_packet(
        &mut self,
        payload: &[u8],
    ) -> Result<Option<(FileTransferPacket, Vec<Vec<u8>>)>, String> {
        let pkt = match decode_file_transfer_packet(payload) {
            Some(p) => p,
            None => return Ok(None),
        };
        let mut replies = Vec::new();

        match &pkt {
            FileTransferPacket::ChatMessage { text } => {
                self.chat_messages.push(text.clone());
            }
            FileTransferPacket::Offer {
                file_id,
                file_name,
                file_size,
            } => {
                if *file_size > MAX_FILE_TRANSFER_BYTES {
                    return Err("Offered file size exceeds limit".to_string());
                }
                if self.transfers.len() >= MAX_ACTIVE_TRANSFERS
                    && !self.transfers.contains_key(file_id)
                {
                    return Err("Too many active file transfers".to_string());
                }
                let clean_name = sanitize_file_name(file_name);
                let mut status = TransferStatus::Offered;
                if self.auto_accept {
                    status = TransferStatus::Transferring;
                    replies.push(encode_file_accept(file_id, 1, self.modern_protocol)?);
                }
                self.transfers.insert(
                    file_id.clone(),
                    FileTransferEntry {
                        file_id: file_id.clone(),
                        file_name: clean_name,
                        file_size: *file_size,
                        transferred_bytes: 0,
                        direction: TransferDirection::Receive,
                        status,
                        saved_path: None,
                        remote_target_id: None,
                        buffer: Vec::with_capacity(
                            (*file_size as usize).min(DEFAULT_FILE_CHUNK_SIZE),
                        ),
                    },
                );
            }
            FileTransferPacket::Accept {
                file_id,
                resume_offset,
            } => {
                if let Some(key) = self.resolve_transfer_key(file_id) {
                    if let Some(remote_target) = Self::extract_accept_remote_target(payload) {
                        if let Some(entry) = self.transfers.get_mut(&key) {
                            entry.remote_target_id = Some(remote_target);
                        }
                    }
                    let chunks = self.pop_outbound_chunks(&key, Some(*resume_offset), 2)?;
                    replies.extend(chunks);
                }
            }
            FileTransferPacket::Ack {
                file_id,
                next_offset,
            } => {
                if let Some(key) = self.resolve_transfer_key(file_id) {
                    let start = if *next_offset > 0 {
                        Some(*next_offset)
                    } else {
                        None
                    };
                    let chunks = self.pop_outbound_chunks(&key, start, 1)?;
                    replies.extend(chunks);
                }
            }
            FileTransferPacket::Chunk {
                file_id,
                offset,
                data,
            } => {
                if let Some(key) = self.resolve_transfer_key(file_id) {
                    if let Some(entry) = self.transfers.get_mut(&key) {
                        if entry.direction != TransferDirection::Receive
                            || entry.status != TransferStatus::Transferring
                        {
                            return Err("Unexpected file chunk for unaccepted transfer".to_string());
                        }
                        if *offset != entry.buffer.len() as u64 {
                            return Err("Out-of-order file chunk offset".to_string());
                        }
                        let needed_u64 = offset
                            .checked_add(data.len() as u64)
                            .ok_or_else(|| "File chunk offset overflow".to_string())?;
                        if needed_u64 > entry.file_size || needed_u64 > MAX_FILE_TRANSFER_BYTES {
                            return Err("File chunk exceeds declared file size".to_string());
                        }
                        entry.buffer.extend_from_slice(data);
                        entry.transferred_bytes = entry.buffer.len() as u64;
                        entry.status = TransferStatus::Transferring;

                        replies.push(encode_file_ack(&key, entry.transferred_bytes, false)?);

                        if entry.transferred_bytes >= entry.file_size {
                            entry.status = TransferStatus::Completed;
                            let _ = fs::create_dir_all(&self.download_dir);
                            if let Some(saved) = write_unique_download(
                                &self.download_dir,
                                &entry.file_name,
                                &entry.buffer,
                            ) {
                                entry.saved_path = Some(saved.to_string_lossy().to_string());
                            }
                        }
                    }
                }
            }
            FileTransferPacket::Cancel { file_id } => {
                if let Some(key) = self.resolve_transfer_key(file_id) {
                    if let Some(entry) = self.transfers.get_mut(&key) {
                        entry.status = TransferStatus::Cancelled;
                    }
                }
            }
        }

        Ok(Some((pkt, replies)))
    }
}

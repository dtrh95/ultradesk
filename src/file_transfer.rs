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

pub const DEFAULT_FILE_CHUNK_SIZE: usize = 32_768;

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

/// Encodes a File Offer packet (Kind `13`, `SendFile_Up` at `0x747469`).
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
        let cmd = format!("|||13|||{}|||{}|||{}", file_id, file_name, file_size);
        let raw = encode_senddata_string(&cmd, false);
        encode_byte_array_fields(&[&raw])
    }
}

/// Encodes a File Accept packet (Kind `14`, `SaveUpload` at `0x757b2e`).
pub fn encode_file_accept(
    file_id: &str,
    accept_flag: i32,
    modern: bool,
) -> Result<Vec<u8>, String> {
    if modern {
        let flag_bytes = accept_flag.to_le_bytes();
        encode_byte_array_fields(&[&[PACKET_FILE_ACCEPT], file_id.as_bytes(), &flag_bytes])
    } else {
        let cmd = format!("|||14|||{}|||{}", file_id, accept_flag);
        let raw = encode_senddata_string(&cmd, false);
        encode_byte_array_fields(&[&raw])
    }
}

/// Encodes a File Chunk packet (Kind `15`) using the 2-field format (`["|||15|||<id>|||<offset>|||", chunk]`)
/// from `0x701a25` or the 4-field `Form1.ManipulatePacket` format (`[[15], id, offset_le, chunk]`) from `0x701c85`.
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
        let header = format!("|||15|||{}|||{}|||", file_id, offset);
        encode_byte_array_fields(&[header.as_bytes(), data])
    }
}

/// Encodes a File Ack / Next-Chunk packet (Kind `21` at `0x755722`).
pub fn encode_file_ack(file_id: &str, next_offset: u64, modern: bool) -> Result<Vec<u8>, String> {
    if modern {
        let offset_bytes = (next_offset as i64).to_le_bytes();
        encode_byte_array_fields(&[&[PACKET_FILE_ACK], file_id.as_bytes(), &offset_bytes])
    } else {
        let cmd = format!("|||21|||{}|||{}", file_id, next_offset);
        let raw = encode_senddata_string(&cmd, false);
        encode_byte_array_fields(&[&raw])
    }
}

/// Encodes a File Cancel packet (Kind `16`, `VCB_cancelUpload` at `0x753908`).
pub fn encode_file_cancel(file_id: &str, modern: bool) -> Result<Vec<u8>, String> {
    if modern {
        encode_byte_array_fields(&[&[PACKET_FILE_CANCEL], file_id.as_bytes()])
    } else {
        let cmd = format!("|||16|||{}", file_id);
        let raw = encode_senddata_string(&cmd, false);
        encode_byte_array_fields(&[&raw])
    }
}

/// Encodes a Chat Text Message packet (Kind `12` at `0x700e4d`).
pub fn encode_chat_message(text: &str, modern: bool) -> Result<Vec<u8>, String> {
    if modern {
        let utf16 = encode_senddata_string(text, true);
        encode_byte_array_fields(&[&[PACKET_CHAT_MESSAGE], &utf16])
    } else {
        let cmd = format!("|||12|||{}", text);
        let raw = encode_senddata_string(&cmd, false);
        encode_byte_array_fields(&[&raw])
    }
}

fn decode_int_le(bytes: &[u8]) -> u64 {
    match bytes.len() {
        8 => i64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])
        .max(0) as u64,
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
    if bytes.len() % 2 == 0 && (bytes.contains(&0) || std::str::from_utf8(bytes).is_err()) {
        return decode_utf16le_lossy(bytes);
    }
    String::from_utf8_lossy(bytes).to_string()
}

/// Decodes any inbound UltraViewer file transfer or chat packet (both modern byte-array and legacy `|||`).
pub fn decode_file_transfer_packet(payload: &[u8]) -> Option<FileTransferPacket> {
    if let Ok(fields) = decode_byte_array_fields(payload) {
        if !fields.is_empty() && fields[0].len() == 1 {
            let kind = fields[0][0];
            match kind {
                PACKET_CHAT_MESSAGE if fields.len() >= 2 => {
                    return Some(FileTransferPacket::ChatMessage {
                        text: decode_text_field(&fields[1]),
                    });
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
                        if v <= 1 { 0 } else { v }
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
                PACKET_FILE_ACK if fields.len() >= 3 => {
                    return Some(FileTransferPacket::Ack {
                        file_id: decode_text_field(&fields[1]),
                        next_offset: decode_int_le(&fields[2]),
                    });
                }
                _ => {}
            }
        }
        // Check 2-field chunk packet: ["|||15|||<file_id>|||<offset>|||", chunk_bytes]
        if fields.len() >= 2 && fields[0].starts_with(b"|||15|||") {
            let header = String::from_utf8_lossy(&fields[0]);
            let parts = parse_command_fields(&header);
            if parts.len() >= 3 {
                let file_id = parts[1].clone();
                let offset = parts[2].parse::<u64>().unwrap_or(0);
                return Some(FileTransferPacket::Chunk {
                    file_id,
                    offset,
                    data: fields[1].clone(),
                });
            }
        }
        if fields.len() == 1 && fields[0].starts_with(b"|||") {
            return parse_legacy_file_command(&String::from_utf8_lossy(&fields[0]));
        }
    }
    if payload.starts_with(b"|||") {
        return parse_legacy_file_command(&String::from_utf8_lossy(payload));
    }
    None
}

fn parse_legacy_file_command(text: &str) -> Option<FileTransferPacket> {
    let parts = parse_command_fields(text);
    let cmd = parts.first()?.as_str();
    match cmd {
        "12" if parts.len() >= 2 => Some(FileTransferPacket::ChatMessage {
            text: parts[1..].join("|||"),
        }),
        "13" if parts.len() >= 4 => Some(FileTransferPacket::Offer {
            file_id: parts[1].clone(),
            file_name: sanitize_file_name(&parts[2]),
            file_size: parts[3].parse::<u64>().unwrap_or(0),
        }),
        "14" if parts.len() >= 2 => {
            let flag = parts.get(2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
            let resume = if flag <= 1 { 0 } else { flag };
            Some(FileTransferPacket::Accept {
                file_id: parts[1].clone(),
                resume_offset: resume,
            })
        }
        "16" if parts.len() >= 2 => Some(FileTransferPacket::Cancel {
            file_id: parts[1].clone(),
        }),
        "21" if parts.len() >= 3 => Some(FileTransferPacket::Ack {
            file_id: parts[1].clone(),
            next_offset: parts[2].parse::<u64>().unwrap_or(0),
        }),
        _ => None,
    }
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
            .map(|h| PathBuf::from(h).join("Downloads").join("UltraViewer"))
            .unwrap_or_else(|_| std::env::temp_dir().join("ultraviewer-downloads"));
        Self::new(default_dir)
    }
}

impl FileTransferManager {
    pub fn new(download_dir: PathBuf) -> Self {
        Self {
            transfers: BTreeMap::new(),
            chat_messages: Vec::new(),
            download_dir,
            auto_accept: true,
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
                buffer: data,
            },
        );
        Ok((file_id, packet))
    }

    pub fn offer_file_from_path(&mut self, path: &Path) -> Result<(String, Vec<u8>), String> {
        let data = fs::read(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
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

    /// Generates up to `max_chunks` outbound chunk packets for an active upload (`Send` transfer),
    /// matching `uv_unpacked.exe` at `0x70186a` which sends 2 chunks upon accept and streams on ack.
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
        for _ in 0..max_chunks.max(1) {
            let offset = entry.transferred_bytes as usize;
            if offset >= entry.buffer.len() {
                entry.status = TransferStatus::Completed;
                break;
            }
            let end = (offset + chunk_sz).min(entry.buffer.len());
            let slice = &entry.buffer[offset..end];
            // Send 2-field format ("|||15|||<id>|||<offset>|||", data) as produced by 0x701a25
            let pkt = encode_file_chunk(file_id, offset as u64, slice, false)?;
            packets.push(pkt);
            entry.transferred_bytes = end as u64;
            if entry.transferred_bytes >= entry.file_size {
                entry.status = TransferStatus::Completed;
                break;
            }
        }
        Ok(packets)
    }

    /// Processes an inbound file transfer packet and returns any automatic reply packets to send.
    pub fn handle_packet(&mut self, payload: &[u8]) -> Result<Option<(FileTransferPacket, Vec<Vec<u8>>)>, String> {
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
                        buffer: Vec::with_capacity((*file_size as usize).min(16 * 1024 * 1024)),
                    },
                );
            }
            FileTransferPacket::Accept {
                file_id,
                resume_offset,
            } => {
                if self.transfers.contains_key(file_id) {
                    let chunks = self.pop_outbound_chunks(file_id, Some(*resume_offset), 2)?;
                    replies.extend(chunks);
                }
            }
            FileTransferPacket::Ack {
                file_id,
                next_offset,
            } => {
                if self.transfers.contains_key(file_id) {
                    let chunks = self.pop_outbound_chunks(file_id, Some(*next_offset), 2)?;
                    replies.extend(chunks);
                }
            }
            FileTransferPacket::Chunk {
                file_id,
                offset,
                data,
            } => {
                if let Some(entry) = self.transfers.get_mut(file_id) {
                    let off = *offset as usize;
                    let needed = off + data.len();
                    if entry.buffer.len() < needed {
                        entry.buffer.resize(needed, 0);
                    }
                    entry.buffer[off..needed].copy_from_slice(data);
                    entry.transferred_bytes = entry.buffer.len() as u64;
                    entry.status = TransferStatus::Transferring;

                    if entry.transferred_bytes >= entry.file_size {
                        entry.status = TransferStatus::Completed;
                        let _ = fs::create_dir_all(&self.download_dir);
                        let target = self.download_dir.join(&entry.file_name);
                        if fs::write(&target, &entry.buffer).is_ok() {
                            entry.saved_path = Some(target.to_string_lossy().to_string());
                        }
                    } else {
                        replies.push(encode_file_ack(
                            file_id,
                            entry.transferred_bytes,
                            self.modern_protocol,
                        )?);
                    }
                }
            }
            FileTransferPacket::Cancel { file_id } => {
                if let Some(entry) = self.transfers.get_mut(file_id) {
                    entry.status = TransferStatus::Cancelled;
                }
            }
        }

        Ok(Some((pkt, replies)))
    }
}

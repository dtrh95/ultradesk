use crate::protocol::{decode_byte_array_fields, encode_byte_array_fields, encode_senddata_string};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u16 = 2;
pub const AUDIO_BITS_PER_SAMPLE: u16 = 16;

pub const PACKET_PC_SILENCE: u8 = 42;
pub const PACKET_PC_G722: u8 = 43;
pub const PACKET_MIC_SILENCE: u8 = 50;
pub const PACKET_MIC_G722: u8 = 51;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormReceiveResult {
    pub fields: Option<Vec<Vec<u8>>>,
    pub event_id: Option<i32>,
    pub event_arguments: Option<(String, u16, Option<Vec<Vec<u8>>>, Vec<u8>, bool)>,
    pub audio_operation: Option<String>,
    pub parse_error: Option<String>,
    pub audio_error: Option<String>,
}

pub fn process_form1_packet<F>(
    payload: &[u8],
    source: &str,
    source_port: u16,
    unicode_bytes: bool,
    playback_enabled: bool,
    max_silence_bytes: isize,
    mut audio_sink: Option<F>,
) -> Result<FormReceiveResult, String>
where
    F: FnMut(&str, &[u8]) -> Result<(), String>,
{
    if max_silence_bytes < 0 {
        return Err("negative silence allocation limit".to_string());
    }
    let raw = payload.to_vec();
    let fields = match decode_byte_array_fields(&raw) {
        Ok(f) => f,
        Err(_) => {
            return Ok(FormReceiveResult {
                fields: None,
                event_id: Some(9),
                event_arguments: Some((source.to_string(), source_port, None, raw, unicode_bytes)),
                audio_operation: None,
                parse_error: Some("ValueError".to_string()),
                audio_error: None,
            })
        }
    };
    if fields.is_empty() || fields[0].is_empty() {
        return Err("packet has no discriminator byte".to_string());
    }
    let kind = fields[0][0];
    let operation = match kind {
        42 => Some("Play16BitAudio"),
        43 => Some("PlayEncodedAudio"),
        50 => Some("Play16BitMicrophoneAudio"),
        51 => Some("PlayMicrophoneEncodedAudio"),
        _ => None,
    };
    let attempted = operation.is_some() && (kind >= 50 || playback_enabled);
    let mut audio_error = None;
    if attempted {
        let op_name = operation.unwrap();
        let step_res: Result<(), String> = (|| {
            let body = fields
                .get(1)
                .ok_or_else(|| "IndexError".to_string())?;
            let audio_bytes: Vec<u8> = if kind == 42 || kind == 50 {
                if body.len() < 4 {
                    return Err("ValueError".to_string());
                }
                let size = i32::from_le_bytes([body[0], body[1], body[2], body[3]]) as isize;
                if size < 0 || size > max_silence_bytes {
                    return Err("ValueError".to_string());
                }
                vec![0u8; size as usize]
            } else {
                body.clone()
            };
            if let Some(ref mut sink) = audio_sink {
                sink(op_name, &audio_bytes)?;
            }
            Ok(())
        })();
        if let Err(err_kind) = step_res {
            audio_error = Some(err_kind);
        }
    }
    let consumed = kind == 42 || kind == 43;
    let event_arguments = if consumed {
        None
    } else {
        Some((
            source.to_string(),
            source_port,
            Some(fields.clone()),
            raw,
            unicode_bytes,
        ))
    };
    Ok(FormReceiveResult {
        fields: Some(fields),
        event_id: if consumed { None } else { Some(9) },
        event_arguments,
        audio_operation: if attempted {
            operation.map(|s| s.to_string())
        } else {
            None
        },
        parse_error: None,
        audio_error,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaveSource {
    PcSound,
    Microphone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodedAudioPacket {
    pub kind: u8,
    pub source: WaveSource,
    pub is_silence: bool,
    pub pcm_s16le: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct G722Band {
    pub s: i32,
    pub sp: i32,
    pub sz: i32,
    pub r: [i32; 3],
    pub a: [i32; 3],
    pub ap: [i32; 3],
    pub p: [i32; 3],
    pub d: [i32; 7],
    pub b: [i32; 7],
    pub bp: [i32; 7],
    pub sg: [i32; 7],
    pub nb: i32,
    pub det: i32,
}

#[derive(Debug, Clone)]
pub struct G722CodecState {
    pub band: [G722Band; 2],
    pub x: [i32; 24],
}

impl Default for G722CodecState {
    fn default() -> Self {
        Self::new()
    }
}

const WL: [i32; 8] = [-60, -30, 58, 172, 334, 538, 1198, 3042];
const RL42: [i32; 16] = [0, 7, 6, 5, 4, 3, 2, 1, 7, 6, 5, 4, 3, 2, 1, 0];
const ILB: [i32; 32] = [
    2048, 2093, 2139, 2186, 2233, 2282, 2332, 2383, 2435, 2489, 2543, 2599, 2656, 2714, 2774, 2834,
    2896, 2960, 3025, 3091, 3158, 3228, 3298, 3371, 3444, 3520, 3597, 3676, 3756, 3838, 3922, 4008,
];
const QM4: [i32; 16] = [
    0, -20456, -12896, -8968, -6288, -4240, -2584, -1200, 20456, 12896, 8968, 6288, 4240, 2584,
    1200, 0,
];
const QM6: [i32; 64] = [
    -136, -136, -136, -136, -24808, -21904, -19008, -16704, -14984, -13512, -12280, -11192, -10232,
    -9360, -8576, -7856, -7192, -6576, -6000, -5456, -4944, -4464, -4008, -3576, -3168, -2776,
    -2400, -2032, -1688, -1360, -1040, -728, 24808, 21904, 19008, 16704, 14984, 13512, 12280,
    11192, 10232, 9360, 8576, 7856, 7192, 6576, 6000, 5456, 4944, 4464, 4008, 3576, 3168, 2776,
    2400, 2032, 1688, 1360, 1040, 728, 432, 136, -432, -136,
];
const Q6: [i32; 32] = [
    0, 35, 72, 110, 150, 190, 233, 276, 323, 370, 422, 473, 530, 587, 650, 714, 786, 858, 940,
    1023, 1121, 1219, 1339, 1458, 1612, 1765, 1980, 2195, 2557, 2919, 0, 0,
];
const ILN: [i32; 32] = [
    0, 63, 62, 31, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14, 13, 12, 11,
    10, 9, 8, 7, 6, 5, 4, 0,
];
const ILP: [i32; 32] = [
    0, 61, 60, 59, 58, 57, 56, 55, 54, 53, 52, 51, 50, 49, 48, 47, 46, 45, 44, 43, 42, 41, 40, 39,
    38, 37, 36, 35, 34, 33, 32, 0,
];
const WH: [i32; 3] = [0, -214, 798];
const RH2: [i32; 4] = [2, 1, 2, 1];
const QM2: [i32; 4] = [-7408, -1616, 7408, 1616];
const QMF_COEFFS: [i32; 12] = [3, -11, 12, 32, -210, 951, 3876, -805, 362, -156, 53, -11];

#[inline]
fn saturate(amp: i32) -> i32 {
    amp.clamp(i16::MIN as i32, i16::MAX as i32)
}

#[inline]
fn scale_det(nb: i32, shift_base: i32) -> i32 {
    let wd1 = ((nb >> 6) & 31) as usize;
    let wd2 = shift_base - (nb >> 11);
    if wd2 < 0 {
        ILB[wd1] << (-wd2)
    } else {
        ILB[wd1] >> wd2
    }
}

fn block4(state: &mut G722CodecState, band_idx: usize, d: i32) {
    let b = &mut state.band[band_idx];
    b.d[0] = d;
    b.r[0] = saturate(b.s + d);
    for i in 0..2 {
        b.p[i] = saturate(b.sz + d);
    }
    for i in (1..=6).rev() {
        b.d[i] = b.d[i - 1];
    }
    b.sz = 0;
    for i in 1..=6 {
        b.sg[i] = b.d[i] >> 15;
        b.bp[i] = saturate(
            b.b[i] - (b.b[i] >> 8) + (if (b.sg[0] ^ b.sg[i]) == 0 { 128 } else { -128 }),
        );
        b.sz = saturate(b.sz + ((b.bp[i] * b.d[i]) >> 14));
    }
    b.sg[0] = b.p[0] >> 15;
    b.sg[1] = b.p[1] >> 15;
    b.sg[2] = b.p[2] >> 15;

    b.ap[1] = saturate(
        b.a[1] - (b.a[1] >> 8) + (if (b.sg[0] ^ b.sg[1]) == 0 { 192 } else { -192 }),
    );
    let a2p = if (b.sg[0] ^ b.sg[2]) == 0 { 128 } else { -128 }
        - if (b.sg[1] ^ b.sg[2]) == 0 {
            b.a[1] >> 7
        } else {
            -(b.a[1] >> 7)
        };
    b.ap[2] = saturate(b.a[2] - (b.a[2] >> 7) + a2p).clamp(-12288, 12288);

    let a1_limit = 15360 - b.ap[2];
    b.ap[1] = b.ap[1].clamp(-a1_limit, a1_limit);

    b.sp = 0;
    for i in 1..=2 {
        b.sp = saturate(b.sp + ((b.ap[i] * b.r[i - 1]) >> 14));
    }
    b.s = saturate(b.sp + b.sz);

    for i in 1..=6 {
        b.b[i] = b.bp[i];
    }
    for i in 1..=2 {
        b.a[i] = b.ap[i];
    }
    for i in (1..=2).rev() {
        b.r[i] = b.r[i - 1];
        b.p[i] = b.p[i - 1];
    }
}

impl G722CodecState {
    pub fn new() -> Self {
        let mut s = Self {
            band: [G722Band::default(), G722Band::default()],
            x: [0; 24],
        };
        s.band[0].det = 32;
        s.band[1].det = 8;
        s
    }

    pub fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        let mut out = Vec::with_capacity(pcm.len() / 2);
        let mut j = 0usize;
        while j + 1 < pcm.len() {
            let xin0 = (pcm[j] as i32) >> 1;
            let xin1 = (pcm[j + 1] as i32) >> 1;
            j += 2;

            for i in (2..24).rev() {
                self.x[i] = self.x[i - 2];
            }
            self.x[1] = xin0;
            self.x[0] = xin1;

            let mut sum_even = 0i32;
            let mut sum_odd = 0i32;
            for i in 0..12 {
                sum_even += self.x[2 * i] * QMF_COEFFS[i];
                sum_odd += self.x[2 * i + 1] * QMF_COEFFS[11 - i];
            }
            let xl = saturate((sum_even + sum_odd) >> 14);
            let xh = saturate((sum_even - sum_odd) >> 14);

            // Lower sub-band
            let el = saturate(xl - self.band[0].s);
            let wd = if el >= 0 { el } else { !el };
            let mut mil = 1usize;
            while mil < 30 && wd >= ((Q6[mil] * self.band[0].det) >> 12) {
                mil += 1;
            }
            let il = (if el < 0 { ILN[mil] } else { ILP[mil] }) as usize;
            let dlt = (self.band[0].det * QM4[il >> 2]) >> 15;
            let nb_l = ((self.band[0].nb * 127) >> 7) + WL[RL42[il >> 2] as usize];
            self.band[0].nb = nb_l.clamp(0, 18432);
            self.band[0].det = scale_det(self.band[0].nb, 6);
            block4(self, 0, dlt);

            // Upper sub-band
            let eh = saturate(xh - self.band[1].s);
            let wd_h = if eh >= 0 { eh } else { !eh };
            let mih = if wd_h >= ((564 * self.band[1].det) >> 12) {
                2
            } else {
                1
            };
            let ih = (if eh < 0 { mih - 1 } else { 4 - mih }) as usize;
            let dh = (self.band[1].det * QM2[ih]) >> 15;
            let nb_h = ((self.band[1].nb * 127) >> 7) + WH[RH2[ih] as usize];
            self.band[1].nb = nb_h.clamp(0, 22528);
            self.band[1].det = scale_det(self.band[1].nb, 8);
            block4(self, 1, dh);

            out.push((((ih << 6) | il) & 0xFF) as u8);
        }
        out
    }

    pub fn decode(&mut self, data: &[u8]) -> Vec<i16> {
        let mut out = Vec::with_capacity(data.len() * 2);
        for &code in data {
            let ilr = (code & 0x3F) as usize;
            let ih = ((code >> 6) & 0x03) as usize;

            let rlt = (self.band[0].det * QM6[ilr]) >> 15;
            let rlt_reconstructed = saturate(self.band[0].s + rlt);
            let dlt = (self.band[0].det * QM4[ilr >> 2]) >> 15;
            let nb_l = ((self.band[0].nb * 127) >> 7) + WL[RL42[ilr >> 2] as usize];
            self.band[0].nb = nb_l.clamp(0, 18432);
            self.band[0].det = scale_det(self.band[0].nb, 6);
            block4(self, 0, dlt);

            let dh = (self.band[1].det * QM2[ih]) >> 15;
            let rh_reconstructed = saturate(self.band[1].s + dh);
            let nb_h = ((self.band[1].nb * 127) >> 7) + WH[RH2[ih] as usize];
            self.band[1].nb = nb_h.clamp(0, 22528);
            self.band[1].det = scale_det(self.band[1].nb, 8);
            block4(self, 1, dh);

            for i in (2..24).rev() {
                self.x[i] = self.x[i - 2];
            }
            self.x[1] = rlt_reconstructed + rh_reconstructed;
            self.x[0] = rlt_reconstructed - rh_reconstructed;

            let mut sum_even = 0i32;
            let mut sum_odd = 0i32;
            for i in 0..12 {
                sum_even += self.x[2 * i] * QMF_COEFFS[i];
                sum_odd += self.x[2 * i + 1] * QMF_COEFFS[11 - i];
            }
            let s0 = saturate((sum_even << 1) >> 14) as i16;
            let s1 = saturate((sum_odd << 1) >> 14) as i16;
            out.push(s0);
            out.push(s1);
        }
        out
    }

    pub fn decode_to_bytes(&mut self, data: &[u8]) -> Vec<u8> {
        let samples = self.decode(data);
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        bytes
    }
}

pub fn encode_sound_toggle_command(enable: bool) -> Vec<u8> {
    let text = if enable { "31|||" } else { "32|||" };
    encode_senddata_string(text, false)
}

pub fn encode_audio_packet(
    source: WaveSource,
    g722_or_silence: &[u8],
    is_silence: bool,
) -> Result<Vec<u8>, String> {
    let kind: u8 = match (source, is_silence) {
        (WaveSource::PcSound, true) => PACKET_PC_SILENCE,
        (WaveSource::PcSound, false) => PACKET_PC_G722,
        (WaveSource::Microphone, true) => PACKET_MIC_SILENCE,
        (WaveSource::Microphone, false) => PACKET_MIC_G722,
    };
    encode_byte_array_fields(&[&[kind], g722_or_silence])
}

pub fn decode_audio_packet(
    payload: &[u8],
    pc_decoder: &mut G722CodecState,
    mic_decoder: &mut G722CodecState,
) -> Result<Option<DecodedAudioPacket>, String> {
    let fields = match decode_byte_array_fields(payload) {
        Ok(f) if f.len() >= 2 && f[0].len() == 1 => f,
        _ => return Ok(None),
    };
    let kind = fields[0][0];
    let body = &fields[1];

    match kind {
        PACKET_PC_SILENCE => {
            if body.len() < 4 {
                return Err("Audio packet 42 requires a 4-byte silence length".to_string());
            }
            let len = i32::from_le_bytes([body[0], body[1], body[2], body[3]]).max(0) as usize;
            let capped = len.min(8 * 1024 * 1024);
            Ok(Some(DecodedAudioPacket {
                kind,
                source: WaveSource::PcSound,
                is_silence: true,
                pcm_s16le: vec![0u8; capped],
            }))
        }
        PACKET_PC_G722 => {
            let pcm = pc_decoder.decode_to_bytes(body);
            Ok(Some(DecodedAudioPacket {
                kind,
                source: WaveSource::PcSound,
                is_silence: false,
                pcm_s16le: pcm,
            }))
        }
        PACKET_MIC_SILENCE => {
            if body.len() < 4 {
                return Err("Audio packet 50 requires a 4-byte silence length".to_string());
            }
            let len = i32::from_le_bytes([body[0], body[1], body[2], body[3]]).max(0) as usize;
            let capped = len.min(8 * 1024 * 1024);
            Ok(Some(DecodedAudioPacket {
                kind,
                source: WaveSource::Microphone,
                is_silence: true,
                pcm_s16le: vec![0u8; capped],
            }))
        }
        PACKET_MIC_G722 => {
            let pcm = mic_decoder.decode_to_bytes(body);
            Ok(Some(DecodedAudioPacket {
                kind,
                source: WaveSource::Microphone,
                is_silence: false,
                pcm_s16le: pcm,
            }))
        }
        _ => Ok(None),
    }
}

pub fn build_wav_bytes(
    pcm_s16le: &[u8],
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
) -> Vec<u8> {
    let byte_rate = sample_rate * (channels as u32) * (bits_per_sample as u32 / 8);
    let block_align = channels * (bits_per_sample / 8);
    let data_len = pcm_s16le.len() as u32;
    let riff_size = 36 + data_len;

    let mut out = Vec::with_capacity(44 + pcm_s16le.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm_s16le);
    out
}

#[derive(Debug, Clone)]
pub struct AudioBuffer {
    pub enabled: bool,
    pub chunks_received: u64,
    pub silence_chunks_received: u64,
    pub bytes_decoded: u64,
    pub max_chunks: usize,
    chunks: VecDeque<(u64, Vec<u8>)>,
    pc_decoder: G722CodecState,
    mic_decoder: G722CodecState,
}

impl Default for AudioBuffer {
    fn default() -> Self {
        Self::new(64)
    }
}

impl AudioBuffer {
    pub fn new(max_chunks: usize) -> Self {
        Self {
            enabled: true,
            chunks_received: 0,
            silence_chunks_received: 0,
            bytes_decoded: 0,
            max_chunks: max_chunks.max(8),
            chunks: VecDeque::new(),
            pc_decoder: G722CodecState::new(),
            mic_decoder: G722CodecState::new(),
        }
    }

    pub fn ingest_packet(&mut self, payload: &[u8]) -> Result<Option<DecodedAudioPacket>, String> {
        let decoded = decode_audio_packet(payload, &mut self.pc_decoder, &mut self.mic_decoder)?;
        if let Some(ref pkt) = decoded {
            self.chunks_received += 1;
            if pkt.is_silence {
                self.silence_chunks_received += 1;
            }
            self.bytes_decoded += pkt.pcm_s16le.len() as u64;
            let seq = self.chunks_received;
            if self.chunks.len() >= self.max_chunks {
                self.chunks.pop_front();
            }
            self.chunks.push_back((seq, pkt.pcm_s16le.clone()));
        }
        Ok(decoded)
    }

    pub fn wav_since(&self, after_seq: u64) -> (u64, Vec<u8>) {
        let mut combined = Vec::new();
        let mut max_seq = after_seq;
        for (seq, pcm) in &self.chunks {
            if *seq > after_seq {
                combined.extend_from_slice(pcm);
                if *seq > max_seq {
                    max_seq = *seq;
                }
            }
        }
        let wav = build_wav_bytes(
            &combined,
            AUDIO_SAMPLE_RATE,
            AUDIO_CHANNELS,
            AUDIO_BITS_PER_SAMPLE,
        );
        (max_seq, wav)
    }
}

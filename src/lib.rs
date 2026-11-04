pub mod audio;
pub mod clipboard;
pub mod file_transfer;
pub mod input;
pub mod protocol;
pub mod screen;
pub mod server;
pub mod session;

#[cfg(test)]
mod tests {
    use crate::audio::{
        build_wav_bytes, decode_audio_packet, encode_audio_packet, encode_sound_toggle_command,
        process_form1_packet, AudioBuffer, G722CodecState, WaveSource, PACKET_MIC_G722,
        PACKET_MIC_SILENCE, PACKET_PC_G722, PACKET_PC_SILENCE,
    };
    use crate::clipboard::{
        decode_clipboard_packet, encode_clipboard_modern_packet, encode_clipboard_packet,
    };
    use crate::file_transfer::{
        decode_file_transfer_packet, encode_chat_message, encode_file_accept, encode_file_ack,
        encode_file_cancel, encode_file_chunk, encode_file_offer, FileTransferManager,
        FileTransferPacket, TransferDirection, TransferStatus,
    };
    use crate::input::{
        keyboard, mouse_button, mouse_move, mouse_wheel, with_held_key, InputEvent,
    };
    use crate::protocol::{
        atlas_layout, build_connection_request_command, build_login_with_password,
        build_registration_command, connection_request_password_value,
        decode_byte_array_fields_checked, decode_p2p_frame_body, decode_target_route,
        decode_transport_body, decode_utf16le_lossy, decrypt_p2p_aes_body,
        decrypt_registration_bytes, derive_p2p_aes_key, derive_vfunction_key_bytes, dib_stride,
        encode_byte_array_fields, encode_p2p_frame, encode_relay_frame, encode_transport_frame,
        encrypt_p2p_aes_body, encrypt_registration_bytes, export_cache, extract_command3_identity,
        extract_tile, http_service_preface, import_cache, login_password_fields, pack_atlas,
        restore_atlas, tile_sample_score, CachedItem, FrameStream, REGISTRATION_PASSWORD,
    };
    use crate::screen::{
        apply_rgb555_update, compress_rgb555_with_dict, desktop_size_from_login, RGB555Screen,
        RgbBuffer, ZstdDictionaryDecoder,
    };
    use crate::server::{decode_base64, encode_base64, PortalServer};
    use crate::session::{looks_like_tls_record, ControlSession};
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageEncoder, ImageFormat};
    use std::io::{Read, Write};
    use std::net::TcpStream;

    #[test]
    fn test_reference_tiles_atlas_cache_and_transport() {
        let stride = dib_stride(3, 16).unwrap();
        assert_eq!(stride, 8);
        let mut frame = vec![0u8; stride * 3];
        for y in 0..3 {
            for x in 0..3 {
                let offset = y * stride + x * 2;
                let val = ((y * 10 + x + 1) as u16).to_le_bytes();
                frame[offset] = val[0];
                frame[offset + 1] = val[1];
            }
        }
        let tile = extract_tile(&frame, 2, stride as isize, 1, 1, 2, 2, 3).unwrap();
        assert_eq!(tile.len(), 8);

        let stride24 = dib_stride(4, 24).unwrap();
        let mut frame24 = vec![0u8; stride24 * 4];
        for y in 0..4 {
            for x in 0..4 {
                let offset = y * stride24 + x * 3;
                frame24[offset] = (x + 1) as u8;
                frame24[offset + 1] = (y + 1) as u8;
                frame24[offset + 2] = (x * 10 + y) as u8;
            }
        }
        let mut atlas = vec![0u8; 4 * 4 * 3];
        let mut restored_frame = vec![0u8; stride24 * 4];
        pack_atlas(&frame24, &mut atlas, 3, 12, stride24 as isize, 1, 1, 0, 0, 2, 2, 4).unwrap();
        restore_atlas(
            &atlas,
            &mut restored_frame,
            3,
            stride24 as isize,
            12,
            0,
            0,
            1,
            1,
            2,
            2,
            4,
        )
        .unwrap();
        for row in 0..2 {
            let src_row = (4 - 1 - 2) + row;
            let off = src_row * stride24 + 3;
            assert_eq!(&restored_frame[off..off + 6], &frame24[off..off + 6]);
        }

        let items = vec![
            (
                10,
                CachedItem {
                    width: 2,
                    height: 2,
                    rect_hash: 99,
                    data: b"abc".to_vec(),
                },
            ),
            (
                20,
                CachedItem {
                    width: 4,
                    height: 4,
                    rect_hash: 120,
                    data: b"defg".to_vec(),
                },
            ),
        ];
        let blob = export_cache(&items);
        let restored = import_cache(&blob).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.get(&10).unwrap().data, b"abc");

        let layout = atlas_layout(&[(0, 0, 40, 20)], 32, 64).unwrap();
        assert_eq!(layout.len(), 2);
        assert_eq!(layout[0], (0, 0, 32, 20, 0, 0));
        assert_eq!(layout[1], (32, 0, 8, 20, 0, 20));

        let sample_buf = vec![0u8; 32 * 4];
        let (diff, total) = tile_sample_score(&sample_buf, 5, 3, 16).unwrap();
        assert_eq!((diff, total), (0, 2));

        let preface = http_service_preface("example.com:443", true);
        assert!(preface.starts_with(b"POST /UVService/ HTTP/1.1\r\n"));

        let mut stream = FrameStream::new(false);
        let f1 = encode_transport_frame(b"\x01abc");
        let f2 = encode_relay_frame(b"xyz", "1.2.3.4", 5678, 1).unwrap();
        let wire = [f1.as_slice(), f2.as_slice()].concat();
        assert!(stream.feed(&wire[..3]).is_empty());
        let bodies = stream.feed(&wire[3..]);
        assert_eq!(bodies.len(), 2);
        let d0 = decode_transport_body(&bodies[0]).unwrap().unwrap();
        assert_eq!(d0.kind, "direct");
        assert_eq!(d0.data, b"abc");
        let d1 = decode_transport_body(&bodies[1]).unwrap().unwrap();
        assert_eq!(d1.kind, "relay");
        assert_eq!(d1.port, 5678);
        assert!(d1.flag);
        assert_eq!(d1.data, b"xyz");
    }

    #[test]
    fn test_registration_crypto_and_target_route() {
        assert_eq!(
            derive_vfunction_key_bytes(REGISTRATION_PASSWORD, 24),
            hex_to_bytes("409a194eaa8b6a12b95a60486d47c650aa81bc8900000000")
        );
        let sample = b"3:hash:pass:0:192.0.2.10:0:b64:6.6.133:AA:_:0|6|2|8|0|0:0:::0:en:u:0:0:1:0:1:0:0:0:::";
        let encrypted = encrypt_registration_bytes(sample, REGISTRATION_PASSWORD);
        assert_eq!(
            decrypt_registration_bytes(&encrypted, REGISTRATION_PASSWORD).unwrap(),
            sample
        );

        let identity =
            extract_command3_identity(b"3:50000:75234077:198.51.100.9:6.6.133:0:").unwrap();
        assert_eq!(identity.public_source_port, 50000);
        assert_eq!(identity.client_id, 75234077);
        assert_eq!(identity.public_ip, "198.51.100.9");

        let route = decode_target_route(
            b"8:1:192.168.1.10:0:203.0.113.10:55000:55001:55002:hostB64:2112:0:198.51.100.22:0:1:functions3.ultraviewer.net:0:0:0:0:0:token-xyz:",
        )
        .unwrap();
        assert_eq!(route.lan_ip, "192.168.1.10");
        assert_eq!(route.public_ip, "203.0.113.10");
        assert_eq!(route.tcp_port, 2112);
        assert_eq!(route.tcp_server_ip, "198.51.100.22");
        assert_eq!(route.partner_secret_token.as_deref(), Some("token-xyz"));
        assert_eq!(route.tcp_server_hostname, "functions3.ultraviewer.net");

        let mut reg_terms: Vec<String> = (0..26).map(|i| format!("t{}", i)).collect();
        reg_terms[0] = "3".to_string();
        reg_terms[3] = "0".to_string();
        reg_terms[5] = "0".to_string();
        reg_terms[12] = String::new();
        reg_terms[22] = "1:2:3".to_string();
        assert!(build_registration_command(&reg_terms).is_ok());

        let mut req_terms: Vec<String> = (0..17).map(|i| format!("r{}", i)).collect();
        req_terms[0] = "8".to_string();
        assert!(build_connection_request_command(&req_terms).is_ok());
    }

    #[test]
    fn test_login_password_and_p2p_wire() {
        assert_eq!(
            connection_request_password_value("test-password", 0),
            "8d9b56d443454f7f8b2b31912d142693"
        );
        let auth = login_password_fields("test-password", 0, Some("12345678")).unwrap();
        assert_eq!(auth.salt, "12345678");
        assert_eq!(auth.primary, "63867665cb0fd85445db63a1b2bae1a3");
        assert_eq!(auth.secondary, "8d9b56d443454f7f8b2b31912d142693");

        let fields: Vec<String> = (0..22).map(|i| format!("metadata{}", i)).collect();
        let login_pkt =
            build_login_with_password(&fields, "test-password", 0, Some("12345678")).unwrap();
        let login_str = decode_utf16le_lossy(&login_pkt);
        assert!(login_str.starts_with("login:12345678:63867665cb0fd85445db63a1b2bae1a3:"));

        let aes_key = derive_p2p_aes_key("token").unwrap();
        assert_eq!(aes_key[31], 0);
        let iv = [0u8; 16];
        let p2p_plain = b"|||12|||hello\x00world";
        let p2p_cipher = encrypt_p2p_aes_body(p2p_plain, "secret", &iv).unwrap();
        assert_eq!(
            decrypt_p2p_aes_body(&p2p_cipher, "secret").unwrap(),
            p2p_plain
        );

        let raw_frame = encode_p2p_frame(p2p_plain, true, None, None).unwrap();
        let (dec_raw, flag_raw) = decode_p2p_frame_body(&raw_frame[4..], None).unwrap();
        assert!(flag_raw);
        assert_eq!(dec_raw, p2p_plain);

        let enc_frame = encode_p2p_frame(p2p_plain, false, Some("secret"), Some(&iv)).unwrap();
        let (dec_enc, flag_enc) =
            decode_p2p_frame_body(&enc_frame[4..], Some("secret")).unwrap();
        assert!(!flag_enc);
        assert_eq!(dec_enc, p2p_plain);
    }

    #[test]
    fn test_byte_array_fields_and_receive_pipeline() {
        let fields = [b"\x01".as_slice(), b"abc".as_slice(), b"".as_slice()];
        let raw = encode_byte_array_fields(&fields).unwrap();
        assert_eq!(
            decode_byte_array_fields_checked(&raw, Some(3)).unwrap(),
            vec![b"\x01".to_vec(), b"abc".to_vec(), b"".to_vec()]
        );

        let mut seen = Vec::new();
        let pkt42 = encode_byte_array_fields(&[&[42u8], &12i32.to_le_bytes()]).unwrap();
        let res42 = process_form1_packet(
            &pkt42,
            "peer",
            2112,
            true,
            true,
            8 * 1024 * 1024,
            Some(|op: &str, data: &[u8]| -> Result<(), String> {
                seen.push((op.to_string(), data.to_vec()));
                Ok(())
            }),
        )
        .unwrap();
        assert_eq!(res42.event_id, None);
        assert_eq!(
            seen,
            vec![("Play16BitAudio".to_string(), vec![0u8; 12])]
        );

        seen.clear();
        let pkt51 = encode_byte_array_fields(&[&[51u8], b"\xaa\xbb"]).unwrap();
        let res51 = process_form1_packet(
            &pkt51,
            "peer",
            2112,
            false,
            false,
            8 * 1024 * 1024,
            Some(|op: &str, data: &[u8]| -> Result<(), String> {
                seen.push((op.to_string(), data.to_vec()));
                Ok(())
            }),
        )
        .unwrap();
        assert_eq!(res51.event_id, Some(9));
        assert_eq!(
            seen,
            vec![(
                "PlayMicrophoneEncodedAudio".to_string(),
                b"\xaa\xbb".to_vec()
            )]
        );
    }

    #[test]
    fn test_screen_decode_11_field_rgb555_jpeg_scroll_and_lru() {
        let mut raw_tile = Vec::new();
        raw_tile.extend_from_slice(&0x7c00u16.to_le_bytes());
        raw_tile.extend_from_slice(&0x03e0u16.to_le_bytes());
        raw_tile.extend_from_slice(&0x001fu16.to_le_bytes());
        raw_tile.extend_from_slice(&0x7fffu16.to_le_bytes());
        let zstd_tile = compress_rgb555_with_dict(&raw_tile, 1).unwrap();

        let rect_xywh = |x: i16, y: i16, w: i16, h: i16| -> Vec<u8> {
            let mut b = Vec::with_capacity(8);
            b.extend_from_slice(&x.to_le_bytes());
            b.extend_from_slice(&y.to_le_bytes());
            b.extend_from_slice(&w.to_le_bytes());
            b.extend_from_slice(&h.to_le_bytes());
            b
        };

        let mut cache_add = rect_xywh(0, 0, 2, 2);
        cache_add.extend_from_slice(&77i32.to_le_bytes());

        let fields: Vec<Vec<u8>> = vec![
            vec![1u8],
            rect_xywh(0, 0, 2, 2),
            zstd_tile,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            cache_add,
        ];

        let mut screen = RGB555Screen::new((4, 4), ZstdDictionaryDecoder::default(), 100, 32);
        assert_eq!(screen.apply(&fields).unwrap(), 1);
        assert_eq!(screen.canvas.get_pixel(0, 0), (255, 0, 0));
        assert_eq!(screen.canvas.get_pixel(1, 0), (0, 255, 0));
        assert_eq!(screen.canvas.get_pixel(0, 1), (0, 0, 255));
        assert_eq!(screen.canvas.get_pixel(1, 1), (255, 255, 255));
        assert_eq!(screen.cache_keys(), vec![77]);

        let mut cache_ref = Vec::new();
        cache_ref.extend_from_slice(&2i16.to_le_bytes());
        cache_ref.extend_from_slice(&0i16.to_le_bytes());
        cache_ref.extend_from_slice(&77i32.to_le_bytes());
        let cache_hit_fields: Vec<Vec<u8>> = vec![
            vec![1u8],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            rect_xywh(2, 0, 2, 2),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            cache_ref,
            Vec::new(),
        ];
        assert_eq!(screen.apply(&cache_hit_fields).unwrap(), 1);
        assert_eq!(screen.canvas.get_pixel(2, 0), (255, 0, 0));
        assert_eq!(screen.canvas.get_pixel(3, 1), (255, 255, 255));

        let mut missing_and_valid_ref = Vec::new();
        missing_and_valid_ref.extend_from_slice(&0i16.to_le_bytes());
        missing_and_valid_ref.extend_from_slice(&2i16.to_le_bytes());
        missing_and_valid_ref.extend_from_slice(&9999i32.to_le_bytes());
        missing_and_valid_ref.extend_from_slice(&2i16.to_le_bytes());
        missing_and_valid_ref.extend_from_slice(&2i16.to_le_bytes());
        missing_and_valid_ref.extend_from_slice(&77i32.to_le_bytes());
        let partial_cache_fields: Vec<Vec<u8>> = vec![
            vec![1u8],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            rect_xywh(0, 2, 4, 2),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            missing_and_valid_ref,
            Vec::new(),
        ];
        assert_eq!(screen.apply(&partial_cache_fields).unwrap(), 1);
        assert_eq!(screen.canvas.get_pixel(2, 2), (255, 0, 0));
        assert_eq!(screen.canvas.get_pixel(3, 3), (255, 255, 255));

        let mut jpeg_bytes = Vec::new();
        let atlas_rgb = vec![0u8, 255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0];
        JpegEncoder::new_with_quality(&mut jpeg_bytes, 100)
            .write_image(&atlas_rgb, 2, 2, ExtendedColorType::Rgb8)
            .unwrap();
        let jpeg_fields: Vec<Vec<u8>> = vec![
            vec![1u8],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            jpeg_bytes,
            rect_xywh(0, 2, 2, 2),
            Vec::new(),
            Vec::new(),
        ];
        assert_eq!(screen.apply(&jpeg_fields).unwrap(), 1);
        let p = screen.canvas.get_pixel(0, 2);
        assert!(p.0 <= 2 && p.1 >= 253 && p.2 <= 2);

        let png = screen.to_png().unwrap();
        let decoded_png = image::load_from_memory_with_format(&png, ImageFormat::Png).unwrap();
        assert_eq!((decoded_png.width(), decoded_png.height()), (4, 4));

        let login_fields = vec![
            vec![0u8],
            1920i32.to_le_bytes().to_vec(),
            1080i32.to_le_bytes().to_vec(),
        ];
        assert_eq!(
            desktop_size_from_login(&login_fields).unwrap(),
            (1920, 1080)
        );

        let mut buf = RgbBuffer::new(4, 4);
        let simple_fields = vec![
            vec![1u8],
            rect_xywh(0, 0, 2, 2),
            fields[2].clone(),
            Vec::new(),
            Vec::new(),
        ];
        assert_eq!(
            apply_rgb555_update(&mut buf, &simple_fields, |p, sz| {
                screen.decoder.decompress(p, sz)
            })
            .unwrap(),
            1
        );
    }

    #[test]
    fn test_input_control_packets() {
        assert_eq!(mouse_move(640, 360).unwrap(), b"8|||512|||0|||640|||360");
        assert_eq!(mouse_button("left", true).unwrap(), b"8|||513");
        assert_eq!(mouse_button("left", false).unwrap(), b"8|||514");
        assert_eq!(mouse_button("right", true).unwrap(), b"8|||516");
        assert_eq!(mouse_button("right", false).unwrap(), b"8|||517");
        assert_eq!(mouse_button("middle", true).unwrap(), b"8|||519");
        assert_eq!(mouse_button("middle", false).unwrap(), b"8|||520");
        assert_eq!(mouse_wheel(120).unwrap(), b"8|||522|||7864320");
        assert_eq!(keyboard(65, true, false).unwrap(), b"8|||256|||65");
        assert_eq!(keyboard(65, false, true).unwrap(), b"8|||261|||65");

        let mut sent = Vec::new();
        with_held_key(
            |pkt| sent.push(pkt),
            18,
            true,
            || {
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            sent,
            vec![b"8|||260|||18".to_vec(), b"8|||261|||18".to_vec()]
        );

        let evt = InputEvent {
            event_type: "move".to_string(),
            x: 100,
            y: 200,
            ..Default::default()
        };
        assert_eq!(evt.to_wire().unwrap(), mouse_move(100, 200).unwrap());
    }

    #[test]
    fn test_g722_audio_codec_and_packets() {
        let mut enc = G722CodecState::new();
        let mut dec = G722CodecState::new();
        let mut samples = Vec::with_capacity(320);
        for i in 0..320 {
            let v = ((i as f64 * 0.15).sin() * 8000.0) as i16;
            samples.push(v);
        }
        let g722_bytes = enc.encode(&samples);
        assert_eq!(g722_bytes.len(), 160);
        let decoded_samples = dec.decode(&g722_bytes);
        assert_eq!(decoded_samples.len(), 320);
        let energy: i64 = decoded_samples.iter().map(|&s| (s as i64).abs()).sum();
        assert!(energy > 100_000);

        let pkt42 = encode_audio_packet(WaveSource::PcSound, &128i32.to_le_bytes(), true).unwrap();
        let pkt43 = encode_audio_packet(WaveSource::PcSound, &g722_bytes, false).unwrap();
        let pkt50 =
            encode_audio_packet(WaveSource::Microphone, &64i32.to_le_bytes(), true).unwrap();
        let pkt51 = encode_audio_packet(WaveSource::Microphone, &g722_bytes, false).unwrap();

        let mut buf = AudioBuffer::new(16);
        let d42 = buf.ingest_packet(&pkt42).unwrap().unwrap();
        assert_eq!(d42.kind, PACKET_PC_SILENCE);
        assert!(d42.is_silence);
        assert_eq!(d42.pcm_s16le.len(), 128);

        let d43 = buf.ingest_packet(&pkt43).unwrap().unwrap();
        assert_eq!(d43.kind, PACKET_PC_G722);
        assert!(!d43.is_silence);
        assert_eq!(d43.pcm_s16le.len(), 640);

        let d50 = buf.ingest_packet(&pkt50).unwrap().unwrap();
        assert_eq!(d50.kind, PACKET_MIC_SILENCE);
        assert_eq!(d50.pcm_s16le.len(), 64);

        let d51 = buf.ingest_packet(&pkt51).unwrap().unwrap();
        assert_eq!(d51.kind, PACKET_MIC_G722);
        assert_eq!(d51.pcm_s16le.len(), 640);

        let (seq, wav) = buf.wav_since(0);
        assert_eq!(seq, 4);
        assert!(wav.starts_with(b"RIFF"));
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + 128 + 640 + 64 + 640);

        assert_eq!(encode_sound_toggle_command(true), b"|||31|||");
        assert_eq!(encode_sound_toggle_command(false), b"|||32|||");
        let direct_wav = build_wav_bytes(&[0u8; 16], 48000, 2, 16);
        assert_eq!(direct_wav.len(), 60);
        let mut pc = G722CodecState::new();
        let mut mic = G722CodecState::new();
        assert!(decode_audio_packet(b"not-audio", &mut pc, &mut mic)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_clipboard_sync_all_wire_formats() {
        let pkt2 = encode_clipboard_packet(7, "UltraViewer Tiếng Việt").unwrap();
        let msg2 = decode_clipboard_packet(&pkt2).unwrap();
        assert_eq!(msg2.client_id, 7);
        assert_eq!(msg2.text, "UltraViewer Tiếng Việt");

        let pkt3 = encode_clipboard_modern_packet(42, "Clipboard 3-field").unwrap();
        let msg3 = decode_clipboard_packet(&pkt3).unwrap();
        assert_eq!(msg3.client_id, 42);
        assert_eq!(msg3.text, "Clipboard 3-field");

        let msg_legacy = decode_clipboard_packet(b"|||33|||1|||Legacy text").unwrap();
        assert_eq!(msg_legacy.client_id, 1);
        assert_eq!(msg_legacy.text, "Legacy text");
    }

    #[test]
    fn test_file_transfer_protocol_and_state_machine() {
        let offer_m = encode_file_offer("f1", "report.pdf", 102400, true).unwrap();
        assert_eq!(
            decode_file_transfer_packet(&offer_m),
            Some(FileTransferPacket::Offer {
                file_id: "f1".to_string(),
                file_name: "report.pdf".to_string(),
                file_size: 102400,
            })
        );
        let offer_l = encode_file_offer("f1", "report.pdf", 102400, false).unwrap();
        assert_eq!(
            decode_file_transfer_packet(&offer_l),
            Some(FileTransferPacket::Offer {
                file_id: "f1".to_string(),
                file_name: "report.pdf".to_string(),
                file_size: 102400,
            })
        );

        let accept_m = encode_file_accept("f1", 1, true).unwrap();
        assert_eq!(
            decode_file_transfer_packet(&accept_m),
            Some(FileTransferPacket::Accept {
                file_id: "f1".to_string(),
                resume_offset: 0,
            })
        );

        let chunk_2f = encode_file_chunk("f1", 512, b"chunk-bytes", false).unwrap();
        assert_eq!(
            decode_file_transfer_packet(&chunk_2f),
            Some(FileTransferPacket::Chunk {
                file_id: "f1".to_string(),
                offset: 512,
                data: b"chunk-bytes".to_vec(),
            })
        );
        let chunk_4f = encode_file_chunk("f1", 512, b"chunk-bytes", true).unwrap();
        assert_eq!(
            decode_file_transfer_packet(&chunk_4f),
            Some(FileTransferPacket::Chunk {
                file_id: "f1".to_string(),
                offset: 512,
                data: b"chunk-bytes".to_vec(),
            })
        );

        let ack_m = encode_file_ack("f1", 1024, true).unwrap();
        assert_eq!(
            decode_file_transfer_packet(&ack_m),
            Some(FileTransferPacket::Ack {
                file_id: "f1".to_string(),
                next_offset: 1024,
            })
        );

        let cancel_m = encode_file_cancel("f1", true).unwrap();
        assert_eq!(
            decode_file_transfer_packet(&cancel_m),
            Some(FileTransferPacket::Cancel {
                file_id: "f1".to_string(),
            })
        );

        let chat_m = encode_chat_message("Hello world", true).unwrap();
        assert_eq!(
            decode_file_transfer_packet(&chat_m),
            Some(FileTransferPacket::ChatMessage {
                text: "Hello world".to_string(),
            })
        );

        let tmp_dir = std::env::temp_dir().join(format!("uv_ft_test_{}", std::process::id()));
        let mut sender = FileTransferManager::new(tmp_dir.clone());
        sender.chunk_size = 1024;
        let mut receiver = FileTransferManager::new(tmp_dir.clone());
        receiver.auto_accept = true;

        let payload_bytes: Vec<u8> = (0..2500).map(|i| (i % 251) as u8).collect();
        let (file_id, offer_pkt) = sender
            .offer_file_bytes("archive.bin", payload_bytes.clone())
            .unwrap();

        let (_, mut rx_replies) = receiver.handle_packet(&offer_pkt).unwrap().unwrap();
        assert_eq!(rx_replies.len(), 1);
        let accept_pkt = rx_replies.pop().unwrap();

        let (_, tx_chunks) = sender.handle_packet(&accept_pkt).unwrap().unwrap();
        assert_eq!(tx_chunks.len(), 2);

        let (_, ack1) = receiver.handle_packet(&tx_chunks[0]).unwrap().unwrap();
        assert_eq!(ack1.len(), 1);
        let (_, mut ack2) = receiver.handle_packet(&tx_chunks[1]).unwrap().unwrap();
        assert_eq!(ack2.len(), 1);
        let ack_pkt = ack2.pop().unwrap();

        let (_, final_chunks) = sender.handle_packet(&ack_pkt).unwrap().unwrap();
        assert_eq!(final_chunks.len(), 1);
        assert_eq!(
            sender.transfers.get(&file_id).unwrap().status,
            TransferStatus::Completed
        );

        let (_, final_replies) = receiver.handle_packet(&final_chunks[0]).unwrap().unwrap();
        assert!(final_replies.is_empty());
        let rx_entry = receiver.transfers.get(&file_id).unwrap();
        assert_eq!(rx_entry.direction, TransferDirection::Receive);
        assert_eq!(rx_entry.status, TransferStatus::Completed);
        assert_eq!(rx_entry.buffer, payload_bytes);
        assert!(rx_entry.saved_path.is_some());
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_control_session_and_http_portal_end_to_end() {
        assert!(looks_like_tls_record(b"\x16\x03\x01\x00\x20"));
        assert!(!looks_like_tls_record(b"\x01\x05\x00\x00\x00"));

        let b64 = encode_base64(b"hello-rust-uv");
        assert_eq!(decode_base64(&b64).unwrap(), b"hello-rust-uv");

        let session = ControlSession::new();
        let login_pkt = encode_byte_array_fields(&[
            b"\x00\x00",
            &1920i32.to_le_bytes(),
            &1080i32.to_le_bytes(),
        ])
        .unwrap();
        session.handle_session_payload(&login_pkt);
        let snap_after_login = session.snapshot();
        assert_eq!(snap_after_login.state, "connected");
        assert_eq!(snap_after_login.screen_width, 1920);
        assert_eq!(snap_after_login.screen_height, 1080);

        let clip_pkt = encode_clipboard_modern_packet(1, "Remote clip 123").unwrap();
        session.handle_session_payload(&clip_pkt);
        let audio_pkt =
            encode_audio_packet(WaveSource::PcSound, &64i32.to_le_bytes(), true).unwrap();
        session.handle_session_payload(&audio_pkt);

        let (portal, listener) = PortalServer::bind("127.0.0.1", 0, session.clone(), None).unwrap();
        let addr = portal.bind_addr.clone();
        let shutdown = portal.shutdown.clone();
        let handle = portal.spawn(listener);

        let resp_state = http_get(&addr, "/status");
        assert!(resp_state.contains("\"clipboard_remote\":\"Remote clip 123\""));
        assert!(resp_state.contains("\"audio_chunks_received\":1"));

        let resp_send = http_post(
            &addr,
            "/files/send",
            &format!(
                "{{\"file_name\":\"test.txt\",\"data_base64\":\"{}\"}}",
                encode_base64(b"hello file transfer")
            ),
        );
        assert!(resp_send.contains("\"ok\":true"));
        assert!(resp_send.contains("uvf_1"));

        let resp_dl = http_get(&addr, "/files/download?file_id=uvf_1");
        assert!(resp_dl.ends_with("hello file transfer"));

        let resp_audio = http_post(&addr, "/audio/toggle", "{\"enable\":false}");
        assert!(resp_audio.contains("\"audio_enabled\":false"));

        let resp_evt = http_post(&addr, "/event", "{\"type\":\"move\",\"x\":150,\"y\":250}");
        assert!(resp_evt.contains("\"ok\":true"));

        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = handle.join();
    }

    fn http_get(addr: &str, path: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        let req = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            path, addr
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        String::from_utf8_lossy(&out).to_string()
    }

    fn http_post(addr: &str, path: &str, json_body: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        let req = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            path,
            addr,
            json_body.len(),
            json_body
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        String::from_utf8_lossy(&out).to_string()
    }

    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }
}

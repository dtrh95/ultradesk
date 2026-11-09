#![allow(
    clippy::large_enum_variant,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

pub mod audio;
pub mod clipboard;
pub mod cursor;
pub mod direct;
pub mod e2e;
pub mod file_transfer;
pub mod gui;
pub mod host;
pub mod input;
pub mod mcp;
pub mod profile;
pub mod protocol;
pub mod screen;
pub mod server;
pub mod session;
pub mod updater;

pub use gui::run_clipboard_sync_tick;

#[cfg(test)]
mod tests {
    use crate::audio::{
        build_wav_bytes, decode_audio_packet, encode_audio_packet, encode_sound_toggle_command,
        process_form1_packet, AudioBuffer, G722CodecState, WaveSource, PACKET_MIC_G722,
        PACKET_MIC_SILENCE, PACKET_PC_G722, PACKET_PC_SILENCE,
    };
    use crate::clipboard::{
        clipboard_text_eq, decode_clipboard_packet, encode_clipboard_for_remote,
        encode_clipboard_modern_packet, encode_clipboard_packet,
    };
    use crate::file_transfer::{
        decode_file_transfer_packet, encode_chat_message, encode_file_accept, encode_file_ack,
        encode_file_cancel, encode_file_chunk, encode_file_offer, FileTransferManager,
        FileTransferPacket, TransferDirection, TransferStatus,
    };
    use crate::gui::{
        run_clipboard_sync_tick, ClipboardAction, ClipboardEvent, ClipboardSeedingState,
        ClipboardSyncMachine, LanguageCode, SpecialKey, ThemeName, ToolTab, ToolbarAction,
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
        encode_byte_array_fields, encode_p2p_frame, encode_relay_frame, encode_senddata_string,
        encode_transport_frame, encrypt_p2p_aes_body, encrypt_registration_bytes, export_cache,
        extract_command3_identity, extract_tile, http_service_preface, import_cache,
        login_password_fields, pack_atlas, restore_atlas, tile_sample_score, CachedItem,
        FrameStream, REGISTRATION_PASSWORD,
    };
    use crate::screen::{
        apply_rgb555_update, compress_rgb555_with_dict, desktop_size_from_login, RGB555Screen,
        RgbBuffer, ZstdDictionaryDecoder,
    };
    use crate::server::{
        decode_base64, encode_base64, materialize_embedded_gui_assets, GuiLaunchOutcome,
        PortalServer,
    };
    use crate::session::{
        effective_lookup_terms, effective_registration_terms, load_saved_credentials,
        looks_like_tls_record, ControlSession, PartnerProfile, RegistrationProfile,
    };
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
        pack_atlas(
            &frame24,
            &mut atlas,
            3,
            12,
            stride24 as isize,
            1,
            1,
            0,
            0,
            2,
            2,
            4,
        )
        .unwrap();
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
        let (dec_enc, flag_enc) = decode_p2p_frame_body(&enc_frame[4..], Some("secret")).unwrap();
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
        assert_eq!(seen, vec![("Play16BitAudio".to_string(), vec![0u8; 12])]);

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

        let mut screen16 = RGB555Screen::new((64, 64), ZstdDictionaryDecoder::default(), 100, 32);
        let mut jpeg_16col = Vec::new();
        let atlas_16x64 = vec![200u8; 16 * 64 * 3];
        JpegEncoder::new_with_quality(&mut jpeg_16col, 100)
            .write_image(&atlas_16x64, 16, 64, ExtendedColorType::Rgb8)
            .unwrap();
        let jpeg_16_fields: Vec<Vec<u8>> = vec![
            vec![1u8],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            jpeg_16col,
            rect_xywh(10, 10, 32, 32),
            Vec::new(),
            Vec::new(),
        ];
        assert_eq!(screen16.apply(&jpeg_16_fields).unwrap(), 4);
        assert_eq!(screen16.block_size, 16);

        let mut mixed_refs = Vec::new();
        mixed_refs.extend_from_slice(&0i16.to_le_bytes());
        mixed_refs.extend_from_slice(&0i16.to_le_bytes());
        mixed_refs.extend_from_slice(&77i32.to_le_bytes());
        mixed_refs.extend_from_slice(&2i16.to_le_bytes());
        mixed_refs.extend_from_slice(&0i16.to_le_bytes());
        mixed_refs.extend_from_slice(&77i32.to_le_bytes());
        screen.canvas.pixels.fill(0);
        let mixed_fields: Vec<Vec<u8>> = vec![
            vec![1u8],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            rect_xywh(0, 0, 1, 1),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            mixed_refs,
            Vec::new(),
        ];
        assert_eq!(screen.apply(&mixed_fields).unwrap(), 2);
        assert_eq!(screen.canvas.get_pixel(0, 0), (255, 0, 0));
        assert_eq!(screen.canvas.get_pixel(1, 1), (0, 0, 0));
        assert_eq!(screen.canvas.get_pixel(2, 0), (255, 0, 0));
        assert_eq!(screen.canvas.get_pixel(3, 1), (255, 255, 255));
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
        assert_eq!(keyboard(91, true, false).unwrap(), b"8|||256|||91");
        assert_eq!(keyboard(91, false, false).unwrap(), b"8|||257|||91");
        assert_eq!(keyboard(160, true, false).unwrap(), b"8|||256|||16");
        assert_eq!(keyboard(161, false, false).unwrap(), b"8|||257|||16");
        assert_eq!(keyboard(162, true, false).unwrap(), b"8|||256|||17");
        assert_eq!(keyboard(163, false, false).unwrap(), b"8|||257|||17");
        assert_eq!(keyboard(164, true, true).unwrap(), b"8|||260|||18");
        assert_eq!(keyboard(165, false, true).unwrap(), b"8|||261|||18");

        let mut sent = Vec::new();
        with_held_key(|pkt| sent.push(pkt), 18, true, || Ok(())).unwrap();
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

        assert_eq!(encode_sound_toggle_command(true), b"31|||");
        assert_eq!(encode_sound_toggle_command(false), b"32|||");
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

        let short_vserver =
            encode_byte_array_fields(&[&[5u8], &encode_senddata_string("Copy ngắn", true)])
                .unwrap();
        let msg_short = decode_clipboard_packet(&short_vserver).unwrap();
        assert_eq!(msg_short.text, "Copy ngắn");

        let long_text = "Đoạn văn bản dài hơn 150 ký tự từ máy remote ".repeat(5);
        let (long_ctrl, is_utf16) = encode_clipboard_for_remote(&long_text).unwrap();
        assert!(is_utf16);
        let msg_long_ctrl = decode_clipboard_packet(&long_ctrl).unwrap();
        assert_eq!(msg_long_ctrl.text, long_text);

        let utf16_bytes = encode_senddata_string(&long_text, true);
        let zstd_bytes = zstd::bulk::compress(&utf16_bytes, 1).unwrap();
        let uncomp_len = (utf16_bytes.len() as i32).to_le_bytes();
        let vserver_zstd = encode_byte_array_fields(&[&[33u8], &uncomp_len, &zstd_bytes]).unwrap();
        let msg_vserver_zstd = decode_clipboard_packet(&vserver_zstd).unwrap();
        assert_eq!(msg_vserver_zstd.text, long_text);

        assert!(clipboard_text_eq("Line1\r\nLine2\r\n", "Line1\nLine2"));
        assert!(!clipboard_text_eq("Line1\r\nLine2", "Line1\nLine3"));
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

        let host_chat_i16 = encode_byte_array_fields(&[
            &[12u8, 0u8],
            &encode_senddata_string("hello cc đồng ý", true),
        ])
        .unwrap();
        assert_eq!(
            decode_file_transfer_packet(&host_chat_i16),
            Some(FileTransferPacket::ChatMessage {
                text: "hello cc đồng ý".to_string(),
            })
        );

        let tmp_dir = std::env::temp_dir().join(format!("uv_ft_test_{}", std::process::id()));
        let mut sender = FileTransferManager::new(tmp_dir.clone());
        assert!(sender.modern_protocol);
        sender.chunk_size = 1024;
        let mut receiver = FileTransferManager::new(tmp_dir.clone());
        assert!(receiver.modern_protocol);
        receiver.auto_accept = true;

        let payload_bytes: Vec<u8> = (0..2500).map(|i| (i % 251) as u8).collect();
        let (file_id, offer_pkt) = sender
            .offer_file_bytes("archive.bin", payload_bytes.clone())
            .unwrap();
        let offer_fields = decode_byte_array_fields_checked(&offer_pkt, Some(4)).unwrap();
        assert_eq!(offer_fields[0], vec![13u8]);

        let (_, mut rx_replies) = receiver.handle_packet(&offer_pkt).unwrap().unwrap();
        assert_eq!(rx_replies.len(), 1);
        let accept_pkt = rx_replies.pop().unwrap();

        let (_, tx_chunks) = sender.handle_packet(&accept_pkt).unwrap().unwrap();
        assert_eq!(tx_chunks.len(), 2);
        let chunk0_fields = decode_byte_array_fields_checked(&tx_chunks[0], Some(4)).unwrap();
        assert_eq!(chunk0_fields[0], vec![15u8]);
        assert_eq!(chunk0_fields[2].len(), 8);

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
        assert_eq!(final_replies.len(), 1);
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

        let test_creds_dir =
            std::env::temp_dir().join(format!("uv_creds_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&test_creds_dir);
        let _ = std::fs::create_dir_all(&test_creds_dir);
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &test_creds_dir);

        let session = ControlSession::new();
        let snap_init = session.snapshot();
        assert_eq!(snap_init.saved_language, "en");
        assert_eq!(snap_init.saved_theme, "auto-time");
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
        let host_chat_pkt =
            encode_byte_array_fields(&[&[12u8, 0u8], &encode_senddata_string("hello cc", true)])
                .unwrap();
        session.handle_session_payload(&host_chat_pkt);
        assert_eq!(
            session.snapshot().chat_messages,
            vec!["Partner: hello cc".to_string()]
        );
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
        let snap_after_move = session.snapshot();
        assert_eq!(
            (snap_after_move.cursor_x, snap_after_move.cursor_y),
            (150, 250)
        );

        let resp_pref = http_post(
            &addr,
            "/preferences",
            "{\"theme\":\"gruvbox-light\",\"language\":\"vi\",\"toolbar_collapsed\":true}",
        );
        assert!(resp_pref.contains("\"ok\":true"));
        let snap_pref = session.snapshot();
        assert_eq!(snap_pref.saved_theme, "gruvbox-light");
        assert_eq!(snap_pref.saved_language, "vi");
        assert!(snap_pref.saved_toolbar_collapsed);

        let creds_from_disk = load_saved_credentials();
        assert_eq!(creds_from_disk.theme, "gruvbox-light");
        assert_eq!(creds_from_disk.language, "vi");
        assert!(creds_from_disk.toolbar_collapsed);

        let resp_html = http_get(&addr, "/");
        assert!(resp_html.contains("<html lang=\"en\">"));
        assert!(resp_html.contains("lang: 'en'"));
        assert!(resp_html.contains("UltraDesk (Rust)"));
        assert!(resp_html.contains("ControlLeft:17"));
        assert!(resp_html.contains("id=\"btnCycleTheme\""));
        assert!(resp_html.contains("id=\"themeSelect\""));
        assert!(resp_html.contains("data-theme=\"onedark\""));
        assert!(resp_html.contains("data-theme=\"vscode-dark\""));
        assert!(resp_html.contains("data-theme=\"vscode-light\""));
        assert!(resp_html.contains("resolveActiveTheme"));
        assert!(resp_html.contains("cycleTheme"));
        assert!(resp_html.contains("canvasContentRect"));
        assert!(resp_html.contains("scheduleMove"));
        assert!(!resp_html.contains("drawImage(cursorImg"));
        assert!(resp_html.contains("--text-sm: 12px;"));
        assert!(resp_html.contains("'Noto Sans'"));
        assert!(resp_html.contains("'Host Grotesk'"));
        assert!(resp_html.contains("--m-ease"));
        assert!(resp_html.contains("showNotice"));
        assert!(resp_html.contains(".m-tab-ind"));
        assert!(!resp_html.contains("backdrop-filter"));
        assert!(!resp_html.contains("alert("));
        assert!(!resp_html.contains("'Inter'"));
        assert!(!resp_html.contains("--pill-bg"));
        assert!(!resp_html.contains("(|||31|||)"));

        let resp_icon = http_get(&addr, "/ultradesk-rust.png");
        assert!(resp_icon.contains("Content-Type: image/png"));

        let gui_tmp = std::env::temp_dir().join(format!("uv_gui_test_{}", std::process::id()));
        let viewer_asset_path = materialize_embedded_gui_assets(&gui_tmp).unwrap();
        let viewer_text = std::fs::read_to_string(&viewer_asset_path).unwrap();
        assert!(viewer_text.contains("canvasContentRect"));
        assert!(gui_tmp.join("ultradesk-rust.png").exists());
        let _ = std::fs::remove_dir_all(&gui_tmp);

        assert_eq!(
            GuiLaunchOutcome::from_exit(Some(0), std::time::Duration::from_millis(50)),
            GuiLaunchOutcome::ClosedCleanly
        );
        assert!(matches!(
            GuiLaunchOutcome::from_exit(Some(2), std::time::Duration::from_millis(120)),
            GuiLaunchOutcome::FallbackToBrowser(_)
        ));
        assert_eq!(
            GuiLaunchOutcome::from_exit(Some(1), std::time::Duration::from_secs(10)),
            GuiLaunchOutcome::ClosedCleanly
        );

        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = handle.join();
        match prev_xdg {
            Some(val) => std::env::set_var("XDG_CONFIG_HOME", val),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&test_creds_dir);
    }

    #[test]
    fn test_host_mode_and_portal_upgrade_end_to_end() {
        let session = ControlSession::new();
        let (portal, listener) = PortalServer::bind("127.0.0.1", 0, session.clone(), None).unwrap();
        let addr = format!("127.0.0.1:{}", portal.port);
        let shutdown = portal.shutdown.clone();
        let handle = portal.spawn(listener);

        let w_bytes = 3840i32.to_le_bytes();
        let h_bytes = 1080i32.to_le_bytes();
        let zero_bytes = 0i32.to_le_bytes();
        let init_pkt = encode_byte_array_fields(&[
            &[0u8][..],
            &w_bytes[..],
            &h_bytes[..],
            &[][..],
            &[][..],
            &[0u8][..],
            &b"6.6.133"[..],
            &[2u8][..],
            &b"0:0:1920:1080|1920:0:3840:1080|\0"[..],
            &zero_bytes[..],
            &zero_bytes[..],
        ])
        .unwrap();
        session.handle_session_payload(&init_pkt);

        let snap = session.snapshot();
        assert_eq!(snap.screen_width, 3840);
        assert_eq!(snap.monitors.len(), 2);

        let ignored_sel = encode_byte_array_fields(&[
            &[3u8][..],
            &crate::cursor::IGNORED_SELECTION_HASH.to_le_bytes()[..],
        ])
        .unwrap();
        {
            let mut guard = session.inner.lock().unwrap();
            guard.outbound_queue.clear();
        }
        session.handle_session_payload(&ignored_sel);
        {
            let guard = session.inner.lock().unwrap();
            assert!(guard.outbound_queue.is_empty());
        }

        let valid_hash: i32 = 424242;
        let sel_pkt =
            encode_byte_array_fields(&[&[3u8][..], &valid_hash.to_le_bytes()[..]]).unwrap();
        session.handle_session_payload(&sel_pkt);
        {
            let guard = session.inner.lock().unwrap();
            assert_eq!(guard.outbound_queue.len(), 1);
            assert_eq!(guard.outbound_queue[0].0, b"4|||424242");
        }

        let pos_pkt = encode_byte_array_fields(&[
            &[28u8][..],
            &120i16.to_le_bytes()[..],
            &240i16.to_le_bytes()[..],
        ])
        .unwrap();
        session.handle_session_payload(&pos_pkt);
        let snap_pos = session.snapshot();
        assert_eq!((snap_pos.cursor_x, snap_pos.cursor_y), (120, 240));

        let resp_mon = http_post(&addr, "/monitor", "{\"index\":1}");
        assert!(resp_mon.contains("\"ok\":true"));
        let snap_mon = session.snapshot();
        assert_eq!(snap_mon.selected_monitor, Some(1));
        assert_eq!(snap_mon.width, 1920);
        assert_eq!(snap_mon.height, 1080);

        let ver = snap_mon.frame_version;
        let resp_jpg = http_get(&addr, "/frame.jpg?v=0&q=75");
        assert!(resp_jpg.starts_with("HTTP/1.1 200 OK"));
        assert!(resp_jpg.contains(&format!("X-Frame-Version: {}", ver)));

        let resp_304 = http_get(&addr, &format!("/frame.jpg?v={}&q=75", ver));
        assert!(resp_304.starts_with("HTTP/1.1 304 Not Modified"));

        let resp_bmp = http_get(&addr, "/frame.bmp?v=0");
        assert!(resp_bmp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp_bmp.contains("Content-Type: image/bmp"));

        let resp_bmp_304 = http_get(&addr, &format!("/frame.bmp?v={}", ver));
        assert!(resp_bmp_304.starts_with("HTTP/1.1 304 Not Modified"));

        let resp_text = http_post(&addr, "/text", "{\"text\":\"Việt\",\"caps_lock\":false}");
        assert!(resp_text.contains("\"ok\":true"));
        assert!(resp_text.contains("\"packets_sent\":8"));

        let resp_host_start = http_post(&addr, "/host/start", "{}");
        assert!(resp_host_start.contains("\"running\":true"));

        let resp_host_status = http_get(&addr, "/host/status");
        assert!(resp_host_status.contains("\"running\":true"));

        let resp_host_stop = http_post(&addr, "/host/stop", "{}");
        assert!(resp_host_stop.contains("\"running\":false"));

        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = handle.join();
    }

    fn fetch_test_portal_token(addr: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        let req = format!(
            "GET / HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            addr
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        let text = String::from_utf8_lossy(&out);
        for line in text.lines() {
            if let Some(rest) = line
                .strip_prefix("Set-Cookie: ")
                .or_else(|| line.strip_prefix("set-cookie: "))
            {
                for part in rest.split(';') {
                    if let Some(tok) = part.trim().strip_prefix("ultradesk_token=") {
                        return tok.to_string();
                    }
                }
            }
        }
        String::new()
    }

    fn http_get(addr: &str, path: &str) -> String {
        let token = if path == "/" {
            String::new()
        } else {
            fetch_test_portal_token(addr)
        };
        let mut s = TcpStream::connect(addr).unwrap();
        let req = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nX-Viewer-Token: {}\r\nConnection: close\r\n\r\n",
            path, addr, token
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        String::from_utf8_lossy(&out).to_string()
    }

    fn http_post(addr: &str, path: &str, json_body: &str) -> String {
        let token = fetch_test_portal_token(addr);
        let mut s = TcpStream::connect(addr).unwrap();
        let req = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nX-Viewer-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            path,
            addr,
            token,
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

    #[test]
    fn test_clipboard_sync_machine_seeding_and_echo_prevention() {
        let mut machine = ClipboardSyncMachine::new();
        assert_eq!(machine.seeding_state(), ClipboardSeedingState::Unseeded);

        assert_eq!(machine.on_host_clipboard("initial local text", true), None);
        assert_eq!(machine.seeding_state(), ClipboardSeedingState::Seeded);

        assert_eq!(machine.on_host_clipboard("second local text", false), None);

        assert_eq!(
            machine.on_host_clipboard("third local text", true),
            Some("third local text".to_string())
        );

        assert_eq!(machine.on_host_clipboard("third local text", true), None);

        assert_eq!(
            machine.on_remote_clipboard("remote incoming text\r\n"),
            Some("remote incoming text\r\n".to_string())
        );

        assert_eq!(
            machine.on_host_clipboard("remote incoming text", true),
            None
        );

        let mut step_machine = ClipboardSyncMachine::new();
        assert_eq!(
            step_machine.step(ClipboardEvent::ConnectionChanged(true)),
            None
        );
        assert_eq!(
            step_machine.step(ClipboardEvent::SystemTextRead("first read".to_string())),
            None
        );
        assert_eq!(
            step_machine.step(ClipboardEvent::RemoteTextReceived("peer text".to_string())),
            Some(ClipboardAction::WriteToSystem("peer text".to_string()))
        );
        assert_eq!(
            step_machine.step(ClipboardEvent::SystemTextRead("peer text".to_string())),
            None
        );
        assert_eq!(
            step_machine.step(ClipboardEvent::SystemTextRead("peer text 2".to_string())),
            Some(ClipboardAction::PostToRemote("peer text 2".to_string()))
        );
    }

    #[test]
    fn test_clipboard_sync_machine_normalization() {
        assert_eq!(
            ClipboardSyncMachine::normalize("hello\r\nworld\r\n"),
            "hello\nworld"
        );
        assert_eq!(
            ClipboardSyncMachine::normalize("hello\nworld\n"),
            "hello\nworld"
        );
        assert_eq!(ClipboardSyncMachine::normalize(""), "");
    }

    #[test]
    fn test_toolbar_action_serde_roundtrips() {
        let actions = vec![
            ToolbarAction::ToggleConnectModal,
            ToolbarAction::DisconnectRemote,
            ToolbarAction::StartHostSharing,
            ToolbarAction::StopHostSharing,
            ToolbarAction::ToggleToolsPopover,
            ToolbarAction::SelectToolTab(ToolTab::Chat),
            ToolbarAction::SelectToolTab(ToolTab::Files),
            ToolbarAction::SelectToolTab(ToolTab::Clipboard),
            ToolbarAction::SelectToolTab(ToolTab::Audio),
            ToolbarAction::SetTheme(ThemeName::AutoTime),
            ToolbarAction::SetTheme(ThemeName::AutoOs),
            ToolbarAction::SetTheme(ThemeName::Onedark),
            ToolbarAction::SetTheme(ThemeName::Onelight),
            ToolbarAction::SetTheme(ThemeName::VscodeDark),
            ToolbarAction::SetTheme(ThemeName::VscodeLight),
            ToolbarAction::SetTheme(ThemeName::GruvboxDark),
            ToolbarAction::SetTheme(ThemeName::GruvboxLight),
            ToolbarAction::CycleTheme,
            ToolbarAction::SetLanguage(LanguageCode::Vi),
            ToolbarAction::SetLanguage(LanguageCode::En),
            ToolbarAction::SelectMonitor(Some(0)),
            ToolbarAction::SelectMonitor(Some(2)),
            ToolbarAction::SelectMonitor(None),
            ToolbarAction::SendSpecialKey(SpecialKey::Super),
            ToolbarAction::SendSpecialKey(SpecialKey::CtrlAltDel),
            ToolbarAction::SendSpecialKey(SpecialKey::F9NextMonitor),
            ToolbarAction::ToggleToolbarCollapse,
            ToolbarAction::SetToolbarCollapsed(true),
            ToolbarAction::SetToolbarCollapsed(false),
            ToolbarAction::ToggleFullscreen,
            ToolbarAction::CopyHostId,
            ToolbarAction::QuitApplication,
            ToolbarAction::Input(crate::input::InputEvent::default()),
        ];

        for action in &actions {
            let json = serde_json::to_string(action).unwrap();
            let parsed: ToolbarAction = serde_json::from_str(&json).unwrap();
            assert_eq!(action, &parsed);
        }

        let json = serde_json::to_string(&ToolbarAction::SelectToolTab(ToolTab::Chat)).unwrap();
        assert_eq!(json, r#"{"action":"SelectToolTab","payload":"chat"}"#);

        let json = serde_json::to_string(&ToolbarAction::SetTheme(ThemeName::Onedark)).unwrap();
        assert_eq!(json, r#"{"action":"SetTheme","payload":"onedark"}"#);

        let json = serde_json::to_string(&ToolbarAction::SetLanguage(LanguageCode::Vi)).unwrap();
        assert_eq!(json, r#"{"action":"SetLanguage","payload":"vi"}"#);

        let json =
            serde_json::to_string(&ToolbarAction::SendSpecialKey(SpecialKey::Super)).unwrap();
        assert_eq!(json, r#"{"action":"SendSpecialKey","payload":"super"}"#);
    }

    #[test]
    fn test_lan_direct_and_master_password_status_99_reproduction() {
        let route = decode_target_route(
            b"8:1:0:118.69.226.188:118.69.226.188:0:0:54662:VWx0cmFWaWV3ZXI=:57808:103.56.158.44:103.56.158.44:1:-2::-1062728963:0::0:False:0:|0:0",
        )
        .unwrap();
        assert_eq!(route.matched_custom_password, 1);
        assert_eq!(route.target_session_id, -2);
        assert_eq!(route.tcp_server_hostname, "functions3.ultraviewer.net");
        assert_eq!(route.partner_secret_token.as_deref(), Some("-1062728963"));

        let err99 = crate::protocol::format_command8_status_error(
            b"8:99:Partner's password is too easy to guess and must be changed for safety. Please tell your partner to change their password.:0",
        )
        .unwrap();
        assert_eq!(
            err99,
            "Không thể kết nối tới đối tác (Status 99, Partner's password is too easy to guess and must be changed for safety. Please tell your partner to change their password.)"
        );
        assert_eq!(
            crate::session::parse_direct_lan_target("192.168.1.10"),
            Some(("192.168.1.10".to_string(), 5938))
        );
        assert_eq!(crate::session::parse_direct_lan_target("130173637"), None);

        let tmp = std::env::temp_dir().join(format!("uv_lan_repro_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mgr = crate::profile::ProfileManager::from_custom_dir(tmp.clone());
        assert!(mgr.set_master_password(Some("12345678")).is_err());
        assert!(mgr.set_master_password(Some("UltraDesk95!")).is_ok());

        let layout = crate::screen::DisplayLayout {
            width: 64,
            height: 64,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![crate::screen::MonitorRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
        };
        let host = crate::host::HostSession::with_id(
            "136659993",
            "001122334455",
            "87654321",
            Some(crate::host::compute_master_password_proof("UltraDesk95!")),
            layout,
            crate::host::DesktopBackend::headless(64, 64),
        );
        host.set_auto_approve(true);
        let status = host.start_with_endpoint("127.0.0.1", 1, false);
        let status_json = serde_json::to_value(&status).unwrap();
        let lan_port = status_json
            .get("lan_port")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u16;
        assert!(lan_port > 0);

        let viewer = ControlSession::new();
        viewer
            .start_connect_with_remember(&format!("127.0.0.1:{}", lan_port), "UltraDesk95!", false)
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let snap = viewer.snapshot();
            if snap.state == "connected" && snap.route_mode == "lan" && snap.frames >= 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let snap = viewer.snapshot();
        assert_eq!(snap.state, "connected");
        assert_eq!(snap.route_mode, "lan");
        assert!(snap.frames >= 1);

        viewer.handle_session_payload(&crate::host::encode_host_auth_failed());
        let rejected = viewer.snapshot();
        assert_eq!(rejected.state, "disconnected");
        assert_eq!(
            rejected.error.as_deref(),
            Some("Sai mật khẩu (Wrong Password)")
        );

        {
            let mut guard = viewer.inner.lock().unwrap();
            guard.host_session = host.clone();
        }
        assert_eq!(
            viewer.resolve_lan_endpoint("136659993"),
            Some(("127.0.0.1".to_string(), lan_port))
        );

        viewer.disconnect();
        let _ = host.stop();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_establish_relay_session_populates_local_ip_and_command8_fields() {
        let reg_profile = RegistrationProfile::default();
        assert_eq!(reg_profile.terms[4], "127.0.0.1");

        let reg_terms = effective_registration_terms(&reg_profile, "192.168.11.51");
        assert_eq!(reg_terms[4], "192.168.11.51");

        let mut zero_ip_profile = reg_profile.clone();
        zero_ip_profile.terms[4] = "0.0.0.0".to_string();
        assert_eq!(
            effective_registration_terms(&zero_ip_profile, "192.168.11.51")[4],
            "192.168.11.51"
        );

        let mut custom_ip_profile = reg_profile.clone();
        custom_ip_profile.terms[4] = "10.0.0.42".to_string();
        assert_eq!(
            effective_registration_terms(&custom_ip_profile, "192.168.11.51")[4],
            "10.0.0.42"
        );

        let mut partner = PartnerProfile::default();
        partner.request_terms[1] = "90135737".to_string();
        partner.password = "12345678".to_string();
        partner.password_type = 0;

        let lookup_terms = effective_lookup_terms(
            &partner,
            134462379,
            reg_profile
                .terms
                .get(2)
                .map(String::as_str)
                .unwrap_or_default(),
        )
        .unwrap();
        assert_eq!(
            lookup_terms[2],
            connection_request_password_value("12345678", 0)
        );
        assert_eq!(lookup_terms[6], "134462379");
        assert_eq!(
            lookup_terms[7],
            crate::protocol::md5_hex_lower(b"13446237900000000")
        );
        assert_eq!(lookup_terms[12], "0");
        assert_eq!(lookup_terms[14], "1");
    }

    #[test]
    fn test_e2e_live_lan_session_and_legacy_unencrypted_fallback() {
        let layout = crate::screen::DisplayLayout {
            width: 32,
            height: 32,
            virtual_left: 0,
            virtual_top: 0,
            monitors: vec![crate::screen::MonitorRect {
                left: 0,
                top: 0,
                right: 32,
                bottom: 32,
            }],
        };
        let host = crate::host::HostSession::with_id(
            "199887766",
            "001122334455",
            "87654321",
            None,
            layout.clone(),
            crate::host::DesktopBackend::headless(32, 32),
        );
        host.set_auto_approve(true);
        let status = host.start_with_endpoint("127.0.0.1", 1, false);
        assert!(status.lan_port > 0);

        let viewer = ControlSession::new();
        viewer
            .start_connect_with_remember(
                &format!("127.0.0.1:{}", status.lan_port),
                "87654321",
                false,
            )
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            let snap = viewer.snapshot();
            if snap.state == "connected" && snap.e2e_enabled && snap.frames >= 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let v_snap = viewer.snapshot();
        let h_stat = host.status();
        assert_eq!(v_snap.state, "connected");
        assert!(v_snap.e2e_enabled);
        assert!(h_stat.e2e_enabled);
        assert_eq!(v_snap.e2e_cipher, crate::e2e::E2E_CIPHER_NAME);
        assert_eq!(h_stat.e2e_cipher, crate::e2e::E2E_CIPHER_NAME);
        assert!(!v_snap.e2e_fingerprint.is_empty());
        assert_eq!(v_snap.e2e_fingerprint, h_stat.e2e_fingerprint);

        viewer.send_chat("hello over e2e").unwrap();
        let chat_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut received_chats = Vec::new();
        while std::time::Instant::now() < chat_deadline {
            let (chats, _) = host.drain_auxiliary_events();
            if !chats.is_empty() {
                received_chats.extend(chats);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(received_chats, vec!["Partner: hello over e2e".to_string()]);

        viewer.disconnect();
        let _ = host.stop();

        let legacy_viewer = ControlSession::new();
        {
            let mut guard = legacy_viewer.inner.lock().unwrap();
            let (_, offer_state) = crate::e2e::create_viewer_offer("12345678", "proof1", "proof2");
            guard.e2e = offer_state;
        }
        let legacy_init =
            crate::host::encode_host_screen_init_with_e2e(&layout, &[0xffu8, 0xfe, 0x00, 0x80]);
        legacy_viewer.handle_session_payload(&legacy_init);
        let legacy_snap = legacy_viewer.snapshot();
        assert_eq!(legacy_snap.state, "connected");
        assert!(!legacy_snap.e2e_enabled);
        assert_eq!(legacy_snap.e2e_fingerprint, "");
    }

    #[test]
    fn test_mcp_jsonrpc_lifecycle_tools_resources_and_websocket() {
        use crate::mcp::{
            handle_mcp_raw_json, list_mcp_tools, parse_key_name_to_vk, run_stdio_mcp_loop,
            websocket_accept_key, McpServer, McpTool, McpTransportOrigin,
        };
        use serde_json::{json, Value};

        assert_eq!(
            websocket_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        assert_eq!(parse_key_name_to_vk("Enter").unwrap(), 13);
        assert_eq!(parse_key_name_to_vk("F5").unwrap(), 116);
        assert_eq!(parse_key_name_to_vk("a").unwrap(), 65);
        assert_eq!(list_mcp_tools().len(), McpTool::ALL.len());

        let session = ControlSession::new();
        {
            let mut guard = session.inner.lock().unwrap();
            let layout = crate::screen::DisplayLayout {
                width: 64,
                height: 48,
                virtual_left: 0,
                virtual_top: 0,
                monitors: vec![crate::screen::MonitorRect {
                    left: 0,
                    top: 0,
                    right: 64,
                    bottom: 48,
                }],
            };
            guard.host_session = crate::host::HostSession::new(
                "123456",
                None,
                layout,
                crate::host::DesktopBackend::headless(64, 48),
            );
        }

        let init_req = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#;
        let init_resp = handle_mcp_raw_json(&session, init_req, McpTransportOrigin::Stdio).unwrap();
        assert_eq!(init_resp["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(init_resp["result"]["serverInfo"]["name"], "ultradesk-mcp");

        let notif_req = br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert!(handle_mcp_raw_json(&session, notif_req, McpTransportOrigin::Stdio).is_none());

        let list_req = br#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
        let list_resp = handle_mcp_raw_json(&session, list_req, McpTransportOrigin::Stdio).unwrap();
        let tools_arr = list_resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools_arr.len(), 19);

        let shot_req = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "take_screenshot",
                "arguments": {
                    "source": "host",
                    "format": "png"
                }
            }
        }))
        .unwrap();
        let shot_resp =
            handle_mcp_raw_json(&session, &shot_req, McpTransportOrigin::Stdio).unwrap();
        assert_eq!(shot_resp["result"]["isError"], false);
        let content = shot_resp["result"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["mimeType"], "image/png");
        let png_bytes = decode_base64(content[1]["data"].as_str().unwrap()).unwrap();
        assert!(png_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));

        let click_req = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "mouse_click",
                "arguments": {
                    "x": 20,
                    "y": 15,
                    "button": "left",
                    "action": "double_click",
                    "target": "host"
                }
            }
        }))
        .unwrap();
        let click_resp =
            handle_mcp_raw_json(&session, &click_req, McpTransportOrigin::Stdio).unwrap();
        assert_eq!(click_resp["result"]["isError"], false);

        let key_req = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "key_press",
                "arguments": {
                    "key": "c",
                    "ctrl": true,
                    "target": "host"
                }
            }
        }))
        .unwrap();
        let key_resp = handle_mcp_raw_json(&session, &key_req, McpTransportOrigin::Stdio).unwrap();
        assert_eq!(key_resp["result"]["isError"], false);

        let res_req = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "resources/read",
            "params": {
                "uri": "ultradesk://status"
            }
        }))
        .unwrap();
        let res_resp = handle_mcp_raw_json(&session, &res_req, McpTransportOrigin::Stdio).unwrap();
        assert_eq!(
            res_resp["result"]["contents"][0]["uri"],
            "ultradesk://status"
        );

        let stdio_in = b"{\"jsonrpc\":\"2.0\",\"id\":10,\"method\":\"ping\"}\n";
        let mut stdio_out = Vec::new();
        run_stdio_mcp_loop(&session, &stdio_in[..], &mut stdio_out).unwrap();
        let stdio_parsed: Value = serde_json::from_slice(&stdio_out).unwrap();
        assert_eq!(stdio_parsed["id"], 10);
        assert_eq!(stdio_parsed["result"], json!({}));

        let (mcp_srv, listener) = McpServer::bind(
            "127.0.0.1",
            0,
            session.clone(),
            Some("secret-mcp".to_string()),
        )
        .unwrap();
        let port = mcp_srv.port;
        let shutdown = mcp_srv.shutdown.clone();
        let handle = mcp_srv.spawn(listener);

        let mut ws = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let upgrade_req = format!(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nAuthorization: Bearer secret-mcp\r\n\r\n",
            port
        );
        ws.write_all(upgrade_req.as_bytes()).unwrap();
        let mut hdr_buf = [0u8; 256];
        let n = ws.read(&mut hdr_buf).unwrap();
        let hdr_str = String::from_utf8_lossy(&hdr_buf[..n]);
        assert!(hdr_str.starts_with("HTTP/1.1 101 Switching Protocols"));
        assert!(hdr_str.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));

        let payload = br#"{"jsonrpc":"2.0","id":99,"method":"ping"}"#;
        let mut frame = vec![0x81, 0x80 | (payload.len() as u8), 0x12, 0x34, 0x56, 0x78];
        for (i, &b) in payload.iter().enumerate() {
            frame.push(b ^ frame[2 + (i & 3)]);
        }
        ws.write_all(&frame).unwrap();
        let mut reply_hdr = [0u8; 2];
        ws.read_exact(&mut reply_hdr).unwrap();
        assert_eq!(reply_hdr[0], 0x81);
        let rlen = (reply_hdr[1] & 0x7F) as usize;
        let mut reply_body = vec![0u8; rlen];
        ws.read_exact(&mut reply_body).unwrap();
        let ws_json: Value = serde_json::from_slice(&reply_body).unwrap();
        assert_eq!(ws_json["id"], 99);
        assert_eq!(ws_json["result"], json!({}));
        drop(ws);

        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = handle.join();
    }

    #[test]
    fn test_macos_inherited_nonblocking_socket_and_keep_alive_no_431() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();

        let session = ControlSession::new();
        let token = "test-viewer-token".to_string();
        let bind_addr = format!("127.0.0.1:{}", port);

        let srv_session = session.clone();
        let srv_token = token.clone();
        let srv_bind = bind_addr.clone();
        let srv_thread = std::thread::spawn(move || {
            let stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(e) => panic!("accept failed: {}", e),
                }
            };
            stream.set_nonblocking(true).unwrap();
            let _ =
                crate::server::handle_http_client(stream, srv_session, None, &srv_token, &srv_bind);
        });

        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));
        let req1 = format!(
            "GET /status HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Viewer-Token: {}\r\nConnection: keep-alive\r\n\r\n",
            port, token
        );
        client.write_all(req1.as_bytes()).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(25));
        let req2 = format!(
            "GET /status HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Viewer-Token: {}\r\nConnection: close\r\n\r\n",
            port, token
        );
        client.write_all(req2.as_bytes()).unwrap();
        let mut combined_resp = String::new();
        client.read_to_string(&mut combined_resp).unwrap();
        assert_eq!(
            combined_resp.matches("HTTP/1.1 200 OK").count(),
            2,
            "Expected two 200 OK responses on macOS inherited nonblocking keep-alive socket, got:\n{}",
            combined_resp
        );
        assert!(!combined_resp.contains("Request line too large"));
        let _ = srv_thread.join();

        let (portal, portal_listener) = PortalServer::bind("127.0.0.1", 0, session, None).unwrap();
        let pport = portal.port;
        let pshutdown = portal.shutdown.clone();
        let phandle = portal.spawn(portal_listener);

        let mut bad_client = TcpStream::connect(("127.0.0.1", pport)).unwrap();
        let oversized_line = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(9000));
        let _ = bad_client.write_all(oversized_line.as_bytes());
        let _ = bad_client.shutdown(std::net::Shutdown::Write);
        let mut bad_resp = String::new();
        let _ = bad_client.read_to_string(&mut bad_resp);
        assert!(bad_resp.starts_with("HTTP/1.1 431 Request Header Fields Too Large"));
        assert!(bad_resp.contains("Request line too large"));

        pshutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = phandle.join();
    }

    #[test]
    fn test_clipboard_sync_tick_synchronous_request_text_reentrant_borrow() {
        let clip_machine = std::rc::Rc::new(std::cell::RefCell::new(ClipboardSyncMachine::new()));
        let written = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let posted = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));

        let written_tick1 = written.clone();
        let posted_tick1 = posted.clone();
        run_clipboard_sync_tick(
            &clip_machine,
            true,
            "remote text",
            move |s| written_tick1.borrow_mut().push(s.to_string()),
            |cb| cb(Some("remote text".to_string())),
            move |post| posted_tick1.borrow_mut().push(post.to_string()),
        );
        assert_eq!(*written.borrow(), vec!["remote text".to_string()]);
        assert!(posted.borrow().is_empty());

        let written_tick2 = written.clone();
        let posted_tick2 = posted.clone();
        run_clipboard_sync_tick(
            &clip_machine,
            true,
            "remote text",
            move |s| written_tick2.borrow_mut().push(s.to_string()),
            |cb| cb(Some("local updated text".to_string())),
            move |post| posted_tick2.borrow_mut().push(post.to_string()),
        );
        assert_eq!(*written.borrow(), vec!["remote text".to_string()]);
        assert_eq!(*posted.borrow(), vec!["local updated text".to_string()]);
    }
}

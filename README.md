# ultraviewer-linux-rust

`ultraviewer-linux-rust` is a Rust remote desktop controller for UltraViewer on Linux. It implements the UltraViewer 6.6.133 registration tunnel, relay session transport, RGB555 and JPEG screen decoding, G.722 64kbps audio playback, bidirectional clipboard synchronization, and chunked file transfer.

## Run the controller

Build and start the desktop controller:

```bash
./run.sh
```

Start the HTTP controller server without opening the GTK window:

```bash
cargo run --release -- --no-gui --port 8765
```

Run the test suite:

```bash
cargo test
```

## Module layout

- `src/protocol.rs` implements 3DES registration cryptography, P2P AES-256-CBC framing, target route parsing, and byte-array field serialization.
- `src/screen.rs` decodes 11-field screen updates with scroll-copy, LRU tile caching, bottom-up JPEG atlases, and Zstandard RGB555 tiles using `8bit.dat`.
- `src/input.rs` encodes mouse movement, mouse button, mouse wheel, and keyboard packets.
- `src/audio.rs` implements the ITU-T G.722 64kbps two-band QMF ADPCM codec for packet kinds `42`, `43`, `50`, and `51`, and serves 48kHz stereo WAV audio.
- `src/clipboard.rs` encodes and decodes packet kind `33` across 2-field, 3-field, and pipe-delimited formats, bridging to `wl-copy`, `wl-paste`, `xclip`, and `xsel`.
- `src/file_transfer.rs` manages chunked file transfers and chat across packet kinds `12`, `13`, `14`, `15`, `16`, and `21`.
- `src/session.rs` coordinates registration, relay transport, and session state.
- `src/server.rs` serves `viewer.html` and the JSON and binary HTTP endpoints.

# ultradesk-rust

`ultradesk-rust` is a Linux-native remote desktop client and host written in Rust. It provides a dual-pane Spotlight UI (`viewer.html`) with WebKit2 GTK windowing (`app_window.py`) and browser fallback, X11 MIT-SHM and RandR multi-monitor capture, XTEST and Unicode `VK_PACKET` input injection, G.722 64kbps audio streaming, bidirectional clipboard sync, and chunked file transfer.

## Installation across Linux distributions

### Build from source

```bash
cargo build --release
./run.sh
```

### Arch Linux and Manjaro (`pacman` / `makepkg`)

```bash
cd packaging/arch
makepkg -si
```

### Debian, Ubuntu, and Linux Mint (`.deb`)

```bash
bash packaging/deb/build-deb.sh
sudo apt install ./dist/ultradesk-rust_0.2.0_amd64.deb
```

### Fedora, RHEL, and Rocky Linux (`.rpm`)

```bash
rpmbuild -bb packaging/rpm/ultradesk-rust.spec
```

### openSUSE Tumbleweed and Leap (`.rpm`)

```bash
rpmbuild -bb packaging/opensuse/ultradesk-rust.spec
```

### NixOS and Nix flakes

```bash
nix run .#ultradesk-rust
```

### Generic Makefile install

```bash
make build
sudo make install PREFIX=/usr
```

## CLI usage

Launch the dual-pane client and host portal:

```bash
ultradesk-rust
```

Start directly in host screen-sharing mode:

```bash
ultradesk-rust host --auto-approve
```

Print the persistent Host ID, temporary password, and XDG profile paths:

```bash
ultradesk-rust id
```

Set or clear the persistent unattended Host master password:

```bash
ultradesk-rust master-password MyStrongPass123
ultradesk-rust master-password --clear
```

Run the HTTP portal headless without opening a local GUI window:

```bash
ultradesk-rust --no-gui --port 8765
```

## Module layout

- `src/protocol.rs` implements 3DES registration cryptography, P2P AES-256-CBC framing, target route parsing, and byte-array field serialization.
- `src/screen.rs` decodes 11-field screen updates with scroll-copy, LRU tile caching, bottom-up JPEG atlases, and Zstandard RGB555 tiles.
- `src/host.rs` implements the host state machine, X11 MIT-SHM and GetImage screen capture, RandR monitor enumeration, and XTEST input injection.
- `src/profile.rs` manages XDG configuration profiles (`~/.config/ultradesk-rust`), device IDs, and master password proofs.
- `src/input.rs` encodes mouse movement, mouse button, mouse wheel, keyboard, and `VK_PACKET` Unicode packets.
- `src/audio.rs` implements the ITU-T G.722 64kbps two-band QMF ADPCM codec for packet kinds `42`, `43`, `50`, and `51`, and serves 48kHz stereo WAV audio.
- `src/clipboard.rs` encodes and decodes packet kind `33` across 2-field, 3-field, and pipe-delimited formats, bridging to `wl-copy`, `wl-paste`, `xclip`, and `xsel`.
- `src/file_transfer.rs` manages chunked file transfers and chat across packet kinds `12`, `13`, `14`, `15`, `16`, and `21`.
- `src/session.rs` coordinates registration, relay transport, and session state.
- `src/server.rs` serves `viewer.html`, embedded GUI assets, and the JSON and binary HTTP endpoints.

## Legal and trademark notice

`ultradesk-rust` is released under the [MIT License](LICENSE). It is an independent interoperability project and is not affiliated with or endorsed by DucFabulous Co., Ltd. or ULTRALAB Co., Ltd. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for third-party crate licenses and trademark details.

## UltraDesk (Rust Edition) v1.0.1

Release **v1.0.1** of **UltraDesk (Rust Edition)** (`ultradesk-rust`) adds native **macOS** (`arm64` Apple Silicon and `x86_64` Intel) support, native Cocoa / `WKWebView` GUI integration, automatic macOS TCC permission requests (`Screen Recording` and `Accessibility`), CoreGraphics (`CGEvent`) input injection, and signed `.dmg` / `.app.tar.gz` installers alongside Linux multi-distro packages.

---

### Highlights

#### 1. Dual-Mode Remote Desktop (Viewer & Host)
- **Viewer (Controller) Mode**:
  - Relay and direct LAN P2P session establishment (`Command 3` registration, `Command 8` route lookup, and automatic LAN peer discovery on UDP/TCP `5938`).
  - Fast RGB555 + Zstd dictionary-compressed tile decoding, JPEG/BMP frame decoding, and in-place dirty-rectangle blitting with LRU tile caching (`8bit.dat`).
  - Hardware cursor shape decoding (PNG & DIB bitmaps), hotspot coordinate mapping, and deduplicated LRU cursor cache.
  - Full remote input forwarding: mouse movement/buttons/wheel, binary virtual-key keyboard events, Unicode text packets (`VK_PACKET`), and special keys (`Super`, `Ctrl+Alt+Del`, `F9` monitor cycling).
- **Host (Screen Sharing) Mode**:
  - **X11 Backend**: Zero-copy MIT-SHM screen capture, RandR multi-monitor enumeration, and XTEST input injection.
  - **Wayland Backend**: Native `grim` screen capture + `/dev/uinput` virtual keyboard/mouse synthesis.
  - Ephemeral 8-digit temporary password per session and optional persistent **Master Password** for unattended access.
  - Adaptive HopToDesk-inspired video QoS and dirty-tile diffing.

#### 2. Security & Cryptography Hardening
- **End-to-End Encryption (`UDE2E1`)**:
  - Ephemeral **X25519** key exchange with RFC 7748 low-order point rejection.
  - Dual-key directional **ChaCha20-Poly1305** AEAD (`Host -> Viewer` and `Viewer -> Host`) with monotonic 64-bit sequence numbers and replay/tamper rejection.
  - Domain-separated SHA-256 temporary-password secret derivation (`|s2`) and 128-bit constant-time handshake confirmation tags.
- **Local Portal & File Transfer Protection**:
  - Per-launch 128-bit random `X-UltraDesk-Token` authentication and strict `Origin` / `Host` / `Sec-Fetch-Site` validation on the local HTTP control portal.
  - Explicit user acceptance required before writing incoming file transfers (`~/Downloads/UltraDesk`), with strict filename sanitization, size caps (`512 MiB`), and sender-side path confinement.
  - Strict `0700` config directory and `0600` atomic file permissions for XDG profiles and saved credentials.
  - Per-peer login failure throttling (`5` failed attempts -> `5`-minute lockout) and replay-protected login salts.

#### 3. Media, Clipboard, Chat & Multi-Monitor
- **Audio Streaming**: Built-in **G.722** sub-band ADPCM audio decoder and WebAudio playback toggle.
- **Bidirectional Clipboard Sync**: Loop-free state machine synchronizing Linux X11/Wayland system clipboards with remote peers.
- **Chunked File Transfer & Live Chat**: Modern and legacy wire protocol support for bidirectional file transfers and instant chat.
- **Multi-Monitor Selection**: Single monitor, monitor subset, or full virtual desktop view in both Viewer and Host modes.

#### 4. Desktop UI & Multi-Distro Packaging
- Native **GTK3 / WebKit2GTK (`tao` + `wry` + `muda`)** desktop window with automatic browser fallback (`--open-browser` / `--no-gui`).
- **8 Built-in Themes**: One Dark, One Light, VS Code Dark+, VS Code Light+, Gruvbox Dark, Gruvbox Light, Auto (System Time), and Auto (OS Theme), with bilingual **English / Vietnamese** localization.
- Automated builds and package repositories for **Debian** (`bookworm`, `trixie`, `forky`, `sid`), **Ubuntu** (`jammy`, `noble`, `questing`, `resolute`), **Fedora** (`43`, `44`, `45`, `rawhide`), **openSUSE Tumbleweed**, **Arch Linux** (`.tar.zst` / `PKGBUILD`), and **NixOS** (`flake.nix`).

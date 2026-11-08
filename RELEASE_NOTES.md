## UltraDesk (Rust Edition) v1.0.0

First stable **v1.0.0** release of **UltraDesk (Rust Edition)** (`ultradesk-rust`) — a Linux-native remote desktop controller and host written in Rust, wire-compatible with the UltraViewer 6.6.133 protocol and enhanced with end-to-end encryption (`UDE2E1`), Wayland + X11 display/input backends, and a modern Angular + native GTK/WebKit2 (`tao`/`wry`/`muda`) desktop interface.

---

### Highlights

#### 1. Dual-Mode Remote Desktop (Viewer & Host)
- **Viewer (Controller) Mode**:
  - Relay and direct LAN P2P session establishment (`Command 3` registration, `Command 8` lookup, and automatic LAN discovery on UDP/TCP `5938`).
  - Fast RGB555 + Zstd dictionary-compressed tile decoding, JPEG/BMP frame decoding, and in-place dirty-rectangle blitting with LRU tile caching (`8bit.dat`).
  - Hardware cursor shape decoding (PNG & DIB bitmaps), hotspot mapping, and deduplicated LRU cursor cache.
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
  - Explicit user acceptance required before writing incoming file transfers (`~/Downloads/UltraDesk`), with strict path sanitization, size caps (`512 MiB`), and sender-side path confinement.
  - Strict `0700` config directory and `0600` atomic file permissions for XDG profiles and saved credentials.
  - Per-peer login failure throttling (`5` failed attempts -> `5`-minute lockout) and replay-protected login salts.

#### 3. Media, Clipboard, Chat & Multi-Monitor
- **Audio Streaming**: Built-in **G.722** sub-band ADPCM audio decoder and WebAudio playback toggle.
- **Bidirectional Clipboard Sync**: Loop-free state machine syncing Linux X11/Wayland system clipboards with remote peers.
- **Chunked File Transfer & Live Chat**: Modern and legacy wire protocol support for bidirectional file transfers and instant chat.
- **Multi-Monitor Selection**: Single monitor, monitor subset, or full virtual desktop view in both Viewer and Host modes.

#### 4. Desktop UI & Multi-Distro Packaging
- Native **GTK3 / WebKit2GTK (`tao` + `wry` + `muda`)** desktop window with automatic browser fallback (`--open-browser` / `--no-gui`).
- **8 Built-in Themes**: One Dark, One Light, VS Code Dark+, VS Code Light+, Gruvbox Dark, Gruvbox Light, Auto (System Time), and Auto (OS Theme), with bilingual **English / Tiếng Việt** localization.
- Automated builds and repositories for **Debian** (`bookworm`, `trixie`, `forky`, `sid`), **Ubuntu** (`jammy`, `noble`, `questing`, `resolute`), **Fedora** (`43`, `44`, `45`, `rawhide`), **openSUSE Tumbleweed**, **Arch Linux** (`.tar.zst` / `PKGBUILD`), and **NixOS** (`flake.nix`).

---

### Tóm tắt bản phát hành v1.0.0 (Tiếng Việt)

- **Hỗ trợ song song 2 chiều (Điều khiển & Chia sẻ màn hình)**:
  - Kết nối qua máy chủ điều phối UltraViewer (`functions3.ultraviewer.net`) hoặc trực tiếp qua mạng LAN (`UDP/TCP 5938`).
  - Hỗ trợ đầy đủ cả **Wayland** (`grim` + `/dev/uinput`) và **X11** (`MIT-SHM` + `XTEST`), chọn từng màn hình hoặc nhiều màn hình cùng lúc.
- **Bảo mật & Mã hóa đầu-cuối (E2E)**:
  - Mã hóa đầu-cuối **X25519 + ChaCha20-Poly1305 (`UDE2E1`)** giữa các phiên UltraDesk.
  - Hoàn thành kiểm toán bảo mật toàn diện (22 bản vá bảo mật cho giao thức, xác thực Portal cục bộ, truyền tệp an toàn, phân quyền hồ sơ `0600`/`0700` và hardening CI/CD).
- **Đầy đủ tiện ích điều khiển từ xa**:
  - Truyền âm thanh **G.722**, đồng bộ Clipboard hai chiều, truyền file có xác nhận và khung Chat trực tiếp.

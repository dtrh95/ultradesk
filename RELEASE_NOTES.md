## UltraDesk (Rust Edition) v1.0.4

Release **v1.0.4** of **UltraDesk (Rust Edition)** (`ultradesk-rust`) upgrades core cryptography, compression, and X11 protocol dependencies (`aes 0.9.4`, `cbc 0.2.1`, `cipher 0.5.2`, `des 0.9.0`, `md-5 0.11.0`, `sha2 0.11.1`, `x11rb 0.14.0`, and `zstd 0.14.3`), updates CI/CD actions (`actions/setup-node@v7`, `cloudflare/wrangler-action@v4`, `cachix/cachix-action@v17`), fixes Ubuntu Launchpad PPA source package generation, and highlights direct bidirectional compatibility with **UltraViewer on Windows**.

---

### Highlights

#### 1. Built-in AI Model Context Protocol (MCP) Server
- **Three Native Transports**:
  - `stdio` JSON-RPC 2.0 via `ultradesk-rust mcp` for zero-configuration integration with Claude Desktop, Cursor, Claude Code, and OpenCode.
  - Local WebSocket and HTTP server on `127.0.0.1:9333` (`--mcp` or `--mcp-port <PORT>`) with optional `--mcp-token` bearer authentication.
  - Integrated `POST /mcp` and `GET /mcp/tools` routes on the local `PortalServer`.
- **19 Remote & Host Desktop Tools**:
  - Session & display control: `get_session_status`, `connect_remote`, `disconnect_remote`, `list_monitors`, and `select_monitor`.
  - Visual screenshot capture: `take_screenshot` returning base64 PNG or JPEG `image` content blocks from the connected remote partner or local host desktop.
  - Pointer & keyboard automation: `mouse_move`, `mouse_click` (`click`, `double_click`, `down`, `up`), `mouse_drag`, `mouse_scroll`, `type_text` (`VK_PACKET` Unicode), and `key_press` (named keys, function keys, and `ctrl`/`alt`/`shift`/`meta` shortcuts).
  - Session tools & host management: `get_clipboard`, `set_clipboard`, `send_chat`, `list_files`, `send_file`, `manage_file_transfer`, and `manage_host`.
- **4 MCP Resources**: `ultradesk://status`, `ultradesk://host/status`, `ultradesk://clipboard`, and `ultradesk://screen/current.png`.

#### 2. Dual-Mode Remote Desktop (Viewer & Host)
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

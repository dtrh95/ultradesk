# ultradesk-rust

`ultradesk-rust` is a Linux-native remote desktop client and host written in Rust. It provides a dual-pane Spotlight UI (`viewer.html`) inside a native Tauri (tao + wry + muda) desktop window with automatic browser fallback, multi-monitor capture and selection on both Wayland and X11, kernel `uinput` and X11 `XTEST` input injection, ITU-T G.722 64 kbps audio streaming, bidirectional chat and clipboard synchronization, and chunked file transfer.

## Features

- **Dual-role client and host in one binary.** Connect to a remote partner or share your Linux desktop from the same window.
- **Wayland and X11 display backends.** Captures screens on Wayland compositors (`niri`, `sway`, `Hyprland`, and `wlroots`) through `grim` and `wlr-randr`, and on X11 through `MIT-SHM`, `XGetImage`, and `RandR`.
- **Kernel and X11 input injection.** Injects relative and absolute pointer motion, mouse buttons, scroll wheels, and keyboard events through `/dev/uinput` on Wayland and `XTEST` on X11, plus `VK_PACKET` UTF-16LE keystrokes for full Vietnamese and international Unicode input.
- **Multi-monitor selection in both directions.** View or share all monitors at once, a single monitor, or any custom subset of monitors (`MonitorSelection::All`, `MonitorSelection::Index`, and `MonitorSelection::Subset`).
- **Adaptive multi-theme Spotlight UI.** Includes `Auto (System Time)` (switches between `VS Code Light+` from `06:00` to `17:59` and `One Dark Pro` from `18:00` to `05:59`), `Auto (OS Theme)` (`prefers-color-scheme`), `One Dark Pro`, `One Light`, `VS Code Dark+`, `VS Code Light+`, `Gruvbox Dark`, and `Gruvbox Light`.
- **Low-latency RGB555 and JPEG tile engine.** Pre-allocates the 65,536-entry RGB555 lookup table and reuses a prepared Zstandard dictionary (`ZstdDictionaryDecoder`) across 11-field screen updates, scroll-copy rectangles, and bottom-up JPEG atlases.
- **Bidirectional session tools.** Supports real-time UTF-16LE chat (`PACKET_CHAT_MESSAGE`), system clipboard synchronization (`wl-copy`, `wl-paste`, `xclip`, `xsel`, and GTK3 clipboard fallback), G.722 64 kbps PC and microphone audio (`48 kHz` stereo WAV stream), and resumable chunked file transfers.
- **Unattended access and brute-force protection.** Supports a persistent 9-digit Host ID, temporary session passwords, an optional persistent master password (`8` to `64` characters), and automatic 5-failure login lockout (`LoginThrottle`).

## Install on Linux distributions

### Build from source

Install a Rust toolchain (`cargo` and `rustc`), `pkg-config`, `libx11-dev`, `libxext-dev`, and `libxtst-dev`. Then build and launch the release binary:

```bash
cargo build --release
./run.sh
```

To install the binary, desktop launcher, AppStream metadata, and icons system-wide through the `Makefile`:

```bash
make build
sudo make install PREFIX=/usr
```

### Arch Linux and Manjaro (`AUR` and `makepkg`)

Install from the Arch User Repository with an AUR helper such as `yay` or `paru`:

```bash
yay -S ultradesk-rust
```

To install the prebuilt binary package without compiling Rust from source:

```bash
yay -S ultradesk-rust-bin
```

To build locally from the repository `PKGBUILD`:

```bash
cd packaging/arch
makepkg -si
```

### Debian (`bookworm`, `trixie`, `forky`, and `sid`)

Build a binary `.deb` package locally with `dpkg-buildpackage`:

```bash
cp -r packaging/debian debian
dpkg-buildpackage -us -uc -b
sudo apt install ../ultradesk-rust_0.2.0-1_amd64.deb
```

### Ubuntu and Linux Mint (`jammy`, `noble`, `questing`, and `resolute`)

Install from the signed APT repository or build the `.deb` package locally with the commands in the Debian section above. Runtime dependencies on Debian and Ubuntu include `libgtk-3-0`, `libwebkit2gtk-4.1-0`, `libsoup-3.0-0`, `xclip`, and `wl-clipboard`.

### Fedora, RHEL, AlmaLinux, and Rocky Linux (`.rpm`)

Build and install the RPM package using `rpmbuild` and `packaging/rpm/fedora/ultradesk-rust.spec`:

```bash
rpmbuild -bb packaging/rpm/fedora/ultradesk-rust.spec
sudo dnf install ~/rpmbuild/RPMS/x86_64/ultradesk-rust-0.2.0-1*.rpm
```

### openSUSE Tumbleweed and Leap (`.rpm`)

Build and install the RPM package using `packaging/rpm/opensuse/ultradesk-rust.spec`:

```bash
rpmbuild -bb packaging/rpm/opensuse/ultradesk-rust.spec
sudo zypper install ~/rpmbuild/RPMS/x86_64/ultradesk-rust-0.2.0-1*.rpm
```

### NixOS and Nix flakes

Run `ultradesk-rust` directly from the repository flake:

```bash
nix run .#ultradesk-rust
```

To build the derivation from the current working tree:

```bash
nix build -L .#packages.x86_64-linux.ultradesk-rust-head
```

To enable `ultradesk-rust` in a NixOS system configuration, import `nixosModules.ultradesk-rust` from `flake.nix` and enable the program option:

```nix
{
  programs.ultradesk-rust.enable = true;
}
```

## Configure display and input backends

`ultradesk-rust` selects the best host capture and input backend automatically at startup (`select_desktop_backend` in `src/host.rs`):

1. **`WaylandGrimUinputBackend` (Wayland sessions).** Uses `grim -t ppm -s 1 -` for screen capture and `/dev/uinput` for kernel-level mouse and keyboard injection. It enumerates monitors through `niri msg -j outputs`, `wlr-randr`, or `xrandr`.
2. **`X11ShmXtestBackend` (X11 and Xwayland sessions).** Uses `XShmGetImage` with `XGetImage` fallback for screen capture, `XRRGetMonitors` (`RandR` 1.5) for monitor enumeration, and `XTestFakeMotionEvent`, `XTestFakeButtonEvent`, and `XTestFakeKeyEvent` for input injection.
3. **`HeadlessBackend` (headless servers and CI).** Generates synthetic frames and accepts input no-ops when neither Wayland nor X11 is available.

To allow non-root input injection on Wayland through `/dev/uinput`, ensure the `uinput` kernel module is loaded and your user has write access to `/dev/uinput`:

```bash
sudo modprobe uinput
sudo setfacl -m u:$USER:rw /dev/uinput
```

## Run from the command line

Launch the dual-pane client and host portal window:

```bash
ultradesk-rust
```

Start directly in host screen-sharing mode and automatically accept incoming connections:

```bash
ultradesk-rust host --auto-approve
```

Print the live 9-digit Host ID, temporary password, hardware MAC, synthetic client MAC, and XDG profile path:

```bash
ultradesk-rust id
```

Set or clear the persistent unattended Host master password (`8` to `64` characters):

```bash
ultradesk-rust master-password MyStrongPass123
ultradesk-rust master-password --clear
```

Auto-connect to a remote partner on startup:

```bash
ultradesk-rust --partner-id 130173637 --password 5kicte
```

Run the HTTP portal headless without opening a local GUI window:

```bash
ultradesk-rust --no-gui --host 127.0.0.1 --port 33411
```

### Command-line options

| Subcommand or flag | Description |
| :--- | :--- |
| `host` | Start in host screen-sharing mode. |
| `id` | Query and print the live Host ID, temporary password, and profile metadata. |
| `master-password <PW>` | Save a persistent master password for unattended host access. |
| `master-password --clear` | Clear the saved persistent master password. |
| `--host <ADDR>` | Bind address for the local HTTP portal server (default: `127.0.0.1`). |
| `--port <PORT>` | Bind port for the local HTTP portal server (`0` selects an available port). |
| `--partner-id <ID>` | Partner ID to connect to on startup. |
| `--password <PW>` | Partner password for startup connection. |
| `--auto-approve` | Automatically approve incoming host connections. |
| `--no-gui` | Run the HTTP portal daemon without launching WebKit2 or a browser. |
| `--open-browser` | Open the portal URL in the default system web browser on startup. |
| `--registration <PATH>` | Override path to `linux-controller-private.json`. |
| `--partner <PATH>` | Override path to `default-partner.json`. |
| `--dictionary <PATH>` | Override path to the Zstandard dictionary (`8bit.dat`). |

## Select monitors in multi-display sessions

Both the remote viewer and the local host support three display selection modes:

- **All monitors (`ALL`).** Streams or displays the combined virtual desktop across every connected display.
- **Single monitor (`M1`, `M2`, ...).** Crops the view or host stream to one monitor and translates mouse coordinates to that monitor's origin (`virtual_left` and `virtual_top`).
- **Monitor subset (shift-click or Ctrl-click on `M1`, `M2`, ...).** Combines a custom subset of monitors into a bounding virtual desktop.

In the Spotlight UI (`viewer.html`), click `ALL` to view or share all displays, click `M1` or `M2` to switch to a single display, or hold `Shift` or `Ctrl` while clicking monitor buttons to toggle a multi-monitor subset.

## HTTP portal API reference

`PortalServer` (`src/server.rs`) exposes a local HTTP API consumed by `viewer.html` and external automation scripts:

| Method | Path | Description |
| :--- | :--- | :--- |
| `GET` | `/` | Serve the Spotlight dual-pane UI (`viewer.html`). |
| `GET` | `/status` | Return the full `SessionState` JSON snapshot (client, host, monitors, cursor, audio, clipboard, files, and chat). |
| `GET` | `/frame.bmp?v=<ver>` | Return the active remote view as an uncompressed 24-bit BMP image (`304 Not Modified` when `v` matches `X-Frame-Version`). |
| `GET` | `/frame.jpg?v=<ver>&q=<1..100>` | Return the active remote view as a JPEG image (`304 Not Modified` when unchanged). |
| `GET` | `/frame.png?v=<ver>` | Return the active remote view as a PNG image (`304 Not Modified` when unchanged). |
| `POST` | `/connect` | Connect to a remote partner (`{"partner_id":"...","password":"...","remember":true}`). |
| `POST` | `/disconnect` | Disconnect the active remote control session. |
| `POST` | `/event` | Send mouse or keyboard input (`{"type":"move","x":100,"y":200}`, `{"type":"button","button":"left","down":true}`, or `{"type":"key","vk":65,"down":true}`). |
| `POST` | `/text` | Send Unicode text via `VK_PACKET` keystrokes (`{"text":"Tiếng Việt","caps_lock":false}`). |
| `POST` | `/monitor` | Select remote client monitor view (`{"index":null}` for all, `{"index":0}` for single, or `{"indices":[0,1]}` for a subset). |
| `GET` | `/host/status` | Return the current `HostStatus` JSON object. |
| `POST` | `/host/start` | Start the local Host registration and relay listener worker. |
| `POST` | `/host/stop` | Stop the local Host session and disconnect any active viewer. |
| `POST` | `/host/monitor` | Select which local host monitors to share (`{"index":null}`, `{"index":1}`, or `{"indices":[0,1]}`). |
| `POST` | `/host/approve` | Approve a pending viewer connection (`{"allow_control":true}`). |
| `POST` | `/host/decline` | Decline a pending viewer connection. |
| `POST` | `/host/control` | Toggle remote input control permission for the connected viewer (`{"allow_control":false}`). |
| `POST` | `/host/master-password` | Set or clear the persistent unattended master password (`{"password":"..."}`). |
| `POST` | `/chat` | Send a chat message to the connected partner (`{"text":"hello"}`). |
| `GET` | `/clipboard` | Return local and remote clipboard state and counters. |
| `POST` | `/clipboard` | Send clipboard text to the partner (`{"text":"..."}` or `{"sync_from_system":true}`). |
| `GET` | `/files` | List active file transfers and chat history. |
| `POST` | `/files/send` | Offer a file to the partner by path (`{"file_path":"/path/to/file"}`) or base64 payload (`{"file_name":"a.txt","data_base64":"..."}`). |
| `POST` | `/files/accept` | Accept an incoming file transfer (`{"file_id":"..."}`). |
| `POST` | `/files/cancel` | Cancel an active file transfer (`{"file_id":"..."}`). |
| `GET` | `/files/download?file_id=<id>` | Download a completed incoming file transfer. |
| `POST` | `/audio/toggle` | Enable or disable remote computer sound (`{"enable":true}`). |
| `GET` | `/audio.wav?after=<seq>` | Stream decoded 48 kHz stereo PCM audio as WAV (`X-Audio-Seq` response header). |

## Repository layout

- `src/main.rs` parses CLI subcommands (`host`, `id`, `master-password`, and `help`), loads XDG profiles, starts `HostSession` and `PortalServer`, and launches the native Tauri desktop window or browser fallback.
- `src/gui.rs` implements the native desktop window runtime with Tauri `tao` and `wry`, native menubar integration via `muda`, bidirectional clipboard synchronization, and browser fallback.
- `src/lib.rs` exports all crate modules and houses the end-to-end integration test suite.
- `src/protocol.rs` implements 3DES registration cryptography, P2P AES-256-CBC framing, target route parsing, and multipart byte-array field serialization.
- `src/screen.rs` implements `RGB555Screen`, `DisplayLayout`, `MonitorSelection`, 11-field screen update decoding, scroll-copy, LRU tile caching, bottom-up JPEG atlases, and Zstandard RGB555 decompression.
- `src/host.rs` implements `HostCore`, `HostSession`, `WaylandGrimCapturer`, `UinputInjector`, `X11Capturer`, `X11Injector`, multi-monitor layout cropping, input coordinate translation, and `LoginThrottle`.
- `src/session.rs` coordinates client registration, relay transport, keepalives, clipboard polling, and unified `SessionState` synchronization across client and host modes.
- `src/server.rs` implements `PortalServer`, serves `viewer.html` and embedded assets, and handles all JSON and binary HTTP routes.
- `src/input.rs` encodes mouse movement, mouse button, mouse wheel, keyboard (`WM_KEYDOWN` and `WM_KEYUP`), and `VK_PACKET` Unicode packets, and decodes incoming host input commands.
- `src/cursor.rs` decodes remote cursor shape updates (`PACKET_CURSOR_CHANGE`, `PACKET_CURSOR_IMAGE`, and `PACKET_CURSOR_POS`), caches PNG cursors by hash, and requests missing shapes.
- `src/audio.rs` implements the ITU-T G.722 64 kbps two-band QMF ADPCM codec for packet kinds `42`, `43`, `50`, and `51`, and builds 48 kHz stereo WAV buffers.
- `src/clipboard.rs` encodes and decodes packet kinds `5` and `33` across 1-byte and 2-byte `Int16LE` tags, Zstandard-compressed UTF-16LE streams, and pipe-delimited strings, bridging to `wl-copy`, `wl-paste`, `xclip`, `xsel`, and GTK3.
- `src/file_transfer.rs` manages chunked file transfers and bidirectional UTF-16LE chat across packet kinds `12`, `13`, `14`, `15`, `16`, and `21`.
- `src/direct.rs` implements LAN direct TCP candidate exchange and probing (`DirectHandshakeMachine`).
- `src/profile.rs` manages XDG configuration files under `~/.config/ultradesk-rust` (`host-profile.json`, `client-profile.json`, and `saved_credentials.json`).
- `viewer.html` provides the bilingual (Vietnamese and English) Spotlight UI with a docked top menu toolbar, multi-theme switching, and session tools.

## Run tests and benchmarks

Run the 47 release unit and integration tests and the verification script (`scripts/verify_upgrade.sh`):

```bash
cargo test --release
bash scripts/verify_upgrade.sh
```

The test suite verifies 3DES and AES-256-CBC cryptography, 11-field screen decoding, multi-monitor subset cropping, Wayland `grim` PPM parsing, X11 `ZPixmap` conversion, G.722 audio encoding and decoding, bidirectional `Int16LE` chat and clipboard synchronization, chunked file transfer, and live loopback Host-to-Viewer relay handshakes.

## Automated release pipelines

GitHub Actions workflows under `.github/workflows/` automate building, testing, and publishing across distributions:

- **`.github/workflows/build.yml` and `.github/workflows/nix.yml`** run `cargo test --release` and `nix flake check` on every push and pull request.
- **`.github/workflows/build-and-release.yml`** builds release packages for Debian, Fedora, openSUSE, Arch Linux, and NixOS, publishes the signed GitHub Pages APT, DNF, and Zypper repositories, and attaches artifacts to GitHub Releases.
- **`.github/workflows/publish-debian.yml`** builds `.deb` packages and signed APT repositories for Debian `bookworm`, `trixie`, `forky`, and `sid`.
- **`.github/workflows/publish-ubuntu.yml`** builds `.deb` packages and signed APT repositories for Ubuntu `jammy` (`22.04`), `noble` (`24.04`), `questing` (`25.10`), and `resolute` (`26.04`), and uploads signed source packages to Ubuntu Launchpad PPA.
- **`.github/workflows/publish-aur.yml`** updates both `ultradesk-rust` (source) and `ultradesk-rust-bin` (prebuilt binary) on the Arch User Repository (`aur.archlinux.org`).
- **`.github/workflows/publish-nixos.yml` and `.github/workflows/update-nix-hash.yaml`** build the Nix flake closure, push binary substitutes to Cachix, export portable `.nar.xz` bundles, and open a pull request updating `nix/packages/ultradesk-rust/default.nix`.

## License and trademark notice

`ultradesk-rust` is released under the [MIT License](LICENSE). It is an independent interoperability project and is not affiliated with or endorsed by DucFabulous Co., Ltd. or ULTRALAB Co., Ltd. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for third-party crate licenses and trademark details.

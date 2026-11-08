# ultradesk-rust

`ultradesk-rust` is a native remote desktop client and host for Linux and macOS written in Rust. You can connect to a remote computer or share your local desktop from a single window. The application renders its dual-pane UI (`viewer.html`) inside a native desktop window powered by `tao`, `wry`, and `muda` (using GTK3/WebKit2GTK on Linux and Cocoa/`WKWebView` on macOS), falling back to an external browser if webview initialization fails. Under the hood, it captures screens on Wayland, X11, and macOS, injects kernel `uinput`, X11 `XTEST`, and macOS `CoreGraphics` (`CGEvent`) input events, negotiates end-to-end encryption (`UDE2E1`), streams 64 kbps ITU-T G.722 audio, synchronizes clipboards, and transfers files in chunks.

## Features

A single binary handles both client and host sessions. You can connect to a remote partner using their 9-digit partner ID and password, or let a remote operator control your machine. Unattended access uses a persistent master password of 8 to 64 characters alongside a temporary session password. To prevent brute-force attacks, the host locks out incoming requests after 5 failed login attempts.

Screen capture and input injection adapt to your operating system and display server:
- **Linux Wayland** (`niri`, `sway`, `Hyprland`, and `wlroots` compositors): Captures frames through `grim`, queries monitor geometry with `niri msg -j outputs` or `wlr-randr`, and injects pointer and keyboard events via `/dev/uinput`.
- **Linux X11**: Uses `MIT-SHM` shared memory with `XGetImage` fallback, enumerates monitors via `RandR`, and injects input through `XTEST`.
- **macOS (`arm64` Apple Silicon & `x86_64` Intel)**: Captures the screen via `/usr/sbin/screencapture`, automatically prompts for **Screen Recording** (`CGRequestScreenCaptureAccess`) and **Accessibility** (`AXIsProcessTrustedWithOptions`) permissions, and injects mouse movements, clicks, scroll wheel, virtual keys, and Unicode `VK_PACKET` text via `CoreGraphics` (`CGEventPost`).

The rendering pipeline decodes 11-field screen packets with a pre-allocated 65,536-entry RGB555 lookup table. A prepared Zstandard dictionary decodes legacy 16-bit tiles without re-parsing dictionary headers on every packet, while scroll-copy rects and bottom-up JPEG atlases reduce bandwidth on fast-moving content. Multi-monitor setups let you stream or view all displays together, focus on a single display, or select any subset of screens.

Session tools run alongside the video stream. You can exchange real-time chat messages, transfer files in chunks, and stream 48 kHz stereo audio decoded through an ITU-T G.722 64 kbps ADPCM codec. Clipboard synchronization monitors both local and remote changes through `pbcopy`/`pbpaste` on macOS and `wl-clipboard`, `xclip`, `xsel`, or GTK on Linux, preventing echo loops through deduplication. The interface offers eight color palettes, switching automatically between light and dark modes based on system time or OS color scheme preferences.

## Install on Linux and macOS

### 1. Debian (`bookworm`, `trixie`, `forky`, `sid`)

#### Option A: Install prebuilt `.deb` from GitHub Releases
Download the package matching your Debian codename (`bookworm`, `trixie`, `forky`, or `sid`) from the [Releases](https://github.com/dtrh95/ultradesk/releases) page:

```bash
CODENAME=$(. /etc/os-release && echo "${VERSION_CODENAME:-bookworm}")
curl -fLO "https://github.com/dtrh95/ultradesk/releases/latest/download/ultradesk-rust_1.0.1-1_amd64_${CODENAME}.deb"
sudo apt install -y "./ultradesk-rust_1.0.1-1_amd64_${CODENAME}.deb"
```

#### Option B: Install via signed APT repository
```bash
CODENAME=$(. /etc/os-release && echo "${VERSION_CODENAME:-bookworm}")
sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://ultradesk-rust.pages.dev/pubkey.gpg | sudo tee /etc/apt/keyrings/ultradesk-rust.asc >/dev/null
echo "deb [arch=amd64 signed-by=/etc/apt/keyrings/ultradesk-rust.asc] https://ultradesk-rust.pages.dev/apt/${CODENAME} ${CODENAME} main" | sudo tee /etc/apt/sources.list.d/ultradesk-rust.list >/dev/null
sudo apt update
sudo apt install -y ultradesk-rust
```

#### Option C: Build `.deb` locally from source
```bash
cp -r packaging/debian debian
dpkg-buildpackage -us -uc -b
sudo apt install -y ../ultradesk-rust_1.0.1-1_amd64.deb
```

---

### 2. Ubuntu, Linux Mint, and Pop!_OS (`jammy`, `noble`, `questing`, `resolute`)

#### Option A: Install prebuilt `.deb` from GitHub Releases
```bash
CODENAME=$(. /etc/os-release && echo "${UBUNTU_CODENAME:-${VERSION_CODENAME:-noble}}")
curl -fLO "https://github.com/dtrh95/ultradesk/releases/latest/download/ultradesk-rust_1.0.1-1_amd64_${CODENAME}.deb"
sudo apt install -y "./ultradesk-rust_1.0.1-1_amd64_${CODENAME}.deb"
```

#### Option B: Install via signed Ubuntu APT repository
```bash
CODENAME=$(. /etc/os-release && echo "${UBUNTU_CODENAME:-${VERSION_CODENAME:-noble}}")
sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://ultradesk-rust.pages.dev/pubkey.gpg | sudo tee /etc/apt/keyrings/ultradesk-rust.asc >/dev/null
echo "deb [arch=amd64 signed-by=/etc/apt/keyrings/ultradesk-rust.asc] https://ultradesk-rust.pages.dev/apt/${CODENAME} ${CODENAME} main" | sudo tee /etc/apt/sources.list.d/ultradesk-rust.list >/dev/null
sudo apt update
sudo apt install -y ultradesk-rust
```

---

### 3. Fedora, RHEL, AlmaLinux, and Rocky Linux (`43`, `44`, `45`, `rawhide`)

#### Option A: Install prebuilt `.rpm` from GitHub Releases
```bash
RELEASEVER=$(rpm -E %fedora)
sudo dnf install -y "https://github.com/dtrh95/ultradesk/releases/latest/download/ultradesk-rust-1.0.1-1.fc${RELEASEVER}.x86_64.${RELEASEVER}.rpm"
```

#### Option B: Build `.rpm` locally with `rpmbuild`
```bash
sudo dnf install -y rpm-build rpmdevtools dnf-plugins-core
rpmdev-setuptree
cp packaging/rpm/fedora/ultradesk-rust.spec ~/rpmbuild/SPECS/
sudo dnf builddep -y ~/rpmbuild/SPECS/ultradesk-rust.spec
tar --exclude='.git' --exclude='packaging' --exclude='target' \
    --transform 's|^\.|ultradesk-rust-1.0.1|' \
    -czf ~/rpmbuild/SOURCES/ultradesk-rust-1.0.1.tar.gz .
rpmbuild -bb ~/rpmbuild/SPECS/ultradesk-rust.spec
sudo dnf install -y ~/rpmbuild/RPMS/x86_64/ultradesk-rust-1.0.1-1*.rpm
```

---

### 4. openSUSE Tumbleweed and Leap (`.rpm`)

#### Option A: Install prebuilt `.rpm` from GitHub Releases
```bash
sudo zypper install -y "https://github.com/dtrh95/ultradesk/releases/latest/download/ultradesk-rust-1.0.1-1.x86_64.tumbleweed.rpm"
```

#### Option B: Build `.rpm` locally with `rpmbuild`
```bash
sudo zypper install -y rpm-build rpmdevtools
rpmdev-setuptree
cp packaging/rpm/opensuse/ultradesk-rust.spec ~/rpmbuild/SPECS/
tar --exclude='.git' --exclude='packaging' --exclude='target' \
    --transform 's|^\.|ultradesk-rust-1.0.1|' \
    -czf ~/rpmbuild/SOURCES/ultradesk-rust-1.0.1.tar.gz .
rpmbuild -bb ~/rpmbuild/SPECS/ultradesk-rust.spec
sudo zypper install -y ~/rpmbuild/RPMS/x86_64/ultradesk-rust-1.0.1-1*.rpm
```

---

### 5. Arch Linux, Manjaro, and EndeavourOS

#### Option A: Build and install with `makepkg` (`PKGBUILD`)
```bash
cd packaging/arch
makepkg -si
```

#### Option B: Extract prebuilt Arch archive from GitHub Releases
```bash
curl -fLO "https://github.com/dtrh95/ultradesk/releases/latest/download/ultradesk-rust-v1.0.1-x86_64-archlinux.tar.zst"
sudo pacman -S --needed --noconfirm openssl libxcb zstd gtk3 webkit2gtk-4.1 libsoup3
sudo tar --zstd -xvf ultradesk-rust-v1.0.1-x86_64-archlinux.tar.zst -C /
```

---

### 6. macOS (`arm64` Apple Silicon and `x86_64` Intel)

#### Option A: Install via `.dmg` disk image from GitHub Releases
1. Download the `.dmg` matching your Mac architecture from the [Releases](https://github.com/dtrh95/ultradesk/releases) page:
   - **Apple Silicon (`M1` / `M2` / `M3` / `M4`)**: `ultradesk-rust-v1.0.1-arm64-macos.dmg`
   - **Intel Mac (`x86_64`)**: `ultradesk-rust-v1.0.1-x86_64-macos.dmg`
2. Open the `.dmg` file and drag **UltraDesk.app** into the **Applications** folder, or install directly from Terminal:

```bash
ARCH=$(uname -m)
curl -fLO "https://github.com/dtrh95/ultradesk/releases/latest/download/ultradesk-rust-v1.0.1-${ARCH}-macos.dmg"
hdiutil attach "ultradesk-rust-v1.0.1-${ARCH}-macos.dmg"
cp -R "/Volumes/UltraDesk 1.0.1 (${ARCH})/UltraDesk.app" /Applications/
hdiutil detach "/Volumes/UltraDesk 1.0.1 (${ARCH})"
xattr -dr com.apple.quarantine /Applications/UltraDesk.app
open /Applications/UltraDesk.app
```

#### Option B: Extract prebuilt `.app.tar.gz` bundle
```bash
ARCH=$(uname -m)
curl -fLO "https://github.com/dtrh95/ultradesk/releases/latest/download/ultradesk-rust-v1.0.1-${ARCH}-macos.app.tar.gz"
sudo tar -xzf "ultradesk-rust-v1.0.1-${ARCH}-macos.app.tar.gz" -C /Applications
xattr -dr com.apple.quarantine /Applications/UltraDesk.app
open /Applications/UltraDesk.app
```

#### Option C: Build `UltraDesk.app` and `.dmg` locally from source on macOS
Install Xcode Command Line Tools (`xcode-select --install`), a Rust toolchain (`rustup`), and Node.js 22:

```bash
ARCH=$(uname -m)
TARGET=$(rustc -vV | sed -n 's/host: //p')
cd web && npm ci && npm run build && cd ..
cargo build --release --target "${TARGET}"
./packaging/macos/package-macos.sh "${TARGET}" "${ARCH}" "v1.0.1"
```

#### Granting macOS system permissions (Screen Recording & Accessibility)
When you start Host screen sharing on macOS, `UltraDesk.app` automatically checks and prompts for two system permissions required by macOS TCC:
1. **Screen Recording** (`System Settings -> Privacy & Security -> Screen & System Audio Recording`): Required so remote partners can view your screen.
2. **Accessibility** (`System Settings -> Privacy & Security -> Accessibility`): Required so remote partners can move your mouse cursor and type on your keyboard via `CoreGraphics` (`CGEvent`).

If you upgrade `UltraDesk.app` and macOS still uses a cached permission entry from an older ad-hoc signature, reset the TCC entries for `io.github.ultradesk.UltraDesk` and relaunch the app:

```bash
tccutil reset ScreenCapture io.github.ultradesk.UltraDesk
tccutil reset Accessibility io.github.ultradesk.UltraDesk
open /Applications/UltraDesk.app
```

---

### 7. NixOS and Nix Flakes

Run `ultradesk-rust` directly using Nix Flakes:

```bash
nix run github:dtrh95/ultradesk#ultradesk-rust-head
```

Or build from a local checkout:

```bash
nix build -L .#packages.x86_64-linux.ultradesk-rust-head
```

To enable `ultradesk-rust` declaratively in a NixOS configuration, import `nixosModules.ultradesk-rust` from `flake.nix`:

```nix
{
  programs.ultradesk-rust.enable = true;
}
```

---

### 8. Build from source (Any Linux distribution)

Install a Rust toolchain (`cargo` and `rustc`), `pkg-config`, `openssl`, `libxcb`, `zstd`, `gtk3`, `webkit2gtk-4.1`, and `libsoup-3.0`.

Build and install system-wide (`/usr/bin/ultradesk-rust`):

```bash
make build
sudo make install PREFIX=/usr
```

Or install into your user profile (`~/.local/bin/ultradesk-rust`) without root privileges:

```bash
make build
make install PREFIX="$HOME/.local"
```

## Configure display and input backends

When the host worker starts, `DesktopBackend::auto_detect` in `src/host.rs` probes your session to pick a capture and input engine:
- **macOS (`MacOsScreencaptureBackend`)**: Calls `ensure_macos_host_permissions(true)` to verify or request Screen Recording and Accessibility access, captures screen frames via `/usr/sbin/screencapture -x -t png`, and injects mouse, scroll wheel, virtual-key, and Unicode `VK_PACKET` events directly through `CoreGraphics.framework` (`CGEventPost`).
- **Linux Wayland (`WaylandGrimUinputBackend`)**: Captures display buffers using `grim -t ppm -s 1 -` and enumerates connected outputs through `niri msg -j outputs`, `wlr-randr`, or `xrandr`. It injects mouse movements and keyboard events directly into the kernel via `/dev/uinput`.
- **Linux X11 and Xwayland (`X11ShmXtestBackend`)**: Captures frames through `XShmGetImage` with an `XGetImage` fallback, queries monitor layouts via `XRRGetMonitors` (RandR 1.5), and injects input through `XTestFakeMotionEvent`, `XTestFakeButtonEvent`, and `XTestFakeKeyEvent`.
- **Headless fallback (`HeadlessBackend`)**: If no graphical display server is reachable, such as in automated CI environments or headless servers, `HeadlessBackend` generates synthetic test patterns and records input events in memory.

On Linux Wayland, injecting input through `/dev/uinput` requires write permissions for your user account. Load the kernel module and grant your user access with access control lists:

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
| `--no-gui` | Run the HTTP portal daemon without launching a desktop window or browser. |
| `--open-browser` | Open the portal URL in the default system web browser on startup. |
| `--registration <PATH>` | Override path to `linux-controller-private.json`. |
| `--partner <PATH>` | Override path to `default-partner.json`. |
| `--dictionary <PATH>` | Override path to the Zstandard dictionary (`8bit.dat`). |

## Select monitors in multi-display sessions

You can stream or view multiple monitors in three arrangements. Choosing all monitors (`ALL`) spans the entire virtual desktop across every connected display. Switching to a single monitor (`M1`, `M2`) crops the video stream to that screen and translates cursor coordinates relative to the monitor's origin at `virtual_left` and `virtual_top`.

You can also combine arbitrary outputs into a monitor subset. In `viewer.html`, click `ALL` to select the entire desktop or click an individual monitor label to focus on that display. Holding `Shift` or `Ctrl` while clicking monitor buttons toggles screens into a bounding virtual rectangle, so you only stream or view the displays relevant to your session.

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

- `src/main.rs` parses CLI subcommands (`host`, `id`, `master-password`, and `help`), loads XDG profiles, starts `HostSession` and `PortalServer`, and launches the native desktop window or browser fallback.
- `src/gui.rs` manages the native desktop window runtime using `tao` and `wry` (GTK3/WebKit2GTK on Linux and Cocoa/`WKWebView` on macOS), integrates native menubars via `muda`, drives clipboard synchronization, and coordinates browser fallback.
- `src/lib.rs` re-exports library modules and runs the integration test suite.
- `src/protocol.rs` executes 3DES registration cryptography, P2P AES-256-CBC framing, target route parsing, and multipart byte-array field serialization.
- `src/e2e.rs` implements `UDE2E1` end-to-end encryption using ephemeral X25519 key exchange and dual-key directional ChaCha20-Poly1305 AEAD.
- `src/screen.rs` houses `RGB555Screen`, `DisplayLayout`, `MonitorSelection`, 11-field screen update decoding, scroll-copy, LRU tile caching, bottom-up JPEG atlases, and Zstandard RGB555 decompression.
- `src/host.rs` runs `HostCore` and `HostSession`, manages Wayland, X11, and macOS capturers and injectors, handles macOS TCC permission requests (`Screen Recording` and `Accessibility`), performs multi-monitor cropping and coordinate translation, and enforces `LoginThrottle`.
- `src/session.rs` coordinates client registration, relay transport, keepalives, clipboard polling, and unified `SessionState` synchronization across client and host modes.
- `src/server.rs` runs `PortalServer`, serving `viewer.html`, embedded assets, and JSON or binary HTTP endpoints.
- `src/input.rs` encodes mouse movement, buttons, wheel scrolling, key events (`WM_KEYDOWN` and `WM_KEYUP`), and `VK_PACKET` Unicode packets, while decoding incoming host input commands.
- `src/cursor.rs` tracks remote cursor shape updates (`PACKET_CURSOR_CHANGE`, `PACKET_CURSOR_IMAGE`, and `PACKET_CURSOR_POS`), caches PNG cursor bitmaps by hash, and requests missing shapes.
- `src/audio.rs` decodes and encodes ITU-T G.722 64 kbps two-band QMF ADPCM packets (`42`, `43`, `50`, and `51`), producing 48 kHz stereo WAV buffers.
- `src/clipboard.rs` translates clipboard packets (`5` and `33`) across 1-byte and 2-byte `Int16LE` tags, Zstandard UTF-16LE streams, and pipe-delimited strings, bridging to `pbcopy` and `pbpaste` on macOS and `wl-copy`, `wl-paste`, `xclip`, `xsel`, and GTK on Linux.
- `src/file_transfer.rs` handles chunked file transfers and bidirectional UTF-16LE chat across packet kinds `12`, `13`, `14`, `15`, `16`, and `21`.
- `src/direct.rs` exchanges LAN direct TCP candidates and probes peer connectivity (`DirectHandshakeMachine`).
- `src/profile.rs` persists configuration files under `~/.config/ultradesk-rust` (`host-profile`, `profile`, `master-password`, and `credentials.json`).
- `packaging/macos/` contains `Info.plist`, `entitlements.plist`, and `package-macos.sh` to assemble `UltraDesk.app` and `.dmg` installers for macOS (`arm64` and `x86_64`).
- `viewer.html` delivers the bilingual (Vietnamese and English) single-page application with a docked menu toolbar, theme switching, and session tools.

## Run tests and benchmarks

Run the 66 release unit and security regression tests and the verification script (`scripts/verify_upgrade.sh`):

```bash
cargo test --release
bash scripts/verify_upgrade.sh
```

The test suite exercises `UDE2E1` X25519 + ChaCha20-Poly1305 end-to-end encryption, 3DES and AES-256-CBC cryptography, 11-field screen decoding, multi-monitor subset cropping, Wayland `grim` PPM parsing, macOS PNG-to-RGB555 conversion and keycode mapping, X11 `ZPixmap` conversion, G.722 audio encoding and decoding, bidirectional `Int16LE` chat and clipboard synchronization, chunked file transfer confinement, and live loopback host-to-viewer relay handshakes.

## Automated release pipelines

GitHub Actions workflows in `.github/workflows/` handle continuous integration and package distribution. On every push and pull request, `build.yml` and `nix.yml` run the release test suite and evaluate Nix flake checks. When `dtrh95` pushes a release tag (`v*`), `build-and-release.yml` verifies the tag pusher, builds binary packages for Debian, Ubuntu, Fedora, openSUSE, Arch Linux, and macOS (`arm64` and `x86_64` `.dmg` and `.app.tar.gz`), publishes release assets to GitHub Releases, and deploys signed APT, DNF, and Zypper repositories to Cloudflare Pages (`https://ultradesk-rust.pages.dev`).

Dedicated publishing workflows target individual distribution ecosystems. For Debian, `publish-debian.yml` targets bookworm, trixie, forky, and sid. For Ubuntu, `publish-ubuntu.yml` packages for jammy, noble, questing, and resolute, uploading signed source packages to Launchpad PPA. For NixOS, `publish-nixos.yml` and `update-nix-hash.yaml` build the flake closure, push binary substitutes to Cachix, export portable `.nar.xz` archives, and submit a pull request updating the package hash in `nix/packages/ultradesk-rust/default.nix`.

## License and trademark notice

`ultradesk-rust` is released under the [MIT License](LICENSE). It is an independent interoperability project and is not affiliated with or endorsed by DucFabulous Co., Ltd. or ULTRALAB Co., Ltd. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for third-party crate licenses and trademark details.

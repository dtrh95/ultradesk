Name:           ultradesk-rust
Version:        1.0.7
Release:        1%{?dist}
Summary:        Remote desktop controller and host for Linux (Rust Edition)
License:        MIT
URL:            https://github.com/ultradesk/ultradesk-rust
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  make
BuildRequires:  pkgconf-pkg-config
BuildRequires:  openssl-devel
BuildRequires:  libxcb-devel
BuildRequires:  gtk3-devel
BuildRequires:  webkit2gtk4.1-devel
BuildRequires:  libsoup3-devel
BuildRequires:  javascriptcoregtk4.1-devel

Requires:       gtk3
Requires:       webkit2gtk4.1
Requires:       libsoup3

%description
UltraDesk is a remote desktop client and host for Linux. You can run it in a
Tauri desktop window or headless from the command line. It handles screen
streaming, remote input, audio, clipboard synchronization, and file transfer.

%prep
%setup -q

%build
%make_build build

%install
%make_install PREFIX=%{_prefix} BINDIR=%{_bindir} DATADIR=%{_datadir}

%check
make check

%files
%license %{_datadir}/licenses/%{name}/LICENSE
%dir %{_datadir}/licenses/%{name}
%{_bindir}/ultradesk-rust
%{_datadir}/ultradesk-rust/
%{_datadir}/applications/ultradesk-rust.desktop
%{_datadir}/metainfo/io.github.ultradesk.UltraDesk.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/ultradesk-rust.svg
%{_datadir}/icons/hicolor/512x512/apps/ultradesk-rust.png

%post
if [ $1 -eq 1 ]; then
    echo "--- Cấu hình UltraDesk (Rust) ---"
    echo "Hướng dẫn sau cài đặt:"
    echo "1. Khởi chạy ứng dụng từ menu hoặc terminal:"
    echo "   ultradesk-rust"
    echo ""
    echo "2. Chế độ dòng lệnh (không mở cửa sổ GTK):"
    echo "   ultradesk-rust --no-gui --port 8765"
    echo "------------------------------------------------"
elif [ $1 -eq 2 ]; then
    echo "--- Cấu hình UltraDesk (Rust) ---"
    echo "Hướng dẫn sau cập nhật:"
    echo "1. Khởi động lại phiên UltraDesk đang chạy để áp dụng bản mới:"
    echo "   ultradesk-rust"
fi

%changelog
* Tue Nov 10 2026 Hieu Doan <dtrunghieu96@gmail.com> - 1.0.7-1
- Fix GTK3 clipboard reentrancy, Nix sandbox cache fallback, Wayland/XWayland clipboard sync, and Arch PKGBUILD
- Cross-distro and cross-OS auto-update engine, tasteskill 2.5 6-theme update popup (Confirm / Later), and remote-to-host file transfer.
- Release UltraDesk (Rust Edition) 1.0.5 with bidirectional remote-to-host file transfer, remote file pull (PACKET_FILE_REQUEST), automated release script, and Homebrew formula support
- Release UltraDesk (Rust Edition) 1.0.4 with RustCrypto 0.9/0.5/0.2/0.11 and x11rb 0.14.0 dependency upgrades
- Release UltraDesk (Rust Edition) 1.0.3 with macOS Host ID registration fallback, high-resolution relay frame compression, and E2E handshake ordering fixes
- Release UltraDesk (Rust Edition) 1.0.2 with built-in Model Context Protocol (MCP) server and community docs
- Release UltraDesk (Rust Edition) 1.0.1 with macOS support, TCC permission prompts, and DMG packaging
- Support screen streaming, remote input, audio, clipboard sync, and file transfer
- Add bilingual VI/EN portal and collapsible session toolbar

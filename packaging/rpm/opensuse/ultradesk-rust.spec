Name:           ultradesk-rust
Version:        0.2.0
Release:        1
Summary:        Remote desktop controller and host for Linux (Rust Edition)
License:        MIT
URL:            https://github.com/ultradesk/ultradesk-rust
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  make
BuildRequires:  pkg-config
BuildRequires:  libopenssl-devel
BuildRequires:  libxcb-devel
BuildRequires:  python3

Requires:       python3
Requires:       python3-gobject
Requires:       gtk3
Requires:       (typelib-1_0-WebKit2-4_1 or webkit2gtk4.1)

%description
UltraDesk is a Linux-native remote desktop controller and host written in Rust,
supporting screen streaming, remote input, audio, clipboard synchronization,
and file transfer.

%prep
%setup -q
find . -type f -name '*.py' -exec sed -i '1s|^#!.*env python3|#!/usr/bin/python3|' {} +

%build
%make_build build

%install
%make_install PREFIX=%{_prefix} BINDIR=%{_bindir} DATADIR=%{_datadir}
%py3_compile %{buildroot}%{_datadir}/ultradesk-rust

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
* Thu Nov 05 2026 Hieu Doan <hieudoan@ultradesk.io> - 0.2.0-1
- Release UltraDesk (Rust Edition) 0.2.0 with multi-distro packaging
- Support screen streaming, remote input, audio, clipboard sync, and file transfer
- Add bilingual VI/EN portal and collapsible session toolbar

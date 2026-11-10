{
  lib,
  rustPlatform,
  fetchFromGitHub,
  glib,
  gobject-introspection,
  gtk3,
  hicolor-icon-theme,
  libsoup_3,
  libxcb,
  nix-update-script,
  openssl,
  pkg-config,
  webkitgtk_4_1,
  wrapGAppsHook3,
}:

rustPlatform.buildRustPackage (finalAttrs: {
  pname = "ultradesk-rust";
  version = "1.0.7";

  src = fetchFromGitHub {
    owner = "dtrh95";
    repo = "ultradesk";
    tag = "v${finalAttrs.version}";
    hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
  };

  cargoLock = {
    lockFile = ../../.. + "/Cargo.lock";
  };

  passthru = {
    updateScript = nix-update-script { };
  };

  nativeBuildInputs = [
    gobject-introspection
    hicolor-icon-theme
    pkg-config
    wrapGAppsHook3
  ];

  buildInputs = [
    glib
    gtk3
    libsoup_3
    libxcb
    openssl
    webkitgtk_4_1
  ];

  strictDeps = true;

  __structuredAttrs = true;

  doCheck = true;

  postInstall = ''
    if [ -f $out/bin/ultraviewer-linux ] && [ ! -f $out/bin/ultradesk-rust ]; then
      mv $out/bin/ultraviewer-linux $out/bin/ultradesk-rust
    fi
    install -Dm644 viewer.html $out/share/ultradesk-rust/viewer.html
    if [ -f 8bit.dat ]; then
      install -Dm644 8bit.dat $out/share/ultradesk-rust/8bit.dat
    fi
    install -Dm644 io.github.ultradesk.UltraDesk.metainfo.xml $out/share/metainfo/io.github.ultradesk.UltraDesk.metainfo.xml
    if [ -f ultradesk-rust.svg ]; then
      install -Dm644 ultradesk-rust.svg $out/share/icons/hicolor/scalable/apps/ultradesk-rust.svg
    fi
    if [ -f ultradesk-rust.png ]; then
      install -Dm644 ultradesk-rust.png $out/share/icons/hicolor/512x512/apps/ultradesk-rust.png
    fi
    if [ -f LICENSE ]; then
      install -Dm644 LICENSE $out/share/licenses/ultradesk-rust/LICENSE
    elif [ -f packaging/debian/copyright ]; then
      install -Dm644 packaging/debian/copyright $out/share/licenses/ultradesk-rust/LICENSE
    fi
    install -d $out/share/applications
    cat > $out/share/applications/ultradesk-rust.desktop <<EOF
[Desktop Entry]
Version=1.0
Type=Application
Name=UltraDesk (Rust)
GenericName=Remote Desktop Controller
Comment=UltraDesk Remote Desktop Controller for Linux (Rust Edition)
Exec=$out/bin/ultradesk-rust
Icon=ultradesk-rust
Terminal=false
Categories=Network;RemoteAccess;Utility;
Keywords=ultradesk;ultraviewer;rust;remote;desktop;support;control;
StartupNotify=true
StartupWMClass=ultradesk-rust
EOF
  '';

  meta = {
    description = "Remote desktop controller and host for Linux (Rust Edition)";
    homepage = "https://github.com/ultradesk/ultradesk-rust";
    license = with lib.licenses; [
      mit
    ];
    mainProgram = "ultradesk-rust";
    platforms = lib.platforms.linux;
  };
})

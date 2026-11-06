PREFIX ?= /usr
BINDIR ?= $(PREFIX)/bin
DATADIR ?= $(PREFIX)/share
DESTDIR ?=

BINARY := $(firstword $(wildcard target/release/ultradesk-rust target/release/ultraviewer-linux))

.PHONY: all build check install clean

all: build

build:
	cargo build --release --locked || cargo build --release

check:
	cargo test --release

install:
	@if [ -f target/release/ultradesk-rust ]; then \
		install -Dm755 target/release/ultradesk-rust "$(DESTDIR)$(BINDIR)/ultradesk-rust"; \
	elif [ -f target/release/ultraviewer-linux ]; then \
		install -Dm755 target/release/ultraviewer-linux "$(DESTDIR)$(BINDIR)/ultradesk-rust"; \
	else \
		echo "Error: release binary not found in target/release" >&2; \
		exit 1; \
	fi
	install -Dm644 viewer.html "$(DESTDIR)$(DATADIR)/ultradesk-rust/viewer.html"
	@if [ -f 8bit.dat ]; then \
		install -Dm644 8bit.dat "$(DESTDIR)$(DATADIR)/ultradesk-rust/8bit.dat"; \
	fi
	install -d "$(DESTDIR)$(DATADIR)/applications"
	@if [ -f ultradesk-rust.desktop ]; then \
		sed -e 's|^Exec=.*|Exec=$(BINDIR)/ultradesk-rust|' \
		    -e '/^Path=/d' \
		    -e 's|^Icon=.*|Icon=ultradesk-rust|' \
		    -e 's|^StartupWMClass=.*|StartupWMClass=ultradesk-rust|' \
		    ultradesk-rust.desktop > "$(DESTDIR)$(DATADIR)/applications/ultradesk-rust.desktop"; \
		chmod 644 "$(DESTDIR)$(DATADIR)/applications/ultradesk-rust.desktop"; \
	else \
		printf '%s\n' \
			'[Desktop Entry]' \
			'Version=1.0' \
			'Type=Application' \
			'Name=UltraDesk (Rust)' \
			'GenericName=Remote Desktop Controller' \
			'Comment=UltraDesk Remote Desktop Controller for Linux (Rust Edition)' \
			'Exec=$(BINDIR)/ultradesk-rust' \
			'Icon=ultradesk-rust' \
			'Terminal=false' \
			'Categories=Network;RemoteAccess;Utility;' \
			'Keywords=ultradesk;ultraviewer;rust;remote;desktop;support;control;' \
			'StartupNotify=true' \
			'StartupWMClass=ultradesk-rust' \
			> "$(DESTDIR)$(DATADIR)/applications/ultradesk-rust.desktop"; \
		chmod 644 "$(DESTDIR)$(DATADIR)/applications/ultradesk-rust.desktop"; \
	fi
	install -Dm644 io.github.ultradesk.UltraDesk.metainfo.xml "$(DESTDIR)$(DATADIR)/metainfo/io.github.ultradesk.UltraDesk.metainfo.xml"
	@if [ -f ultradesk-rust.svg ]; then \
		install -Dm644 ultradesk-rust.svg "$(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/ultradesk-rust.svg"; \
	elif [ -f icons/ultradesk-rust.svg ]; then \
		install -Dm644 icons/ultradesk-rust.svg "$(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/ultradesk-rust.svg"; \
	fi
	@if [ -f ultradesk-rust.png ]; then \
		install -Dm644 ultradesk-rust.png "$(DESTDIR)$(DATADIR)/icons/hicolor/512x512/apps/ultradesk-rust.png"; \
	elif [ -f icons/ultradesk-rust.png ]; then \
		install -Dm644 icons/ultradesk-rust.png "$(DESTDIR)$(DATADIR)/icons/hicolor/512x512/apps/ultradesk-rust.png"; \
	fi
	@if [ -f LICENSE ]; then \
		install -Dm644 LICENSE "$(DESTDIR)$(DATADIR)/licenses/ultradesk-rust/LICENSE"; \
	elif [ -f packaging/debian/copyright ]; then \
		install -Dm644 packaging/debian/copyright "$(DESTDIR)$(DATADIR)/licenses/ultradesk-rust/LICENSE"; \
	fi

clean:
	cargo clean

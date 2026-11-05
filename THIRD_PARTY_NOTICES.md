# Third-party notices and legal information

## Trademark and non-affiliation disclaimer

UltraDesk (`ultradesk-rust`) is an independent, clean-room remote desktop client and host written in Rust for Linux. This project is not affiliated with, endorsed by, sponsored by, or produced by DucFabulous Co., Ltd. or ULTRALAB Co., Ltd. "UltraViewer" is a trademark of its respective owner and appears in this repository only to describe wire-protocol interoperability.

## External interoperability assets

The Zstandard preset dictionary file (`8bit.dat`) is an external interoperability asset used for decompressing legacy 16-bit RGB555 screen tiles. When `8bit.dat` is absent from `/usr/share/ultradesk-rust/8bit.dat` or `~/.config/ultradesk-rust/8bit.dat`, `ultradesk-rust` operates in standard dictionary-less Zstandard and JPEG screen modes.

## Rust crate dependencies

All direct Rust dependencies used by `ultradesk-rust` are distributed under permissive open-source licenses:

- `aes` (`0.8`), licensed under MIT or Apache-2.0.
- `cbc` (`0.1`), licensed under MIT or Apache-2.0.
- `cipher` (`0.4`), licensed under MIT or Apache-2.0.
- `des` (`0.8`), licensed under MIT or Apache-2.0.
- `image` (`0.25`), licensed under MIT or Apache-2.0.
- `libc` (`0.2`), licensed under MIT or Apache-2.0.
- `md-5` (`0.10`), licensed under MIT or Apache-2.0.
- `native-tls` (`0.2`), licensed under MIT or Apache-2.0.
- `serde` (`1.0`), licensed under MIT or Apache-2.0.
- `serde_json` (`1.0`), licensed under MIT or Apache-2.0.
- `x11rb` (`0.13`), licensed under MIT or Apache-2.0.
- `zstd` (`0.13`), licensed under MIT.

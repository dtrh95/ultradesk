# Third-party notices and legal information

## Trademark and non-affiliation disclaimer

UltraDesk (`ultradesk-rust`) is an independent, clean-room remote desktop client and host written in Rust for Linux. This project is not affiliated with, endorsed by, sponsored by, or produced by DucFabulous Co., Ltd. or ULTRALAB Co., Ltd. "UltraViewer" is a trademark of its respective owner and appears in this repository only to describe wire-protocol interoperability.

## External interoperability assets

The Zstandard preset dictionary file (`8bit.dat`) is an external interoperability asset used for decompressing legacy 16-bit RGB555 screen tiles. When `8bit.dat` is absent from `/usr/share/ultradesk-rust/8bit.dat` or `~/.config/ultradesk-rust/8bit.dat`, `ultradesk-rust` operates in standard dictionary-less Zstandard and JPEG screen modes.

## Rust crate dependencies

All direct crate dependencies are available under permissive open-source licenses.

The `aes`, `cbc`, `cipher`, `des`, `flate2`, `image`, `libc`, `md-5`, `native-tls`, `serde`, `serde_json`, and `x11rb` crates are dual-licensed under the MIT License and the Apache License, Version 2.0. You may use them under either license.

The `gtk` and `zstd` crates are licensed under the MIT License.

The `muda`, `tao`, and `wry` crates are licensed under the Apache License, Version 2.0.

#!/bin/bash
set -e
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DIR"

if [ ! -x "$DIR/target/release/ultradesk-rust" ] || [ -n "$(find "$DIR/src" "$DIR/Cargo.toml" "$DIR/viewer.html" -newer "$DIR/target/release/ultradesk-rust" 2>/dev/null)" ]; then
    cargo build --release
fi

exec "$DIR/target/release/ultradesk-rust" "$@"

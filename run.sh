#!/bin/bash
set -e
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DIR"

if [ ! -x "$DIR/target/release/ultradesk-rust" ]; then
    cargo build --release
fi

exec "$DIR/target/release/ultradesk-rust" "$@"

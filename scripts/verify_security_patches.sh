#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

echo "=== [1/3] Verifying static packaging, workflow, desktop, and secret-scrubbing guards ==="
! rg -q '/tmp/cargo-home|/tmp/rustup' packaging/debian/rules
rg -q 'CARGO_HOME = \$\(CURDIR\)/debian/\.cargo' packaging/debian/rules
rg -q 'RUSTUP_HOME = \$\(CURDIR\)/debian/\.rustup' packaging/debian/rules
! rg -q '^\s*(echo|VER=|RAW_VER=|TARBALL_URL=|SRI_HASH=|CURRENT_VER=).*\$\{\{' .github/workflows/*.yml .github/workflows/*.yaml
! rg -q '\|\|\s*cargo build --release' Makefile
! rg -q 'cargo fetch --locked.*\|\|\s*true' packaging/arch/PKGBUILD
! rg -q '/home/hieudoan' ultradesk-rust.desktop
rg -q '^Exec=ultradesk-rust$' ultradesk-rust.desktop
! rg -q '90135737|27296' default-partner.json
! rg -q '5AFD7336F718AC35C9B601B63E8A487E|64427868|00155dc9d02b' linux-controller-private.json
rg -q 'credentials\.json' .gitignore

echo "=== [2/3] Running dedicated security audit regression test suite ==="
cargo test --test security_audit_regressions -- --nocapture

echo "=== [3/3] Running full release & live portal verification lever (verify_upgrade.sh) ==="
./scripts/verify_upgrade.sh

echo "VERIFIED: All 22 security audit patches and regression checks passed."

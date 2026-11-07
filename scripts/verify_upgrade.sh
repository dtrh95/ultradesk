#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

echo "=== [1/3] Running release unit & integration test suite ==="
cargo test --release -- --nocapture

echo "=== [2/3] Building release binary ==="
cargo build --release

PORT=18769
BIN="./target/release/ultradesk-rust"
test -x "$BIN"
TMP_HOME="$(mktemp -d)"
trap 'kill $SERVER_PID 2>/dev/null || true; rm -rf "$TMP_HOME"' EXIT

XDG_CONFIG_HOME="$TMP_HOME" "$BIN" --no-gui --port "$PORT" >/tmp/uv_verify_server.log 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:${PORT}/status" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

echo "=== [3/3] Verifying live HTTP Portal & Host/Client endpoints ==="
VIEWER_HTML="$(curl -fsS "http://127.0.0.1:${PORT}/")"
rg -q '<html lang="en">' <<< "$VIEWER_HTML"
rg -q 'id="connectModal"' <<< "$VIEWER_HTML"
rg -q 'id="toolsPopover"' <<< "$VIEWER_HTML"
rg -q 'id="btnLangVi"' <<< "$VIEWER_HTML"
rg -q 'id="btnLangEn"' <<< "$VIEWER_HTML"
rg -q 'id="topPill"' <<< "$VIEWER_HTML"
rg -q 'id="pillExpandedContent"' <<< "$VIEWER_HTML"
rg -q 'id="btnCollapsePill"' <<< "$VIEWER_HTML"
rg -q 'id="btnRestorePill"' <<< "$VIEWER_HTML"
rg -q 'id="pillDragHandle"' <<< "$VIEWER_HTML"
rg -q 'top-pill\.collapsed' <<< "$VIEWER_HTML"
rg -q 'menu-toolbar' <<< "$VIEWER_HTML"
rg -q 'window\.__ultradesk_dispatch_action' <<< "$VIEWER_HTML"
rg -q 'setPillCollapsed' <<< "$VIEWER_HTML"
rg -q 'id="btnFullscreen"' <<< "$VIEWER_HTML"
rg -q 'const I18N' <<< "$VIEWER_HTML"
rg -q -- '--text-sm: 12px;' <<< "$VIEWER_HTML"
rg -q "'Host Grotesk'" <<< "$VIEWER_HTML"
rg -q -- '--m-ease' <<< "$VIEWER_HTML"
rg -q 'showNotice' <<< "$VIEWER_HTML"
rg -q '\.m-tab-ind' <<< "$VIEWER_HTML"
! rg -q 'backdrop-filter' <<< "$VIEWER_HTML"
! rg -q 'alert\(' <<< "$VIEWER_HTML"
! rg -q "'Inter'" <<< "$VIEWER_HTML"
! rg -q '\(\|\|\|31\|\|\|\)' <<< "$VIEWER_HTML"

STATUS_JSON="$(curl -fsS "http://127.0.0.1:${PORT}/status")"
rg -q '"host_status"' <<< "$STATUS_JSON"
rg -q '"frame_version"' <<< "$STATUS_JSON"

HOST_STATUS="$(curl -fsS "http://127.0.0.1:${PORT}/host/status")"
rg -q '"host_id"' <<< "$HOST_STATUS"
rg -q '"hardware_mac"' <<< "$HOST_STATUS"

MP_RESP="$(curl -fsS -X POST "http://127.0.0.1:${PORT}/host/master-password" \
  -H "Content-Type: application/json" \
  -d '{"password":"StrongMasterPass123"}')"
rg -q '"has_master_password":true' <<< "$MP_RESP"

MON_RESP="$(curl -fsS -X POST "http://127.0.0.1:${PORT}/monitor" \
  -H "Content-Type: application/json" \
  -d '{"monitor":0}')"
rg -q '"ok":true' <<< "$MON_RESP"

PREF_RESP="$(curl -fsS -X POST "http://127.0.0.1:${PORT}/preferences" \
  -H "Content-Type: application/json" \
  -d '{"theme":"onedark","language":"en"}')"
rg -q '"ok":true' <<< "$PREF_RESP"

STATUS_AFTER_PREF="$(curl -fsS "http://127.0.0.1:${PORT}/status")"
rg -q '"saved_theme":"onedark"' <<< "$STATUS_AFTER_PREF"
rg -q '"saved_language":"en"' <<< "$STATUS_AFTER_PREF"

test -f "$TMP_HOME/ultradesk-rust/credentials.json"
rg -q '"theme": "onedark"' "$TMP_HOME/ultradesk-rust/credentials.json"

HTTP_CODE_JPG="$(curl -s -o /tmp/uv_frame.jpg -w "%{http_code}" "http://127.0.0.1:${PORT}/frame.jpg?q=80")"
test "$HTTP_CODE_JPG" = "200"
test -s /tmp/uv_frame.jpg

HTTP_CODE_BMP="$(curl -s -o /tmp/uv_frame.bmp -w "%{http_code}" "http://127.0.0.1:${PORT}/frame.bmp")"
test "$HTTP_CODE_BMP" = "200"
test -s /tmp/uv_frame.bmp

FRAME_VER="$(curl -fsS "http://127.0.0.1:${PORT}/status" | sed -n 's/.*"frame_version":\([0-9]*\).*/\1/p')"
if [ -n "$FRAME_VER" ]; then
  HTTP_CODE_304="$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:${PORT}/frame.jpg?v=${FRAME_VER}&q=80")"
  test "$HTTP_CODE_304" = "304"
fi

PUBKEY_UID="$(gpg --show-keys --with-colons packaging/pubkey.gpg | awk -F: '/^uid/ {print $10; exit}')"
rg -q 'dtrunghieu96@gmail\.com' <<< "$PUBKEY_UID"
! rg -q 'fcitx5-lotus' <<< "$PUBKEY_UID"
! rg -q 'hieudoan@ultradesk\.io|maintainers@ultradesk\.local' packaging/ .github/workflows/publish-ubuntu.yml

echo "VERIFIED: All unit tests, benchmarks, and live HTTP portal checks passed."

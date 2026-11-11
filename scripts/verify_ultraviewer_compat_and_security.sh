#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

echo "[1/4] Checking static UltraViewer Windows compatibility and supply-chain invariants..."
jq -e '(.request_terms | length) == 17 and .request_terms[1] == "" and .password == ""' default-partner.json >/dev/null
jq -e '.terms[8] == "BC241128BDE1"' linux-controller-private.json >/dev/null

if rg -q "sh\.rustup\.rs" packaging/debian/rules; then
  echo "FAIL: unverified rustup pipe-to-shell found in packaging/debian/rules" >&2
  exit 1
fi
if ! rg -q "ULTRADESK_SOURCE_SHA256" packaging/arch/PKGBUILD || rg -q "^sha256sums=\('SKIP'\)$" packaging/arch/PKGBUILD; then
  echo "FAIL: unconditional SKIP checksum found in packaging/arch/PKGBUILD" >&2
  exit 1
fi
if rg -n "uses:\s+[^#]+@(v[0-9]+|master|main)\b" .github/workflows/*.yml .github/workflows/*.yaml; then
  echo "FAIL: unpinned GitHub Action tag found in .github/workflows" >&2
  exit 1
fi
if rg -q "JSON\.stringify\(\{\s*id,\s*pass" web/src/app/infrastructure/storage/local-preferences.storage.ts viewer.html; then
  echo "FAIL: plaintext password persistence found in localStorage adapter or viewer.html" >&2
  exit 1
fi

echo "[2/4] Verifying Angular Web SPA typecheck and lint..."
(cd web && npm run typecheck && npm run lint)

echo "[3/4] Running Rust unit, UltraViewer Windows 6.6.133 compat, and run-2 security tests..."
RUSTFLAGS="-D warnings" cargo test --offline

echo "[4/4] Running live Portal HTTP, MCP bearer isolation, and LAN E2E smoke test..."
BIN="${REPO_ROOT}/target/release/ultradesk-rust"
if [ ! -x "$BIN" ]; then
  cargo build --release --offline
fi

TMP_RUN="$(mktemp -d /tmp/uv-lever-XXXXXX)"
SERVER_PID=""
cleanup() {
  if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$TMP_RUN"
}
trap cleanup EXIT

pick_port() {
  python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}

PORT="$(pick_port)"
MCP_PORT="$(pick_port)"
while [ "$MCP_PORT" = "$PORT" ]; do
  MCP_PORT="$(pick_port)"
done
MCP_TOKEN="lever-token-$(python3 -c 'import secrets; print(secrets.token_hex(16))')"

mkdir -p "$TMP_RUN/config" "$TMP_RUN/cache"
chmod 700 "$TMP_RUN" "$TMP_RUN/config" "$TMP_RUN/cache"

XDG_CONFIG_HOME="$TMP_RUN/config" \
XDG_CACHE_HOME="$TMP_RUN/cache" \
ULTRADESK_DISABLE_AUTO_UPDATE=1 \
  "$BIN" --no-gui --host 127.0.0.1 --port "$PORT" --mcp --mcp-port "$MCP_PORT" --mcp-token "$MCP_TOKEN" --auto-approve \
  >"$TMP_RUN/server.log" 2>&1 &
SERVER_PID=$!

COOKIE_JAR="$TMP_RUN/cookies.txt"
READY=0
for _ in $(seq 1 60); do
  if curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" "http://127.0.0.1:${PORT}/" >/dev/null 2>&1; then
    STATUS_JSON="$(curl -fsS -b "$COOKIE_JAR" "http://127.0.0.1:${PORT}/status" 2>/dev/null || true)"
    if [ -n "$STATUS_JSON" ] && jq -e '.host_status.running == true and (.host_status.lan_port // 0) > 0' >/dev/null 2>&1 <<<"$STATUS_JSON"; then
      READY=1
      break
    fi
  fi
  sleep 0.1
done
if [ "$READY" -ne 1 ]; then
  echo "FAIL: ultradesk-rust failed to start on port ${PORT}" >&2
  cat "$TMP_RUN/server.log" >&2 || true
  exit 1
fi

REBIND_CODE="$(curl -sS -o /dev/null -w '%{http_code}' -H 'Host: evil.example' -b "$COOKIE_JAR" "http://127.0.0.1:${PORT}/status")"
[ "$REBIND_CODE" = "403" ] || { echo "FAIL: expected 403 for Host DNS rebinding, got ${REBIND_CODE}" >&2; exit 1; }

UNAUTH_CODE="$(curl -sS -o /dev/null -w '%{http_code}' "http://127.0.0.1:${PORT}/status")"
[ "$UNAUTH_CODE" = "401" ] || { echo "FAIL: expected 401 for unauthenticated /status, got ${UNAUTH_CODE}" >&2; exit 1; }

MCP_UNAUTH_CODE="$(curl -sS -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:${MCP_PORT}/mcp" -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')"
[ "$MCP_UNAUTH_CODE" = "401" ] || { echo "FAIL: expected 401 for unauthenticated MCP, got ${MCP_UNAUTH_CODE}" >&2; exit 1; }

MCP_TOOLS_RESP="$(curl -fsS -X POST "http://127.0.0.1:${MCP_PORT}/mcp" -H "Authorization: Bearer ${MCP_TOKEN}" -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')"
jq -e '(.result.tools | length) == 19' >/dev/null <<<"$MCP_TOOLS_RESP"

MCP_HOST_SHOT="$(curl -fsS -X POST "http://127.0.0.1:${MCP_PORT}/mcp" -H "Authorization: Bearer ${MCP_TOKEN}" -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"take_screenshot","arguments":{"source":"host"}}}')"
jq -e '.result.isError == true' >/dev/null <<<"$MCP_HOST_SHOT"

MCP_STATUS_RESP="$(curl -fsS -X POST "http://127.0.0.1:${MCP_PORT}/mcp" -H "Authorization: Bearer ${MCP_TOKEN}" -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_session_status","arguments":{}}}')"
MCP_INNER_JSON="$(jq -r '.result.content[0].text' <<<"$MCP_STATUS_RESP")"
jq -e '(.saved_password == "") and ((.host_status.temp_password // "") == "")' >/dev/null <<<"$MCP_INNER_JSON"

LAN_PORT="$(jq -r '.host_status.lan_port' <<<"$STATUS_JSON")"
TEMP_PASS="$(jq -r '.host_status.temp_password' <<<"$STATUS_JSON")"
CONNECT_BODY="$(jq -nc --arg id "lan://127.0.0.1:${LAN_PORT}" --arg pass "$TEMP_PASS" '{id: $id, pass: $pass, remember: true}')"
curl -fsS -b "$COOKIE_JAR" -H "Origin: http://127.0.0.1:${PORT}" -H 'Content-Type: application/json' -d "$CONNECT_BODY" "http://127.0.0.1:${PORT}/connect" >/dev/null

CONNECTED=0
for _ in $(seq 1 50); do
  SNAP="$(curl -fsS -b "$COOKIE_JAR" "http://127.0.0.1:${PORT}/status")"
  if jq -e '.state == "connected" and .e2e_enabled == true' >/dev/null 2>&1 <<<"$SNAP"; then
    CONNECTED=1
    break
  fi
  sleep 0.1
done
[ "$CONNECTED" -eq 1 ] || { echo "FAIL: live LAN loopback session did not reach connected+e2e_enabled" >&2; exit 1; }

echo "PASS: All UltraViewer Windows 6.6.133 compatibility and run-2 security checks succeeded."

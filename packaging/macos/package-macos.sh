#!/usr/bin/env bash
set -euo pipefail

TARGET="${1:-aarch64-apple-darwin}"
ARCH="${2:-arm64}"
RAW_VERSION="${3:-v1.0.7}"
VERSION="${RAW_VERSION#v}"
VERSION="${VERSION//\//-}"

APP_NAME="UltraDesk.app"
BUILD_DIR="macos-dist-${ARCH}"
APP_DIR="${BUILD_DIR}/${APP_NAME}"
CONTENTS_DIR="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RES_DIR="${CONTENTS_DIR}/Resources"

rm -rf "${BUILD_DIR}"
mkdir -p "${MACOS_DIR}" "${RES_DIR}"

install -m 0755 "target/${TARGET}/release/ultradesk-rust" "${MACOS_DIR}/ultradesk-rust"
strip -x "${MACOS_DIR}/ultradesk-rust" 2>/dev/null || true

install -m 0644 viewer.html "${RES_DIR}/viewer.html"
install -m 0644 8bit.dat "${RES_DIR}/8bit.dat"

ICON_SRC="ultradesk-rust.png"
if [ ! -f "${ICON_SRC}" ] && [ -f "data/icons/hicolor/512x512/apps/ultradesk-rust.png" ]; then
  ICON_SRC="data/icons/hicolor/512x512/apps/ultradesk-rust.png"
fi

if [ -f "${ICON_SRC}" ]; then
  install -m 0644 "${ICON_SRC}" "${RES_DIR}/ultradesk-rust.png"
  ICONSET_DIR="${BUILD_DIR}/UltraDesk.iconset"
  mkdir -p "${ICONSET_DIR}"
  for size in 16 32 128 256 512; do
    sips -z "${size}" "${size}" "${ICON_SRC}" --out "${ICONSET_DIR}/icon_${size}x${size}.png" >/dev/null 2>&1 || cp "${ICON_SRC}" "${ICONSET_DIR}/icon_${size}x${size}.png"
    double=$((size * 2))
    sips -z "${double}" "${double}" "${ICON_SRC}" --out "${ICONSET_DIR}/icon_${size}x${size}@2x.png" >/dev/null 2>&1 || cp "${ICON_SRC}" "${ICONSET_DIR}/icon_${size}x${size}@2x.png"
  done
  iconutil -c icns "${ICONSET_DIR}" -o "${RES_DIR}/UltraDesk.icns" || true
  rm -rf "${ICONSET_DIR}"
fi

sed "s|<string>1.0.7</string>|<string>${VERSION}</string>|g" packaging/macos/Info.plist > "${CONTENTS_DIR}/Info.plist"

codesign --force --deep --entitlements packaging/macos/entitlements.plist --sign - "${APP_DIR}"

tar -czf "ultradesk-rust-v${VERSION}-${ARCH}-macos.app.tar.gz" -C "${BUILD_DIR}" "${APP_NAME}"

DMG_STAGE="${BUILD_DIR}/dmg-root"
mkdir -p "${DMG_STAGE}"
touch "${DMG_STAGE}/.metadata_never_index"
cp -R "${APP_DIR}" "${DMG_STAGE}/${APP_NAME}"
ln -s /Applications "${DMG_STAGE}/Applications"

DMG_OUT="ultradesk-rust-v${VERSION}-${ARCH}-macos.dmg"
rm -f "${DMG_OUT}"
for attempt in 1 2 3; do
  if hdiutil create \
    -volname "UltraDesk ${VERSION} (${ARCH})" \
    -srcfolder "${DMG_STAGE}" \
    -ov \
    -format UDZO \
    "${DMG_OUT}"; then
    break
  fi
  sleep 2
done
test -f "${DMG_OUT}"

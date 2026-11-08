#!/usr/bin/env bash
set -euo pipefail

TARGET="${1:-aarch64-apple-darwin}"
ARCH="${2:-arm64}"
RAW_VERSION="${3:-v1.0.1}"
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
install -m 0644 icon.png "${RES_DIR}/icon.png"

sed "s|<string>1.0.1</string>|<string>${VERSION}</string>|g" packaging/macos/Info.plist > "${CONTENTS_DIR}/Info.plist"

ICONSET_DIR="${BUILD_DIR}/UltraDesk.iconset"
mkdir -p "${ICONSET_DIR}"
for size in 16 32 128 256 512; do
  sips -z "${size}" "${size}" icon.png --out "${ICONSET_DIR}/icon_${size}x${size}.png" >/dev/null 2>&1 || cp icon.png "${ICONSET_DIR}/icon_${size}x${size}.png"
  double=$((size * 2))
  sips -z "${double}" "${double}" icon.png --out "${ICONSET_DIR}/icon_${size}x${size}@2x.png" >/dev/null 2>&1 || cp icon.png "${ICONSET_DIR}/icon_${size}x${size}@2x.png"
done
iconutil -c icns "${ICONSET_DIR}" -o "${RES_DIR}/UltraDesk.icns" || true
rm -rf "${ICONSET_DIR}"

codesign --force --deep --entitlements packaging/macos/entitlements.plist --sign - "${APP_DIR}"

tar -czf "ultradesk-rust-v${VERSION}-${ARCH}-macos.app.tar.gz" -C "${BUILD_DIR}" "${APP_NAME}"

DMG_STAGE="${BUILD_DIR}/dmg-root"
mkdir -p "${DMG_STAGE}"
cp -R "${APP_DIR}" "${DMG_STAGE}/${APP_NAME}"
ln -s /Applications "${DMG_STAGE}/Applications"

hdiutil create \
  -volname "UltraDesk ${VERSION} (${ARCH})" \
  -srcfolder "${DMG_STAGE}" \
  -ov \
  -format UDZO \
  "ultradesk-rust-v${VERSION}-${ARCH}-macos.dmg"

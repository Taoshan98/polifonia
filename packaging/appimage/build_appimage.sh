#!/usr/bin/env bash
# Polifonia - AppImage Builder Script (Rust Native v2.0+)
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

DEFAULT_VER="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
VERSION="${1:-${VERSION:-${DEFAULT_VER:-2.0.0}}}"
APP_DIR="build/AppDir"

echo "=== Building Polifonia AppImage (${VERSION}) ==="

if [ "${SKIP_CARGO_BUILD:-0}" != "1" ]; then
  cargo build --release
fi

if [ ! -x "target/release/polifonia" ]; then
  echo "Error: target/release/polifonia not found or not executable." >&2
  exit 1
fi

rm -rf "${APP_DIR}"
mkdir -p \
  "${APP_DIR}/usr/bin" \
  "${APP_DIR}/usr/share/applications" \
  "${APP_DIR}/usr/share/icons/hicolor/scalable/apps" \
  "${APP_DIR}/usr/share/metainfo"

# Binary
install -Dm755 "target/release/polifonia" "${APP_DIR}/usr/bin/polifonia"

# Desktop entry, AppStream metadata, and icons (including full-color tray icon)
install -Dm644 "io.github.taoshan98.Polifonia.desktop" "${APP_DIR}/io.github.taoshan98.Polifonia.desktop"
install -Dm644 "io.github.taoshan98.Polifonia.desktop" "${APP_DIR}/usr/share/applications/io.github.taoshan98.Polifonia.desktop"
install -Dm644 "assets/io.github.taoshan98.Polifonia.svg" "${APP_DIR}/io.github.taoshan98.Polifonia.svg"
install -Dm644 "assets/io.github.taoshan98.Polifonia.svg" "${APP_DIR}/.DirIcon"
install -Dm644 "assets/io.github.taoshan98.Polifonia.svg" "${APP_DIR}/usr/share/icons/hicolor/scalable/apps/io.github.taoshan98.Polifonia.svg"
install -Dm644 "assets/io.github.taoshan98.Polifonia-symbolic.svg" "${APP_DIR}/usr/share/icons/hicolor/scalable/apps/io.github.taoshan98.Polifonia-symbolic.svg"
install -Dm644 "assets/io.github.taoshan98.Polifonia.svg" "${APP_DIR}/usr/share/icons/hicolor/scalable/apps/polifonia-studio-color.svg"
install -Dm644 "io.github.taoshan98.Polifonia.metainfo.xml" "${APP_DIR}/usr/share/metainfo/io.github.taoshan98.Polifonia.metainfo.xml"
install -Dm755 "packaging/appimage/AppRun" "${APP_DIR}/AppRun"

echo "AppDir structure prepared at ${APP_DIR}."
echo "Use appimagetool to generate the final bundle: ARCH=x86_64 appimagetool --no-appstream ${APP_DIR} Polifonia-${VERSION}-x86_64.AppImage"

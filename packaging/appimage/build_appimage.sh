#!/usr/bin/env bash
# Polifonia - AppImage Builder Script
set -e

VERSION="${1:-${VERSION:-1.0.5}}"
APP_DIR="build/AppDir"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echo "=== Building Polifonia AppImage (${VERSION}) ==="
rm -rf "${APP_DIR}"
mkdir -p "${APP_DIR}/usr/bin"
mkdir -p "${APP_DIR}/usr/share/applications"
mkdir -p "${APP_DIR}/usr/share/icons/hicolor/scalable/apps"

# Copy project files
cp -r "${ROOT_DIR}/core" "${APP_DIR}/"
cp -r "${ROOT_DIR}/backend" "${APP_DIR}/"
cp -r "${ROOT_DIR}/services" "${APP_DIR}/"
cp -r "${ROOT_DIR}/storage" "${APP_DIR}/"
cp -r "${ROOT_DIR}/ui" "${APP_DIR}/"
cp "${ROOT_DIR}/main.py" "${APP_DIR}/"

# Copy metadata and icons
cp "${ROOT_DIR}/io.github.taoshan98.Polifonia.desktop" "${APP_DIR}/"
cp "${ROOT_DIR}/io.github.taoshan98.Polifonia.desktop" "${APP_DIR}/usr/share/applications/"
cp "${ROOT_DIR}/assets/io.github.taoshan98.Polifonia.svg" "${APP_DIR}/io.github.taoshan98.Polifonia.svg"
cp "${ROOT_DIR}/assets/io.github.taoshan98.Polifonia.svg" "${APP_DIR}/usr/share/icons/hicolor/scalable/apps/"
cp "${ROOT_DIR}/assets/io.github.taoshan98.Polifonia-symbolic.svg" "${APP_DIR}/usr/share/icons/hicolor/scalable/apps/"
cp "${ROOT_DIR}/assets/io.github.taoshan98.Polifonia.svg" "${APP_DIR}/.DirIcon"
cp "${ROOT_DIR}/packaging/appimage/AppRun" "${APP_DIR}/AppRun"
chmod +x "${APP_DIR}/AppRun"

echo "AppDir structure prepared at ${APP_DIR}."
echo "Use appimagetool to generate the final bundle: appimagetool ${APP_DIR} Polifonia-${VERSION}-x86_64.AppImage"

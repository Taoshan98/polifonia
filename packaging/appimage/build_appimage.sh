#!/usr/bin/env bash
# Polifonia Audio Studio - AppImage Builder Script
set -e

APP_DIR="build/AppDir"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echo "=== Building Polifonia AppImage ==="
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
cp "${ROOT_DIR}/io.polifonia.AudioStudio.desktop" "${APP_DIR}/"
cp "${ROOT_DIR}/io.polifonia.AudioStudio.desktop" "${APP_DIR}/usr/share/applications/"
cp "${ROOT_DIR}/assets/io.polifonia.AudioStudio.svg" "${APP_DIR}/io.polifonia.AudioStudio.svg"
cp "${ROOT_DIR}/assets/io.polifonia.AudioStudio.svg" "${APP_DIR}/usr/share/icons/hicolor/scalable/apps/"
cp "${ROOT_DIR}/packaging/appimage/AppRun" "${APP_DIR}/AppRun"
chmod +x "${APP_DIR}/AppRun"

echo "AppDir structure prepared at ${APP_DIR}."
echo "Use appimagetool to generate the final bundle: appimagetool ${APP_DIR} Polifonia-x86_64.AppImage"

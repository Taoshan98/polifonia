#!/usr/bin/env bash
# Polifonia - Debian / Ubuntu Package Builder (Rust Native v2.0+)
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

DEFAULT_VER="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
VERSION="${1:-${VERSION:-${DEFAULT_VER:-2.0.0}}}"
DEB_ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
PKG_DIR="build/debian_pkg"
OUT_DEB="polifonia_${VERSION}_${DEB_ARCH}.deb"

echo "=== Building Polifonia Debian Package (${VERSION}, ${DEB_ARCH}) ==="

if [ "${SKIP_CARGO_BUILD:-0}" != "1" ]; then
  cargo build --release
fi

if [ ! -x "target/release/polifonia" ]; then
  echo "Error: target/release/polifonia not found or not executable." >&2
  exit 1
fi

rm -rf "${PKG_DIR}"
mkdir -p \
  "${PKG_DIR}/DEBIAN" \
  "${PKG_DIR}/usr/bin" \
  "${PKG_DIR}/usr/share/applications" \
  "${PKG_DIR}/usr/share/icons/hicolor/scalable/apps" \
  "${PKG_DIR}/usr/share/metainfo" \
  "${PKG_DIR}/usr/share/doc/polifonia"

# Prepare DEBIAN/control with resolved architecture and version
sed "s/^Architecture:.*/Architecture: ${DEB_ARCH}/" "packaging/debian/control" > "${PKG_DIR}/DEBIAN/control"
echo "Version: ${VERSION}" >> "${PKG_DIR}/DEBIAN/control"

# Install compiled native Rust binary directly to /usr/bin/polifonia
install -Dm755 "target/release/polifonia" "${PKG_DIR}/usr/bin/polifonia"

# Copy desktop launcher, AppStream metadata, icons, and documentation
install -Dm644 "io.github.taoshan98.Polifonia.desktop" "${PKG_DIR}/usr/share/applications/io.github.taoshan98.Polifonia.desktop"
install -Dm644 "assets/io.github.taoshan98.Polifonia.svg" "${PKG_DIR}/usr/share/icons/hicolor/scalable/apps/io.github.taoshan98.Polifonia.svg"
install -Dm644 "assets/io.github.taoshan98.Polifonia-symbolic.svg" "${PKG_DIR}/usr/share/icons/hicolor/scalable/apps/io.github.taoshan98.Polifonia-symbolic.svg"
install -Dm644 "assets/io.github.taoshan98.Polifonia.svg" "${PKG_DIR}/usr/share/icons/hicolor/scalable/apps/polifonia-studio-color.svg"
install -Dm644 "io.github.taoshan98.Polifonia.metainfo.xml" "${PKG_DIR}/usr/share/metainfo/io.github.taoshan98.Polifonia.metainfo.xml"
install -Dm644 "README.md" "${PKG_DIR}/usr/share/doc/polifonia/README.md"
install -Dm644 "LICENSE" "${PKG_DIR}/usr/share/doc/polifonia/copyright"

# Build .deb package
dpkg-deb --build "${PKG_DIR}" "${OUT_DEB}"
echo "Debian package successfully built: ${OUT_DEB}"

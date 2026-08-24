#!/usr/bin/env bash
# Polifonia Audio Studio - Debian / Ubuntu Package Builder
set -e

VERSION="1.0.0"
PKG_DIR="build/debian_pkg"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echo "=== Building Polifonia Debian Package (${VERSION}) ==="
rm -rf "${PKG_DIR}"
mkdir -p "${PKG_DIR}/DEBIAN"
mkdir -p "${PKG_DIR}/usr/bin"
mkdir -p "${PKG_DIR}/usr/lib/polifonia"
mkdir -p "${PKG_DIR}/usr/share/applications"
mkdir -p "${PKG_DIR}/usr/share/icons/hicolor/scalable/apps"
mkdir -p "${PKG_DIR}/usr/share/doc/polifonia"

# Copy debian control file
cp "${ROOT_DIR}/packaging/debian/control" "${PKG_DIR}/DEBIAN/"
sed -i "s/\${misc:Depends}//g" "${PKG_DIR}/DEBIAN/control"
sed -i "s/\${python3:Depends}/python3 (>= 3.10)/g" "${PKG_DIR}/DEBIAN/control"
echo "Version: ${VERSION}" >> "${PKG_DIR}/DEBIAN/control"

# Copy source files to /usr/lib/polifonia
cp -r "${ROOT_DIR}/core" "${PKG_DIR}/usr/lib/polifonia/"
cp -r "${ROOT_DIR}/backend" "${PKG_DIR}/usr/lib/polifonia/"
cp -r "${ROOT_DIR}/services" "${PKG_DIR}/usr/lib/polifonia/"
cp -r "${ROOT_DIR}/storage" "${PKG_DIR}/usr/lib/polifonia/"
cp -r "${ROOT_DIR}/ui" "${PKG_DIR}/usr/lib/polifonia/"
cp "${ROOT_DIR}/main.py" "${PKG_DIR}/usr/lib/polifonia/"

# Create executable launcher in /usr/bin/polifonia
cat << 'EOF' > "${PKG_DIR}/usr/bin/polifonia"
#!/usr/bin/env bash
export GSK_RENDERER="gl"
exec python3 /usr/lib/polifonia/main.py "$@"
EOF
chmod 755 "${PKG_DIR}/usr/bin/polifonia"

# Copy desktop launcher and icons
cp "${ROOT_DIR}/io.polifonia.AudioStudio.desktop" "${PKG_DIR}/usr/share/applications/"
cp "${ROOT_DIR}/assets/io.polifonia.AudioStudio.svg" "${PKG_DIR}/usr/share/icons/hicolor/scalable/apps/"
cp "${ROOT_DIR}/assets/io.polifonia.AudioStudio-symbolic.svg" "${PKG_DIR}/usr/share/icons/hicolor/scalable/apps/"
cp "${ROOT_DIR}/README.md" "${PKG_DIR}/usr/share/doc/polifonia/"
cp "${ROOT_DIR}/LICENSE" "${PKG_DIR}/usr/share/doc/polifonia/copyright"

# Build .deb package
dpkg-deb --build "${PKG_DIR}" "polifonia_${VERSION}_all.deb"
echo "Debian package successfully built: polifonia_${VERSION}_all.deb"

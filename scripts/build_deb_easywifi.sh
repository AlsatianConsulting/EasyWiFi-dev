#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

PACKAGE_NAME="easywifi"
PACKAGE_VERSION="${VERSION_OVERRIDE:-$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n1)}"
if [[ -z "${PACKAGE_VERSION}" ]]; then
    echo "failed to determine package version from Cargo.toml" >&2
    exit 1
fi

DEB_ARCH="${DEB_ARCH_OVERRIDE:-$(dpkg --print-architecture)}"
BUILD_ROOT="$ROOT_DIR/dist/deb-build/${PACKAGE_NAME}_${PACKAGE_VERSION}_${DEB_ARCH}"
STAGE_DIR="$BUILD_ROOT/${PACKAGE_NAME}_${PACKAGE_VERSION}_${DEB_ARCH}"
OUTPUT_DEB="$ROOT_DIR/dist/${PACKAGE_NAME}_${PACKAGE_VERSION}_${DEB_ARCH}.deb"

echo "[1/5] Building release binaries"
cargo build --release --bin easywifi --bin easywifi-helper

echo "[2/5] Preparing package tree"
rm -rf "$BUILD_ROOT"
mkdir -p \
    "$STAGE_DIR/DEBIAN" \
    "$STAGE_DIR/usr/bin" \
    "$STAGE_DIR/usr/share/applications" \
    "$STAGE_DIR/usr/share/doc/${PACKAGE_NAME}" \
    "$STAGE_DIR/usr/share/${PACKAGE_NAME}/assets"

install -m755 "target/release/easywifi" "$STAGE_DIR/usr/bin/easywifi"
install -m755 "target/release/easywifi-helper" "$STAGE_DIR/usr/bin/easywifi-helper"
install -m644 "packaging/easywifi.desktop" "$STAGE_DIR/usr/share/applications/easywifi.desktop"
install -m644 "README.md" "$STAGE_DIR/usr/share/doc/${PACKAGE_NAME}/README.md"

for file in assets/bt_company_ids.csv assets/bt_service_uuids.csv assets/bt_uuid_resolver_overrides.csv assets/oui.csv; do
    if [[ -f "$file" ]]; then
        install -m644 "$file" "$STAGE_DIR/usr/share/${PACKAGE_NAME}/assets/$(basename "$file")"
    fi
done

if [[ -f "manuf" ]]; then
    install -m644 "manuf" "$STAGE_DIR/usr/share/${PACKAGE_NAME}/manuf"
fi

if [[ -f "GeoLite2-City.mmdb" && "${SKIP_GEOIP_MMDB:-0}" != "1" ]]; then
    install -m644 "GeoLite2-City.mmdb" "$STAGE_DIR/usr/share/${PACKAGE_NAME}/GeoLite2-City.mmdb"
fi

cat >"$STAGE_DIR/DEBIAN/control" <<CONTROL
Package: ${PACKAGE_NAME}
Version: ${PACKAGE_VERSION}
Section: net
Priority: optional
Architecture: ${DEB_ARCH}
Maintainer: EasyWiFi Maintainers <noreply@easywifi.local>
Depends: libc6 (>= 2.34), libgtk-4-1, libglib2.0-0, libgdk-pixbuf-2.0-0, libpango-1.0-0, libgraphene-1.0-0, libcairo2, libbluetooth3, bluez, iw, tshark
Description: EasyWiFi passive Wi-Fi/Bluetooth observer and mapper
 EasyWiFi is a passive Linux observability application for monitor-mode
 Wi-Fi and Bluetooth/BLE collection, analysis, and export.
CONTROL

cat >"$STAGE_DIR/DEBIAN/postinst" <<'POSTINST'
#!/bin/sh
set -e
if command -v setcap >/dev/null 2>&1; then
    setcap cap_net_admin,cap_net_raw=eip /usr/bin/easywifi-helper || true
fi
exit 0
POSTINST
chmod 0755 "$STAGE_DIR/DEBIAN/postinst"

echo "[3/5] Building .deb"
mkdir -p "$ROOT_DIR/dist"
dpkg-deb --root-owner-group --build "$STAGE_DIR" "$OUTPUT_DEB"

echo "[4/5] Package info"
dpkg-deb -I "$OUTPUT_DEB" | sed -n '1,120p'

echo "[5/5] Package files"
dpkg-deb -c "$OUTPUT_DEB" | sed -n '1,200p'

echo
echo "Built package: $OUTPUT_DEB"

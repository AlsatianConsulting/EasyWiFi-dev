#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT_DIR="$ROOT_DIR/continuity/transfer"
BUNDLE_PATH="$OUT_DIR/EasyWiFi.bundle"
TARBALL_PATH="$OUT_DIR/EasyWiFi-working-tree.tar.gz"
SHA_PATH="$OUT_DIR/SHA256SUMS"

mkdir -p "$OUT_DIR"
cd "$ROOT_DIR"

echo "[1/3] Creating git bundle"
git bundle create "$BUNDLE_PATH" --all

echo "[2/3] Creating working-tree tarball"
tar \
  --exclude='./.git' \
  --exclude='./target' \
  --exclude='./dist' \
  --exclude='./Processed' \
  --exclude='./.cache' \
  --exclude='./.local-link-libs' \
  --exclude='./.pkgconfig-stub' \
  --exclude='./session_*' \
  --exclude='./continuity/transfer/*.bundle' \
  --exclude='./continuity/transfer/*.tar.gz' \
  --exclude='./continuity/transfer/SHA256SUMS' \
  --exclude='./*.png' \
  --exclude='./*.log' \
  --exclude='./*.bt' \
  -czf "$TARBALL_PATH" .

echo "[3/3] Writing checksums"
(
  cd "$OUT_DIR"
  sha256sum "$(basename "$BUNDLE_PATH")" "$(basename "$TARBALL_PATH")" > "$(basename "$SHA_PATH")"
)

echo
printf 'Artifacts created:\n  %s\n  %s\n  %s\n' "$BUNDLE_PATH" "$TARBALL_PATH" "$SHA_PATH"

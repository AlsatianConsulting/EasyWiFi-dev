#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

core_packages=(
  build-essential pkg-config curl git clang cmake
  libgtk-4-dev libglib2.0-dev libcairo2-dev libpango1.0-dev
  libgdk-pixbuf-2.0-dev libsqlite3-dev libssl-dev libasound2-dev
  libpcap-dev libclang-dev iw tshark gpsd gpsd-clients beep bluez
  gdb ripgrep jq
)

optional_packages=(
  rtl-sdr rtl-433 multimon-ng direwolf hackrf bladerf uhd-host ubertooth welle.io
)

install_if_available() {
  local pkg="$1"
  if apt-cache show "$pkg" >/dev/null 2>&1; then
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "$pkg"
  else
    echo "[skip] package not available in current apt sources: $pkg"
  fi
}

echo "[1/5] Installing core apt packages"
sudo apt-get update
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "${core_packages[@]}"

echo "[2/5] Installing optional RF packages when available"
for pkg in "${optional_packages[@]}"; do
  install_if_available "$pkg"
done

if ! command -v rustup >/dev/null 2>&1; then
  echo "[3/5] Installing rustup"
  curl https://sh.rustup.rs -sSf | sh -s -- -y
fi

# shellcheck disable=SC1090
source "$HOME/.cargo/env"

echo "[4/5] Installing stable Rust toolchain"
rustup toolchain install stable
rustup default stable

echo "[5/5] Building and testing"
cargo test -q
cargo build -q

echo
echo "Bootstrap complete. Next recommended commands:"
echo "  cargo run"
echo "  bash scripts/build_deb.sh"
echo "  sudo uhd_images_downloader    # if using B210"

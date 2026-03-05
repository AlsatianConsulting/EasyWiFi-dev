#!/usr/bin/env bash
set -euo pipefail

if ! command -v sudo >/dev/null 2>&1; then
  echo "sudo is required to install system dependencies."
  exit 1
fi

echo "[1/4] Installing OS dependencies"
sudo apt-get update
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y \
  build-essential \
  pkg-config \
  curl \
  git \
  clang \
  cmake \
  libgtk-4-dev \
  libglib2.0-dev \
  libcairo2-dev \
  libpango1.0-dev \
  libgdk-pixbuf-2.0-dev \
  libsqlite3-dev \
  libssl-dev \
  libasound2-dev \
  libpcap-dev \
  libclang-dev \
  iw \
  tshark \
  gpsd \
  gpsd-clients \
  beep

if ! command -v rustup >/dev/null 2>&1; then
  echo "[2/4] Installing rustup"
  curl https://sh.rustup.rs -sSf | sh -s -- -y
fi

# shellcheck disable=SC1090
source "$HOME/.cargo/env"

echo "[3/4] Installing stable toolchain"
rustup toolchain install stable
rustup default stable

echo "[4/4] Building"
cargo build

echo

echo "Build complete. Run with:"
echo "  cargo run"

# Fresh Ubuntu Bootstrap

Use this when bringing the repository up on a clean Ubuntu system.

## Recommended sequence

1. Install base packages.
2. Install Rust via `rustup`.
3. Build the repo.
4. Install optional RF tooling.
5. Plug in hardware and verify visibility.
6. Run noninteractive tests.

## Fast path

```bash
cd WirelessExplorer
bash continuity/bootstrap_ubuntu.sh
```

## Manual path

```bash
sudo apt-get update
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y \
  build-essential pkg-config curl git clang cmake \
  libgtk-4-dev libglib2.0-dev libcairo2-dev libpango1.0-dev \
  libgdk-pixbuf-2.0-dev libsqlite3-dev libssl-dev libasound2-dev \
  libpcap-dev libclang-dev iw tshark gpsd gpsd-clients beep bluez \
  gdb ripgrep jq
```

Install Rust:

```bash
curl https://sh.rustup.rs -sSf | sh -s -- -y
source "$HOME/.cargo/env"
rustup toolchain install stable
rustup default stable
```

Build:

```bash
cargo test -q
cargo build -q
```

Optional RF tool packages to try on Ubuntu:

```bash
sudo apt-get install -y rtl-sdr rtl-433 multimon-ng direwolf hackrf bladerf uhd-host ubertooth welle.io || true
```

## Post-install checks

### Wi-Fi

```bash
iw dev
sudo -n iw dev
```

### Bluetooth

```bash
bluetoothctl list
btmgmt info
```

### SDR

```bash
rtl_sdr -h >/dev/null
hackrf_info
bladeRF-cli -p
uhd_find_devices
ubertooth-util -v
```

## App validation commands

```bash
cargo test -q
cargo build -q
./target/debug/wirelessexplorer --help
```

## Packaging

```bash
bash scripts/build_deb.sh
```

This produces a `.deb` under `dist/`.

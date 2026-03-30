# EasyWiFi (Rust + GTK)

Linux desktop application focused on passive Wi-Fi and Bluetooth discovery.

## Current scope

- Access Points tab
  - AP list with SSID, BSSID, OUI, channel, encryption, clients, first/last seen
  - Detail pane with encryption details, notes, packet pie chart, GPS-derived observations
  - Associated clients pane
  - Context menu: `View Details`, `Locate Device`, `Lock To Channel`
- Clients tab
  - Client list with MAC, OUI, associated AP, RSSI, probes, first/last heard
  - Detail pane with seen APs, probes, handshake networks, GPS-derived observations
  - Context menu: `View Details`, `Locate Device`
- Bluetooth tab
  - Passive Bluetooth discovery (BlueZ and optional Ubertooth source)
  - Detail pane with MFGR/UUID enrichment and active enumeration details
  - Built-in Bluetooth geiger tracker
  - Context menu: `Locate Device`, `Connect & Enumerate`, `Disconnect`
- Channel Usage tab
  - Channel utilization chart
  - Optional inline channel usage panel on Access Points tab
  - Runtime heartbeat status (`wifi/bt`)
- Settings
  - Interface/channel mode settings
  - GPS settings (`Interface`, `GPSD`, `Stream`, `Static`)
  - Bluetooth source/controller/Ubertooth settings
  - View toggles and table layout controls
- Exports
  - CSV exports (global and per-device detail)
  - KML/KMZ location exports
  - Consolidated PCAPNG and handshake captures
- Storage
  - Per-session SQLite persistence

## Build and run

```bash
./scripts/setup_ubuntu.sh
./scripts/run.sh
```

Or:

```bash
cargo run
```

## Non-interactive validation examples

```bash
# Wi-Fi test mode
cargo run -- --test-wifi --interface <iface> --packet-headers radiotap
cargo run -- --test-wifi --interface <iface> --packet-headers ppi
```

## Runtime requirements

- Linux with monitor-capable Wi-Fi adapters
- Tools: `iw`, `tshark`
- GTK 4 runtime/dev libraries
- Optional: `beep` for audible geiger tone
- Optional (Bluetooth): `bluetoothctl`, `hcitool`/BlueZ stack, `ubertooth-btle`

## Passive behavior notes

- No deauthentication or active packet injection.
- WPA2 handshake count increments only when all 4 EAPOL messages are observed passively.
- WPA3 handshake counting is intentionally not implemented.

## OUI database

- Bundled OUI CSV: `assets/oui.csv`
- File menu includes `Update OUI Database` to fetch latest IEEE OUI data.

## Project layout

- `src/ui/mod.rs`: GTK UI and dialogs
- `src/capture/mod.rs`: Wi-Fi capture pipeline
- `src/bluetooth/mod.rs`: Bluetooth scanning and enrichment helpers
- `src/storage/mod.rs`: SQLite schema and persistence
- `src/export/mod.rs`: export logic
- `src/gps/mod.rs`: GPS providers and parsing
- `src/model/mod.rs`: shared models
- `src/settings.rs`: settings models
- `src/oui/mod.rs`: OUI lookup/update

## Current limitations

See:

- `docs/LIMITATIONS.md`
- `docs/VALIDATION.md`
- `CHANGELOG.md`

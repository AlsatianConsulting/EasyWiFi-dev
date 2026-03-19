# WirelessExplorer (Rust + GTK)

Linux desktop application inspired by Sparrow Wi-Fi, focused on **100% passive** Wi-Fi observation with a refined multi-tab workflow.

## MVP status
This repository contains an MVP implementation with:

- Access Points tab
  - AP list (SSID, BSSID, OUI, channel, encryption, clients, first/last seen, handshake count)
  - Detail pane (frequency, full encryption, notes, OUI, 802.11d country, uptime field, WPS field, packet pie chart, first/last/strongest GPS location with timestamp)
  - Associated clients pane (MAC, OUI, data transferred, RSSI)
  - Highlighting: handshake networks in yellow, watchlist-matched networks/devices in green
  - AP context menu: `View Details`, `Locate Device`, `Lock To Channel`
- Clients tab
  - Client list (MAC, OUI, associated AP, RSSI, probes, first/last heard)
  - Detail pane with seen APs, probes, handshake networks, WPS field, first/last/strongest GPS location with timestamp
  - Watchlist-matched clients highlighted in green (list + details)
  - Client context menu: `View Details`, `Locate Device`
- Bluetooth tab
  - Passive Bluetooth discovery (BT/BLE, MAC, OUI, identified type, first/last seen)
  - Detail pane with resolved MFGR IDs/UUIDs (offline bundled SIG mappings where available)
  - Embedded geiger tracker panel for selected Bluetooth device (RSSI + audible tone mapping)
- Channel Usage tab
  - Channel utilization chart with spectrum dropdown filter
  - Optional inline Channel Usage panel in Access Points tab (toggleable)
- SDR tab
  - FFT + spectrogram + decode tables
  - Center-frequency geiger indicators (RSSI/tone estimate/activity bar from center FFT bins)
  - Optional auto-squelch from center geiger with configurable dB margin
  - Bookmark add/jump workflow with persistence in app settings
  - One-click operator preset profiles for common bands/workflows
  - Save Current as Preset stores user-defined SDR presets in app settings
  - Rename/Delete/Move controls for saved user presets
  - Import/Export saved user presets via JSON (`wirelessexplorer-sdr-presets.json`)
  - Map and satcom audit entries now carry both parsed `message` and `raw` decoder text (redacted when no-payload mode is enabled)
- Settings via File menu
  - Interface/channel mode settings (multi-adapter, hop/lock)
  - GPS settings (`Interface`, `GPSD`, `Stream` TCP/UDP NMEA, `Static`)
  - View toggles include status/details/device panes, table column filters, and AP inline Channel Usage
- Layout via File menu
  - Per-table column chooser (show/hide), reordering (up/down), and width controls for AP/client/associated-client tables
  - Alert/watchlist controls (handshake alerts, watchlist alerts, network+device watchlists)
- Live GPS status indicator (mode, connected/disconnected, last fix timestamp, endpoint/detail)
  - GPSD mode uses `WATCH` JSON with TPV fix parsing
  - Output GPS coordinates honor configured `GpsSettings::Static` when valid, with safe default fallback
- Exports
  - CSV exports (global + per-device detail CSV)
  - Location-only CSV logs:
    - AP: `Date/BSSID/OUI/SSID/Channel/Encryption/RSSI/Associated Clients` (+ coordinates)
    - Client: `Date/MAC/OUI/Associated BSSID/RSSI` (+ coordinates)
    - Bluetooth location logs (+ coordinates)
  - KML location logs colorized by RSSI with consolidated folders:
    - `kml/access_points`
    - `kml/clients`
    - `kml/bluetooth`
  - KML/KMZ style policy includes:
    - AP styles by encryption class
    - client styles by associated vs unassociated state
    - bluetooth styles by BLE vs classic transport
  - Consolidated session PCAPNG (`pcap/consolidated_capture.pcapng`)
  - Consolidated GPS-annotated PCAPNG (`pcap/consolidated_capture_with_gps.pcapng`)
  - Dedicated handshake capture folder (`pcap/handshakes`)
  - Handshake files include EAPOL frames plus one beacon frame for offline analyzers
  - Handshake naming: `ssid_bssid_client_YYYY-MM-DDTHH:MM:SSZ.pcapng`
  - GPS track embedding into exported PCAPNG packet comments
- Storage
  - SQLite database created per session
  - Session output directory tree created at startup

## Build and run

```bash
./scripts/setup_ubuntu.sh
./scripts/run.sh
```

Or manually:

```bash
cargo run
```

Non-interactive validation examples:

```bash
# Wi-Fi test mode with explicit packet header choice
cargo run -- --test-wifi --interface <iface> --packet-headers radiotap
cargo run -- --test-wifi --interface <iface> --packet-headers ppi
```

## Runtime requirements

- Linux with monitor-capable Wi-Fi adapters
- Tools: `iw`, `tshark`
- GTK 4 runtime/dev libs
- Optional: `beep` utility for audible geiger tone output

## Notes on passive behavior

- No deauthentication or active injection is performed.
- WPA2 full handshake count increments only when all 4 EAPOL key messages are passively observed.
- WPA3 handshake counting is intentionally not implemented (per your requirement).

## OUI database

- Bundled OUI CSV is included under `assets/oui.csv`.
- File menu includes `Update OUI Database`, which downloads latest IEEE CSV and reloads it.

## Project layout

- `src/ui/mod.rs`: GTK application UI, tabs, menus, dialogs, context menus
- `src/capture/mod.rs`: passive capture pipeline, monitor/channel helpers, geiger updates
- `src/bluetooth/mod.rs`: passive Bluetooth scanning, controller handling, MFGR/UUID resolution helpers
- `src/storage/mod.rs`: SQLite schema and persistence
- `src/export/mod.rs`: CSV/PCAP export logic
- `src/gps/mod.rs`: GPS providers and NMEA parsing
- `src/model/mod.rs`: shared data models
- `src/settings.rs`: app/interface/GPS settings models
- `src/oui/mod.rs`: OUI lookup/update logic

## Current limitations

See:

- `docs/LIMITATIONS.md`
- `docs/VALIDATION.md`

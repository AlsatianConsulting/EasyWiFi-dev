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
  - Bluetooth context menu includes `Scan BLE Data Channels (SDR)` to apply a BLE-focused SDR scan profile
  - Bluetooth context menu also includes `Scan Zigbee 2.4 Channels (SDR)` for quick IoT scan-range application
- Channel Usage tab
  - Channel utilization chart with spectrum dropdown filter
  - Optional inline Channel Usage panel in Access Points tab (toggleable)
  - Status area includes a live runtime heartbeat (`wifi/bt/sdr`) updated every second
- SDR tab
  - FFT + spectrogram + decode tables
  - Center-frequency geiger indicators (RSSI/tone estimate/activity bar from center FFT bins)
  - Optional auto-squelch from center geiger with configurable dB margin
  - Bookmark add/jump workflow with persistence in app settings
  - `Scan Around Bookmark` quick action with configurable ±kHz window to convert selected bookmarks into active scan ranges
  - Presets -> Frequencies now includes FCC-driven explorer actions:
    - `FCC Area Explorer (CSV, with Signal Type)` to generate/apply/persist a scan profile from FCC assignment rows
    - `FCC Area Explorer (CSV URL)` for direct URL ingestion with retry/timeout handling
    - `FCC Frequency Explorer (CSV -> Bookmarks)` to import individual FCC-assigned frequencies as bookmarks
    - `FCC Frequency Explorer (CSV URL -> Bookmarks)` to import bookmarks directly from URL
    - `Remove FCC Bookmarks` to prune previously imported FCC bookmark entries
  - FCC explorers support area token filtering (city/county/state/callsign) and optional signal/service-type filtering
  - FCC area explorers attempt decoder auto-selection from detected signal/service type when possible (for example public safety -> P25, maritime -> AIS)
  - FCC frequency imports now report added vs duplicate-skipped counts and keep bookmark data normalized/sorted
  - Bookmark export now writes both CSV and JSON artifacts
  - One-click operator preset profiles for common bands/workflows
  - Bluetooth frequency presets include Classic channels, BLE data channels (0-36), and BLE advertising channels (37-39)
  - Scanner presets now include a dedicated `BLE Data Channels` sweep profile (2404-2478 MHz, 2 MHz steps)
  - Scanner presets also include a dedicated `Zigbee 2.4 Channels` sweep profile (2405-2480 MHz, 5 MHz steps)
  - Save Current as Preset stores user-defined SDR presets in app settings
  - Rename/Delete/Move controls for saved user presets
  - Import/Export saved user presets via JSON (`wirelessexplorer-sdr-presets.json`)
  - Map and satcom audit entries now carry both parsed `message` and `raw` decoder text (redacted when satellite payload capture is disabled)
  - Satcom audit includes payload mode + parsed unencrypted metadata (`payload_capture_mode`, `payload_parse_state`, normalized `payload_fields`)
  - Built-in `Inmarsat STD-C` decoder (`inmarsat_stdc`) is available when `stdc_decoder`/`stdc-decoder`/`inmarsatc-decoder` is installed
  - Satcom parser denylist is configurable in SDR controls (persisted in settings), with env fallback `WIRELESSEXPLORER_SATCOM_PARSE_DENYLIST`
  - Satcom export now writes full audit JSON plus companion CSV/parsed-only JSON/denied-only JSON/summary JSON artifacts
  - Decode export writes full decode-row JSON + CSV artifacts
  - Decode export now also supports filtered JSON + CSV output using active decode-table filters
  - `Validate Decoder` button performs a dry-run command/dependency readiness check for the selected decoder/hardware profile
  - `Presets -> Frequencies` now includes `Import SDR Bookmarks CSV` (`frequency_hz` or `frequency_mhz` input columns)
  - `Presets -> Frequencies` now includes `Import SDR Bookmarks File (Auto CSV/JSON)` for one-step local ingest
  - `Presets -> Frequencies` now includes `Import SDR Bookmarks JSON` (array root or `{ "bookmarks": [...] }` with `frequency_hz`/`frequency_mhz`)
  - `Presets -> Frequencies` also supports direct bookmark import from remote JSON URL
  - `Presets -> Frequencies` now includes `Import SDR Bookmarks URL (Auto CSV/JSON)` for one-step remote ingest
  - `Presets -> Frequencies` also supports direct bookmark import from remote CSV URL
  - Bookmark imports now auto-detect CSV vs JSON when file extension/content mismatch occurs
  - Bookmark import parsers accept `freq` as an alias for raw Hz values in both CSV and JSON inputs
  - Ambiguous `frequency` fields now auto-detect Hz vs MHz by magnitude in both CSV and JSON imports
  - Duplicate-frequency bookmark imports now upgrade default placeholder labels when richer labels are present
  - Bookmark file import dialogs now include CSV/JSON file filters while still allowing mixed-format autodetection
  - Bookmark controls include `Decode Bookmark` for one-click tune + decoder start on the selected bookmark
  - `Export SDR Health JSON` captures telemetry/rate counters plus satcom/aircraft summaries in one snapshot artifact
  - SDR CSV exports with timestamp fields honor the selected time display mode (Local or Zulu/UTC)
  - SDR decoder text logs also honor the selected Local/Zulu time mode
  - SDR summary JSON artifacts (`generated_at`, first/last windows) honor selected Local/Zulu mode
  - Aircraft correlation export now derives ADS-B/ACARS identity joins (`icao_hex`/`callsign`) and writes JSON+CSV artifacts
  - SDR health section includes live aircraft-correlation counts (mixed ADS-B/ACARS + single-source tallies)
  - SDR health section also includes live satcom summary counters (parse state + encryption posture)
  - Decoder health telemetry is surfaced in SDR pane (`rows/map/satcom/stderr` counters)
  - FFT right-click supports direct per-signal decoder launch
  - Decoder launch availability checks now gate right-click/start actions with explicit hardware/toolchain status
  - Right-click decode menu now exposes unavailable-decoder reason text as hover tooltips
  - Unavailable-decoder hints now include explicit missing-tool guidance for `rtl_433`, `ADS-B`, and `GSM/LTE` paths
  - Unavailable-decoder hints now also cover `AIS` RTL fallback requirements and satellite decoder tooling (`iridium-extractor`, `stdc_decoder` variants)
  - GSM/LTE decoder launch command now carries explicit center frequency and non-RTL Soapy driver arguments for improved multi-device compatibility
  - ADS-B built-in resolver now supports `dump1090-fa` as an additional RTL fallback before `readsb`
  - AIS built-in resolver now uses an explicit RTL fallback pipeline (`rtl_fm -> aisdecoder`) when `rtl_ais` is unavailable
  - ACARS built-in resolver no longer requires `acarsdec` for non-RTL fallback pipelines (`csdr/sox/multimon-ng`), improving multi-device launch success
  - APRS/POCSAG/DECT built-in resolvers now fall back to an RTL IQ pipeline (`rtl_sdr + csdr + sox + multimon-ng`) when `rtl_fm` is unavailable
  - Built-in decoder command path supports optional hardware-specific plugin overrides via `sdr-plugins.json` IDs:
    - `<decoder_id>_<hardware_id>`
    - `<decoder_id>__<hardware_id>`
    - `<decoder_id>` (global built-in override)
- Settings via File menu
  - Interface/channel mode settings (multi-adapter, hop/lock)
  - Time display mode setting (Local default, optional Zulu/UTC)
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

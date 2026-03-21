# Validation Ledger

This document tracks what WirelessExplorer currently outputs, what was validated on this host, what was not collected during live tests, and which areas still need implementation.

## Current Output Inventory

When file output is enabled, a session directory is created under the configured output root:

- `session_<timestamp>_<uuid>/csv/access_points.csv`
- `session_<timestamp>_<uuid>/csv/clients.csv`
- `session_<timestamp>_<uuid>/csv/access_point_locations.csv`
- `session_<timestamp>_<uuid>/csv/client_locations.csv`
- `session_<timestamp>_<uuid>/csv/bluetooth_locations.csv`
- `session_<timestamp>_<uuid>/json/summary.json`
- `session_<timestamp>_<uuid>/kml/access_points/observations.kml`
- `session_<timestamp>_<uuid>/kml/clients/observations.kml`
- `session_<timestamp>_<uuid>/kml/bluetooth/observations.kml`
- `session_<timestamp>_<uuid>/pcap/consolidated_capture.pcapng`
- `session_<timestamp>_<uuid>/pcap/consolidated_capture_with_gps.pcapng`
- `session_<timestamp>_<uuid>/pcap/handshakes/*.pcapng`
- `session_<timestamp>_<uuid>/logs/session.log`

Current SDR logging can also emit:

- per-decoder rolling text logs
- per-decoder map logs (including `message` + `raw` fields)
- per-decoder satcom audit logs (including `message` + `raw` fields; redacted in no-payload mode)
- satcom audit records now include `payload_capture_mode`, `payload_parse_state`, and normalized `payload_fields` for unencrypted frames
- optional satcom parser denylist control via `WIRELESSEXPLORER_SATCOM_PARSE_DENYLIST` (protocol/decoder token match)
- satcom export action now emits companion CSV + parsed-only JSON + denied-only JSON + summary JSON artifacts
- decode export action emits decode-row JSON + CSV artifacts
- decode export now also supports filtered JSON + CSV artifacts based on active decode-table filters
- bookmark management now includes `Import SDR Bookmarks CSV` (`frequency_hz` or `frequency_mhz` columns)
- bookmark management now includes file-based auto import (`CSV`/`JSON`) under `Presets -> Frequencies`
- bookmark management now includes `Import SDR Bookmarks JSON` (array root or `{ "bookmarks": [...] }` envelope)
- bookmark import field aliases include `freq` (Hz) in addition to `frequency_hz`/`freq_hz`/`hz`
- ambiguous `frequency` fields are interpreted as Hz when large (`>= 1_000_000`) or MHz otherwise
- bookmark management also includes URL-based JSON import with retry/timeout fetch behavior
- bookmark management also includes URL-based auto import (`CSV`/`JSON`) with parser auto-detection
- bookmark management also includes URL-based CSV import with retry/timeout fetch behavior
- bookmark import parser path now auto-detects CSV vs JSON on unknown/mismatched file extensions
- duplicate-frequency bookmark imports can upgrade existing default placeholder labels when richer labels are supplied
- Bluetooth frequency presets include BLE data channels (`0-36`) in addition to Classic and advertising channels
- scanner presets include dedicated BLE data-channel and Zigbee 2.4 channel sweep profiles
- Bluetooth context menu can apply a BLE data-channel SDR scan profile directly from selected device rows
- bookmark export now emits both CSV and JSON artifacts with source tagging
- bookmark workflow now includes one-click tune + decoder start (`Decode Bookmark`)
- aircraft correlation export action emits ADS-B/ACARS correlation JSON + CSV artifacts (`icao_hex`/`callsign` merged identities)
- `Export SDR Health JSON` now emits a telemetry/counter snapshot artifact for runtime monitoring handoff
- SDR pane now surfaces decoder telemetry counters (`rows`, `map`, `satcom`, `stderr`)
- SDR pane now surfaces live aircraft-correlation and satcom-summary status counters for operator monitoring
- SDR controls now include a dry-run decoder validation check for selected decoder/hardware/frequency settings
- IQ sample files captured on demand
- user SDR preset exchange file at config path: `wirelessexplorer-sdr-presets.json` (via SDR preset import/export controls)

Preset/Explorer workflow additions:

- `Presets -> Frequencies` includes FCC explorer actions for scan-profile generation and bookmark imports (file CSV + URL modes)
- FCC explorer filters support area token matching and optional signal/service-type constraints
- FCC URL workflows use retry/timeout handling for unstable upstream hosts
- FCC bookmark imports now summarize added vs skipped duplicates and keep bookmark ordering normalized

KML/KMZ regression coverage now also verifies:

- AP/client/bluetooth folders are present in KMZ `doc.kml`
- per-hit ExtendedData includes encryption classification and key identifiers
- per-hit ExtendedData includes `source_adapters` provenance fields
- client per-hit ExtendedData includes association state (`associated` / `unassociated`)
- bluetooth per-hit ExtendedData includes transport class normalization variants (including `btle`)
- bluetooth per-hit ExtendedData includes optional active-enumeration state fields when available

Wi-Fi capture regression coverage now also verifies:

- interactive decoder startup now attempts configured packet-header mode with fallback (`PPI` -> `Radiotap`)
- interactive parse path accepts both `radiotap.dbm_antsignal` and `ppi.dbm_antsignal`
- RSSI selection prefers radiotap and falls back to PPI when radiotap is empty
- geiger-mode RSSI parsing also supports radiotap/PPI fallback

Summary JSON regression coverage now also verifies:

- client `associated_state` (`associated` / `unassociated`) in addition to associated BSSID/SSID
- AP `band` and `wps` serialization
- client `wps` serialization
- bluetooth `transport_class` normalization (`ble` / `classic` / `unknown`)
- bluetooth `active_enumeration` serialization

Static GPS regression coverage now verifies:

- default output coordinates remain `35.1453957, -79.4747181`
- valid `GpsSettings::Static` coordinates are used for output GPS
- invalid static coordinates fall back to defaults
- nearest-fix selection uses closest timestamp, rejects stale fixes (>20s), and rejects invalid coordinates

CSV regression coverage now also verifies:

- `bluetooth_locations.csv` includes normalized `Transport Class` (`ble` / `classic` / `unknown`)

UI runtime status behavior now includes:

- `Runtime Activity` heartbeat line in Channel status pane updates every second
- heartbeat text includes Wi-Fi/Bluetooth/SDR runtime on/off state flags

Time display behavior now includes:

- local time as default display mode for UI timestamps
- optional Zulu/UTC display mode toggle in Preferences -> General
- SDR CSV exports with timestamp fields follow the same Local/Zulu mode
- SDR decoder text log timestamps follow the same Local/Zulu mode
- SDR summary JSON artifact time fields (`generated_at`, first/last windows) follow the same Local/Zulu mode

## Multi-Adapter Status

### Implemented

1. Wi-Fi runtime capture supports multiple enabled interfaces in one session.
2. Wi-Fi AP, client, and Bluetooth records retain `source_adapters` provenance and surface it in details.
3. Wi-Fi geiger and lock workflows prefer the adapter that actually observed the selected record.
4. Wi-Fi test mode accepts one or many interfaces in one invocation and reports per-interface results.
5. Bluetooth scan supports default, specific, or `all` BlueZ controllers in one session.
6. Bluetooth scan supports default, specific, or `all` Ubertooth devices in one session.
7. Bluetooth enumerate, disconnect, and geiger workflows resolve the selected device back to the BlueZ controller that observed it when controller selection is `default` or `all`.

### Partial

1. Bluetooth multi-adapter logic is implemented, but live validation on this host is limited to one BlueZ controller and one enumerated Ubertooth device.

### Not Yet Implemented

1. Multiple SDR devices running simultaneously inside one SDR session.

## Live Validation Results

These results are from privileged local testing on this host and should be treated as sample-window validation, not proof of RF absence.

### Wi-Fi

Adapters present:

1. `wlx1cbfcef8e928`
2. `wlp0s20f3`

Field support confirmed on this host:

1. `wlan.ssid`
2. `wlan.rsn.version`
3. `wlan_rsna_eapol.keydes.msgnr`

Not supported on this host:

1. `wlan_mgt.ssid`
2. `wlan_mgt.rsn.version`
3. `eapol.keydes.msgnr`

Validated behavior:

1. `wlx1cbfcef8e928` can be put into monitor mode successfully.
2. App-style `tshark` capture on `wlx1cbfcef8e928` works without `-I`.
3. Testing with `tshark -I` against an interface already in monitor mode produces a false-negative "device doesn't support monitor mode" error and should not be used as the validation method here.
4. Noninteractive Wi-Fi test mode accepts `--packet-headers radiotap` and `--packet-headers ppi` (with fallback to radiotap when local PPI capture cannot start).

Collected during live tests:

1. `wlx1cbfcef8e928`, channels `1,6,11`, 10s dwell:
   - channel 1: collected `HomeNetwork` (`80:CC:9C:AE:C1:5C`) at `-39 dBm`
   - channel 11: collected `Precious` (`2C:67:BE:4A:ED:85`) at `-59 dBm`
2. Multi-interface test, `wlp0s20f3,wlx1cbfcef8e928`, channels `1,6,11`, 6s dwell:
   - `wlp0s20f3`: collected 0 access points in that sample window
   - `wlx1cbfcef8e928`, channel 11: collected `Precious` (`2C:67:BE:4A:ED:85`) at `-57 dBm`

Not collected during the sampled windows:

1. `wlx1cbfcef8e928`, channel 6: no BSSID-bearing frames observed in the sample window
2. `wlp0s20f3`, channels `1,6,11`, 10s dwell: no BSSID-bearing frames observed in the sample window
3. Multi-interface test, `wlx1cbfcef8e928`, channels 1 and 6: no BSSID-bearing frames observed in the sample window
4. Multi-interface test, `wlp0s20f3`, channels `1,6,11`: no BSSID-bearing frames observed in the sample window

### Bluetooth

Controller present:

1. BlueZ controller `D0:C6:37:4D:3E:05`

Validated behavior:

1. BlueZ passive scan path starts and returns device observations.
2. BlueZ scan accepts `--controller all` and iterates the available controller set without error.
3. BlueZ + Ubertooth scan accepts `--controller all --ubertooth-device all` without error.
4. Ubertooth path is wired, but local collection was not validated successfully on this host.

Collected during live tests:

1. BlueZ-only scan observed at least one Bluetooth device in a 12s window.

Not collected during the sampled windows:

1. Ubertooth-only scan: no devices observed in the sample window
2. BlueZ + Ubertooth combined scan: no devices observed in the sample window
3. BlueZ-only scan with `--controller all`: no devices observed in a 10s sample window
4. BlueZ + Ubertooth scan with `--controller all --ubertooth-device all`: no devices observed in a 10s sample window

Blocked by hardware/runtime state:

1. `ubertooth-util` reported that it could not open the attached Ubertooth device

### SDR Hardware

Detected locally:

1. RTL-SDR
2. HackRF One
3. bladeRF
4. Ettus B210

Validated behavior:

1. RTL-SDR runtime starts
2. HackRF runtime starts
3. bladeRF runtime starts
4. Ettus B210 runtime path starts, but real UHD access is blocked on this host
5. SDR spectrum frames now use live IQ-backed FFT path for available capture tools (`rtl_sdr`, `hackrf_transfer`, `bladeRF-cli`, `uhd_rx_cfile`) with synthetic fallback if live capture fails or tooling is missing
6. SDR right-click decode actions now enforce hardware-constraint checks and provide explicit unavailable-decoder status feedback
7. POCSAG/DECT built-ins now attempt non-RTL fallback chains (HackRF/bladeRF/B210) when `csdr`, `sox`, `multimon-ng`, and capture tools are present
8. Built-in decoder launch path now honors hardware-specific plugin overrides for command templates, enabling per-device demod command customization from `sdr-plugins.json`
9. Runtime logs now identify the specific built-in override ID used when a hardware-specific override is active

Blocked by hardware/runtime state:

1. B210 validation is incomplete because UHD images are missing on this host
2. Ubertooth support is installed but local access was unavailable

### SDR Decoders

Collected during validation:

1. No meaningful live decode payloads were collected in the sampled windows below

Validated execution only:

1. `rtl_433`
2. `adsb`
3. `ais`
4. `pocsag`

Observed but not collected in the sampled windows:

1. `rtl_433`: no decode rows in the sample window
2. `adsb`: no aircraft rows in the sample window
3. `ais`: no decode rows in the sample window
4. `pocsag`: no meaningful decode rows in the sample window

Blocked by implementation or dependency:

1. `acars`: resolver is tightened to avoid known broken fallback invocation, but live decode validation is still pending on this host
2. `gsm_lte`: blocked because `gr-gsm`/`cell_search` is not installed
3. `iridium`: blocked because `iridium-toolkit` is not installed
4. `dect`: tooling not yet installed/validated
5. plugin decoders depending on missing local tools are not yet validated

Missing local SDR dependencies reported by the app:

1. `iridium-extractor`
2. `grgsm_livemon_headless` or `cell_search`
3. `dsd`
4. `csdr`
5. `satdump`
6. `radiosonde-auto-rx`
7. `op25`
8. `dsd-fme`
9. `gr-droneid`
10. `opendroneid`
11. `stdc-decoder`
12. `freedv`
13. `leandvb`
14. `tvheadend`
15. `gr-lora`
16. `m17-tools`
17. `jaero`
18. `osmo-tetra`
19. `dump978`
20. `srsran`

## Outstanding Implementation Gaps

1. Bluetooth multi-controller support across more than one BlueZ controller at once
2. Multi-SDR simultaneous operation in the SDR tab
3. Live SDR FFT/waterfall backed by hardware IQ instead of synthetic frames
4. ACARS runtime still needs live on-air validation after decoder command updates
5. Broader decoder dependency installation and validation

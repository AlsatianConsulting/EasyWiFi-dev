# Validation Ledger

This document tracks what EasyWiFi currently outputs and what was validated on this host.

## Output Inventory

When file output is enabled, each session creates artifacts under the configured output root:

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

## Runtime Validation Coverage

Wi-Fi capture regression checks include:

1. Decoder startup tries the configured packet header mode with fallback (`PPI` to `Radiotap` when needed).
2. Parse path supports `radiotap.dbm_antsignal` and `ppi.dbm_antsignal`.
3. RSSI selection prefers radiotap and falls back to PPI.
4. Geiger-mode RSSI parsing supports radiotap/PPI fallback.

Summary/CSV/KML checks include:

1. AP/client/bluetooth summary fields are serialized in JSON export.
2. KML folders are produced for AP/client/bluetooth datasets.
3. Bluetooth location CSV includes normalized transport class values.
4. UI runtime heartbeat line reports Wi-Fi/Bluetooth state (`wifi/bt`).

Multi-adapter checks include:

1. Wi-Fi capture supports multiple enabled interfaces in one session.
2. Device records retain `source_adapters` provenance.
3. Bluetooth scan supports default, specific, or `all` BlueZ controllers.
4. Bluetooth scan supports default, specific, or `all` Ubertooth devices.

## Live Validation Notes (This Host)

Wi-Fi:

1. Monitor-mode path validated on `wlx1cbfcef8e928`.
2. App-style tshark capture works without `-I` when interface is already in monitor mode.
3. Noninteractive Wi-Fi test mode accepts both `--packet-headers radiotap` and `--packet-headers ppi`.

Bluetooth:

1. BlueZ passive scan path starts and returns device observations.
2. BlueZ scan accepts `--controller all`.
3. BlueZ + Ubertooth path accepts combined controller/device settings.

## Not Yet Fully Validated

1. Long-duration multi-interface Wi-Fi sampling across highly variable RF environments.
2. Multi-controller Bluetooth validation on hosts with more than one physical BlueZ controller.
3. Ubertooth collection quality across all Ubertooth hardware variants/firmware combinations.

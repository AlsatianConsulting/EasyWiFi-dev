# Current Status

Generated: 2026-03-15T21:39:10+00:00
Branch: `main`

## High-level product state

WirelessExplorer is currently a working Rust/GTK desktop application with three mature areas and one expanding area:

1. Wi-Fi passive observation
2. Bluetooth passive observation and enumeration support
3. export/storage pipeline
4. SDR scaffolding with decoder process integration and dependency management

The app is not finished. The Wi-Fi/Bluetooth path is materially ahead of the SDR path.

## What is implemented now

### Wi-Fi

1. Access Points tab with sortable/paged table.
2. Per-column filtering.
3. Toggleable column filter bars (View + Preferences).
4. Watchlist row highlighting with named watchlist entry column.
5. AP details pane with identity, security, country, notes, and packet pie chart.
6. AP-associated clients pane scoped to current AP association.
7. RSSI geiger tab with lock/unlock workflow.
8. Multi-interface Wi-Fi runtime support.
9. Multi-interface Wi-Fi noninteractive test mode.
10. `--packet-headers radiotap|ppi` support in Wi-Fi test mode.
11. Optional inline Channel Usage panel on the AP tab.
12. CSV/JSON/KML/PCAPNG output pipeline.
13. SQLite-backed session storage.
14. GPS-aware export plumbing.
15. Output GPS coordinates now honor configured static GPS settings when valid, with default fallback.
16. Interactive Wi-Fi decode parsing now reads both radiotap and PPI RSSI fields (radiotap preferred, PPI fallback) to match packet-header mode behavior.
17. Wi-Fi geiger capture now parses both radiotap and PPI RSSI fields with fallback behavior.
18. Interactive Wi-Fi decoder startup now follows packet-header mode selection with fallback (`PPI` then `Radiotap`), aligned with PCAP saver behavior.
19. Bluetooth KML/KMZ ExtendedData now includes richer transport metadata and active-enumeration context fields for external GIS use.
20. SDR spectrum frame generation now uses live IQ capture + in-process FFT for RTL-SDR/HackRF/bladeRF/B210 tool paths when available, with synthetic fallback for unavailable/failed live capture.
21. SDR right-click frequency workflow supports per-signal decoder launch with hardware-constraint guardrails and clearer unavailable-decoder status messages.
22. Non-RTL fallback decode pipelines for POCSAG/DECT are now wired through capture-tool + csdr/sox/multimon chains when dependencies are present.

### Bluetooth

1. Bluetooth tab with BT/BLE list.
2. Passive scan support via BlueZ.
3. Support for BlueZ controller selection and `all` controller selection.
4. Ubertooth integration path.
5. Bluetooth detail pane with passive data and active enumeration sections.
6. Bluetooth geiger/locate workflow.
7. Bluetooth actions routed to the adapter that actually observed the selected device.

### SDR

1. SDR tab exists.
2. SDR runtime can start/stop decoder subprocesses.
3. Plugin-backed decoder inventory exists via `sdr-plugins.json`.
4. Decoder dependency detection and install-plan logic exist.
5. Basic noninteractive SDR test mode exists.
6. RTL-SDR / HackRF / bladeRF / B210 detection paths are wired.
7. Center-frequency geiger indicators in SDR UI (RSSI, tone, activity).
8. Optional center-geiger-driven auto squelch in SDR UI.
9. Bookmarks/presets plus scan-range/scan-speed/squelch controls are present.
10. User-added SDR bookmarks persist in app settings.
11. One-click SDR operator presets can apply tuned scan/squelch profiles.
12. User-defined SDR operator presets can be saved and persist in app settings.
13. User-defined SDR presets can be renamed, deleted, and reordered from SDR controls.
14. User-defined SDR presets can be exported/imported via JSON for migration.
15. SDR map and satcom audit records now include both parsed message and raw decoder text, with redaction applied before satcom derivation when no-payload mode is enabled.

### Packaging / transfer

1. `scripts/setup_ubuntu.sh` exists.
2. `scripts/build_deb.sh` exists.
3. `packaging/wirelessexplorer.desktop` exists.
4. `.deb` packaging path exists.

## What is partial

1. SDR FFT/waterfall now has live IQ ingestion support across hardware tool paths, but live RF validation and capture-format robustness across hosts remain partial.
2. SDR decoder execution exists with improved multi-device command tuning for some decoders, but several protocol decoders remain toolchain-limited or RTL-specific in this runtime.
3. Bluetooth multi-adapter support is implemented in code paths, but live validation on this host is limited.
4. Geoiger-style Wi-Fi workflows exist, but ongoing live RF validation depends heavily on the RF environment.
5. Export/format expansion requested later (for example richer KMZ iconography and full KMZ icon policy) is not complete.
6. KMZ folder/property/style coverage now has regression tests, but live GIS-tool interoperability is still pending.
7. Bluetooth KML style policy improved (BLE vs classic style IDs, including transport-string normalization), but broader icon policy is still pending.
8. Summary JSON now covers additional model fields (`band`, `wps`, bluetooth `active_enumeration`) with regression coverage, but live external consumer validation is still pending.

## What is not implemented yet

1. Multiple SDR devices running simultaneously in one session.
2. Real wideband SDR spectrum scanning workflow matching the requested HAVOC/gqrx-style interaction level.
3. Meshtastic / Meshcore decoding.
4. ACARS/ADS-B aircraft correlation.
5. GSMTAP export.
6. Full GSM/LTE/CDMA metadata tooling integration.
7. All requested SDR decoder families and mapping workflows.
8. Complete KMZ iconography/color policy for all requested object classes.
9. Fully live-validated static fake GPS workflow for all output paths.

## Current outputs

When file output is enabled, the app currently creates session trees with these outputs:

1. AP CSV exports
2. Client CSV exports
3. AP/client/Bluetooth location CSVs
4. JSON summary
5. KML trees for AP/client/Bluetooth observations
6. consolidated PCAPNG
7. GPS-annotated PCAPNG
8. per-handshake PCAPNGs
9. session logs
10. rolling SDR logs when enabled for active decoders

See `docs/VALIDATION.md` for the most current validated output list.

## Last completed work before continuity packaging

1. Bluetooth actions were corrected to use the adapter/controller that actually observed the device.
2. Wi-Fi noninteractive test mode was extended to support multiple interfaces in a single invocation.
3. SDR dependency coverage was extended so plugin-defined decoders report missing tools accurately.
4. Filter/watchlist alignment work had been ongoing and was being visually validated.
5. AP details scoping was being checked so selecting an AP only shows clients associated to that AP, rather than broader historical observations.

## Known current code direction

The codebase is currently in a large in-progress state with many modified files beyond the last committed checkpoint. The continuity work is meant to preserve that context and the recovery path on the next machine.

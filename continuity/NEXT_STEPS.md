# Next Steps

This is the recommended follow-on plan after restoring the repo on the new machine.

## Priority 0: bring-up on the new box

1. Run `bash continuity/bootstrap_ubuntu.sh`.
2. Build with `cargo test -q` and `cargo build -q`.
3. Verify hardware visibility:
   - Wi-Fi adapters
   - Bluetooth controller(s)
   - Ubertooth
   - RTL-SDR
   - HackRF
   - bladeRF
   - B210
4. Re-run noninteractive validation commands from `LAST_SESSION.md`.

## Priority 1: stabilize current Wi-Fi/Bluetooth behavior

1. Re-check watchlist row alignment and table filter placement after migration.
2. Confirm AP selection restricts the associated-client pane strictly to the selected AP.
3. Re-validate multi-adapter Wi-Fi runtime behavior.
4. Validate opt-in Wi-Fi parsing mode transitions (disabled capture-only vs enabled live parsing) and resource impact in dense RF environments.
5. Re-validate BlueZ + Ubertooth routing and enumeration on the new host.
6. Re-validate column filter visibility toggle and persistence.

## Priority 2: finish the remaining core export requests

1. Re-validate radiotap vs PPI behavior in live interactive runtime/export flows (decoder/saver fallback logic is now implemented and regression-tested).
2. Add/verify GPS PPI export behavior where requested.
3. Add KMZ output with requested icon/color scheme:
   - Wi-Fi APs colored by encryption class
   - Wi-Fi clients
   - Bluetooth devices
4. Ensure KML/KMZ properties include relevant per-hit detail in external GIS tools (internal regression coverage is in place).
5. Re-validate summary JSON in downstream consumers after recent model-field expansion (`band`, `wps`, bluetooth `active_enumeration`, transport class normalization).
6. Re-validate static GPS output behavior in a live session after migration.

## Priority 3: advance SDR from scaffolding to real operator workflow

1. Validate live IQ-backed FFT/waterfall on-air across each hardware path and harden any host-specific capture-format/tooling mismatches.
2. Validate center-frequency geiger + auto-squelch behavior with live RF input.
3. Validate preset save/rename/delete/reorder/import/export workflow end-to-end on a fresh machine profile.
4. Validate satcom no-payload redaction in live decoder runs (decode/map/satcom logs).
5. Validate live `inmarsat_stdc` built-in decoder path and parsed satcom payload-field extraction quality on-air.
6. Validate persisted satcom parser denylist behavior from SDR controls and env fallback interaction.
7. Validate new satcom export artifacts (CSV/parsed-only/denied-only/summary JSON) in downstream tooling.
8. Validate decoder telemetry counters against expected decode/log rates during long SDR runs.
9. Validate map/satcom `message` + `raw` side-by-side logging in live decoder runs.
10. Validate corrected ACARS runtime integration live.
11. Validate expanded non-RTL AIS/ACARS fallback command paths on HackRF/bladeRF/B210 hosts.
12. Validate ADS-B live and correlate with ACARS by aircraft identifier when possible (artifact workflow now implemented; live validation still pending).
13. Validate newly added non-RTL POCSAG/DECT fallback chain quality and tune audio/FM pipeline parameters per hardware.
14. Validate `FCC Area Explorer (CSV...)` workflow with real FCC assignment exports for multiple cities/states and tune parser header mappings where needed.
15. Validate `FCC Frequency Explorer` bookmark import with large FCC assignment CSVs (label quality, dedup behavior, and SDR bookmark UI refresh persistence).
16. Validate `FCC Area Explorer (CSV URL)` on multiple public FCC-compatible CSV endpoints and harden retry/error messaging for unstable upstream hosts.
17. Validate `FCC Frequency Explorer (CSV URL -> Bookmarks)` with large regional exports and ensure bookmark label readability for long station/service strings.
18. Validate `Export Aircraft Correlation` artifacts (JSON/CSV) during mixed ADS-B + ACARS live runs and confirm merged identity quality (`icao_hex`/`callsign`).
19. Validate `Export Decode JSON` artifacts (JSON/CSV) for large decoder sessions and downstream parser compatibility.
20. Validate `Validate Decoder` dry-run readiness checks across RTL-SDR/HackRF/bladeRF/B210 and compare outputs with live start behavior.
21. Validate `Import SDR Bookmarks CSV` against large bookmark inventories and mixed `frequency_hz`/`frequency_mhz` sources.
22. Validate `Decode Bookmark` one-click workflow (tune + decoder start) across hardware classes with unavailable-decoder guardrail messaging.
23. Validate `Export SDR Health JSON` snapshot contents during long decoder runs and ensure downstream tooling can parse telemetry/rate sections.
24. Validate `Export Decode (Filtered)` output fidelity against decode-table column filters in interactive sessions.
25. Validate `Import SDR Bookmarks CSV URL` against stable/unstable endpoints and confirm retry/error messaging quality.
26. Validate local-vs-Zulu time mode toggle across AP/client/bluetooth/SDR table timestamps, GPS/status panels, SDR CSV timestamp fields, SDR decoder text logs, and SDR summary JSON artifact time fields.
27. Validate bookmark export artifacts in both CSV and JSON forms for schema and source-tag consistency.
28. Validate `Import SDR Bookmarks JSON` with mixed numeric/string frequency fields and both supported top-level schemas (array and `{ "bookmarks": [...] }`).
29. Validate `Import SDR Bookmarks JSON URL` against stable/unstable endpoints and confirm retry/error messaging quality.
30. Validate parser auto-detection when bookmark import file extensions do not match actual CSV/JSON content.
31. Validate `gsm_lte` launch behavior across RTL-SDR/HackRF/bladeRF/B210 and verify command-line args include frequency + correct Soapy driver on non-RTL devices.
32. Validate ADS-B resolver fallback order on RTL-only installs (`dump1090`, `dump1090-mutability`, `dump1090-fa`, `readsb`) and confirm status messaging clarity.
33. Validate AIS RTL fallback behavior when `rtl_ais` is absent but `rtl_fm` + `aisdecoder` are available.
34. Validate ACARS non-RTL fallback behavior when `acarsdec` is missing but `csdr/sox/multimon-ng` toolchain is present.
35. Validate APRS/POCSAG/DECT RTL fallback behavior when `rtl_fm` is absent but `rtl_sdr/csdr/sox/multimon-ng` are present.
36. Validate `Import SDR Bookmarks URL (Auto CSV/JSON)` against mixed endpoint formats and extension mismatches.
37. Validate right-click decoder unavailable tooltips for consistency with start-action/status-line failure reasons.
38. Validate `Import SDR Bookmarks File (Auto CSV/JSON)` with mixed-format local files and mismatched file extensions.
39. Validate BLE frequency presets against channel mapping expectations (data channels `0-36`, advertising channels `37-39`).
40. Validate BLE scanner preset behavior (`scan_ble_data`) and ensure stepping/coverage excludes advertising-only center offsets.
41. Validate enhanced missing-tool decoder hints for `rtl_433`, `ADS-B`, and `GSM/LTE` against real host toolchain states.
42. Validate duplicate-frequency bookmark imports to confirm placeholder labels are upgraded and persisted in runtime + settings lists.
43. Validate Zigbee scanner preset behavior (`scan_zigbee24`) and verify expected 2.4 GHz channel stepping coverage.
44. Validate bookmark import alias handling for `freq` fields across CSV and JSON data sources.
45. Validate extended unavailable-decoder hints for RTL AIS and satellite decoder toolchains against real host dependency states.
46. Validate Bluetooth context-menu BLE scan shortcut updates active SDR runtime scan range and persists reusable BLE operator presets.
47. Validate ambiguous `frequency` field auto-detection (Hz vs MHz) across CSV/JSON imports and URL/file workflows.
48. Validate Bluetooth context-menu Zigbee scan shortcut updates active SDR runtime scan range and persists reusable Zigbee operator presets.
49. Validate Thread channel preset entries (`thread_ch11`..`thread_ch26`) for expected 2.4 GHz center-frequency mapping.
50. Validate bookmark import range guard behavior to confirm out-of-range values (`<100 kHz` or `>8 GHz`) are discarded across CSV/JSON paths.
51. Validate Thread scanner preset (`scan_thread24`) and Bluetooth Thread scan shortcut runtime/preset application behavior.
52. Validate JSON bookmark envelope compatibility for `bookmarks`/`rows`/`items`/`data` top-level keys.
53. Validate URL extension inference in auto bookmark URL import (`.csv`/`.json` path endings and extension-less URLs).
54. Validate CSV `bookmark,name` import variants where `name` is a numeric frequency field.
55. Validate nested JSON bookmark envelope compatibility (for example `data.bookmarks`, `rows.items`) across file and URL import paths.
56. Validate deep JSON envelope compatibility (for example `payload.result.items`) across file and URL import paths.
57. Validate unit-suffixed bookmark/FCC frequency parsing (`Hz`/`kHz`/`MHz`/`GHz`, plus compact `k`/`M`/`G`) across CSV/JSON/file/URL import workflows.
58. Validate FCC explorer compatibility with extended assignment header aliases (`frequency_assigned_hz`, lower/upper `*_hz`, `tx_frequency*`, `rx_frequency*`) and confirm midpoint/range derivation behavior.
59. Validate `Decode Bookmark` auto-decoder fallback behavior (bookmark-label hinting + protocol-priority ordering) across RTL-SDR/HackRF/bladeRF/B210 with unavailable-decoder guardrail messaging.

## Priority 4: requested SDR decoder backlog

Highest practical decoder backlog in the order requested:

1. `rtl_433`
2. `ADS-B`
3. `ACARS`
4. `AIS`
5. `POCSAG`
6. Meshtastic metadata support (plugin preset implemented; prioritize live RF/toolchain validation)
7. Meshcore metadata support (plugin preset implemented; prioritize live RF/toolchain validation)
8. radiosonde
9. APRS / AX.25 (implemented; prioritize live RF validation)
10. weather satellite APT (implemented in presets/macros + plugin path; prioritize live RF/toolchain validation)
11. P25 metadata/audio path
12. DECT metadata/audio path
13. GSM/LTE safe metadata path where policy permits
14. drone metadata decoders
15. remaining plugin decoders after dependency installation and live validation

## Priority 5: broad roadmap items still requested but not yet done

1. Multiple SDRs simultaneously.
2. gqrx-style control depth.
3. PortaPack HAVOC-like wideband interaction features.
4. GSMTAP output.
5. richer satcom non-content metadata workflows.

## Do not re-open without explicit user confirmation

1. IP/content inspection features removed from scope.
2. user-agent extraction.
3. domains visited.
4. nationality profiling from HTTP language settings.
5. GeoIP analysis tied to packet content.

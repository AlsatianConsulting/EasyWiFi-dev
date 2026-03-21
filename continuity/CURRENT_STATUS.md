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
23. Built-in SDR decoders now support hardware-specific plugin command overrides (`<decoder_id>_<hardware_id>` / `<decoder_id>__<hardware_id>`) for cross-device demod customization.
24. Wi-Fi frame parsing is now an explicit opt-in setting (default off) with capture-only fallback and UI warnings about higher resource usage when enabled.
25. Satcom observation pipeline now includes unencrypted payload field parsing metadata (`payload_parse_state`, normalized parsed fields) and a built-in Inmarsat STD-C decoder path (`inmarsat_stdc`) when compatible tools are installed.
26. Satcom parser supports protocol/decoder denylist filtering via `WIRELESSEXPLORER_SATCOM_PARSE_DENYLIST` (comma-separated tokens), yielding `denied_by_policy` parse state without disabling full satcom observation.
27. Satcom parser denylist is now configurable from SDR controls and persisted in settings (env fallback remains supported).
28. SDR pane now shows decoder telemetry counters (`decoded_rows`, `map_points`, `satcom_rows`, `stderr_lines`) for runtime health monitoring.
29. SDR satcom export now emits JSON + CSV + parsed-only JSON + denied-only JSON artifacts in one action.
30. SDR map plotting now renders recent protocol trails and highlights the latest coordinate-bearing decode point.
31. Built-in ACARS/AIS command resolution now includes additional non-RTL fallback pipeline attempts when compatible toolchains are present.
32. Built-in APRS/AX.25 decoder path is now available with RTL and non-RTL fallback command resolution, including right-click launch parity across SDR hardware classes.
33. Meshtastic and Meshcore metadata plugin presets are now included in `sdr-plugins.json` with dependency-plan integration and protocol autotune defaults.
34. Top-level `Presets` menu now provides grouped default frequency selections for Wi-Fi channels, Bluetooth frequencies, pager bands, and satellite targets; selection updates/tunes SDR center frequency.
35. Presets now include additional DECT/DMR/IoT/ISM defaults and scanner profiles that auto-configure center/sample-rate/range/step/squelch and enable scan mode from the menu.
36. Scanner preset catalog now covers 2.4 GHz, Wi-Fi 5/6E, DECT, pager VHF/UHF, and satellite L-band ranges with sane scan-step defaults.
37. Scanner menu now includes user-saved scan profiles (from the existing editable SDR preset system), so custom range/squelch workflows are accessible directly under `Presets`.
38. Satellite payload reception is now an explicit persisted enable/disable control; satcom records/exports include `payload_capture_mode` and retain full unencrypted payload text when enabled.
39. SDR runtime payload control now uses explicit positive `satcom payload capture` semantics end-to-end (UI -> runtime -> observation/log/export path).
40. SDR test-mode CLI now supports explicit positive payload capture flag (`--satcom-payload-capture`), while retaining backward-compatible `--allow-satcom-payload`.
41. Presets now include protocol-focused `Scan Macros` that apply decoder target + scan range/speed/squelch in one action.
42. Weather satellite APT scanning now has dedicated presets/macro coverage, and plugin override hooks are present for RTL-SDR/HackRF/bladeRF/B210 demod paths.
43. Scan macro catalog now includes additional core workflows (ADS-B, ACARS, AIS, APRS/AX.25, and P25) for one-click decoder/range setup.
44. SDR dependency status now tracks weather APT plugin requirements explicitly, including satdump-backed decoder readiness.
45. Presets now include an `FCC Area Explorer (CSV...)` workflow that filters FCC-assignment rows by city/area token and auto-builds a scan profile (center/range/step/squelch), applies it live, and persists it to saved SDR presets.
46. Presets `Frequencies` submenu now includes FCC explorer workflows for both area scan profile generation and per-frequency bookmark import, with signal/station type labeling where present in source data.
47. FCC area explorer now also supports direct CSV URL ingestion from within the app (no pre-download required), then applies/persists the generated scan profile.
48. FCC URL ingestion now uses retry+timeout handling, and frequency-bookmark explorer also supports direct CSV URL import under `Presets -> Frequencies`.
49. FCC frequency-bookmark import status now reports added vs skipped-duplicate counts to improve large-import auditability.
50. FCC frequency-bookmark explorers (CSV and URL) now support optional signal/service-type filtering during import.
51. FCC area explorer workflows (CSV and URL) now support optional signal/service-type filtering for targeted scan-profile generation.
52. Presets and scan macros now include radiosonde-focused defaults (400-406 MHz sweep and common sonde frequencies) tied to existing RS41 decoder plugin workflow.
53. Radiosonde RS41 plugin dependency status is now explicitly surfaced through SDR dependency planning (`rtl_433` mapping).
54. SDR bookmark state now remains synchronized/sorted across manual adds and FCC import workflows (settings, in-memory list, and combo UI stay aligned).
55. Persisted SDR bookmark settings are now normalized (invalid entries removed, sorted by frequency, duplicates deduplicated) during add/import workflows.
56. Scanner presets now include dedicated public-safety P25 ranges (700/800 MHz) for faster targeted trunking-band sweeps.
57. SDR controls now include a `Scan Around Bookmark` quick action with configurable ±kHz window to convert any selected bookmark (including FCC imports) into an active scan range.
58. FCC area import workflows now attempt decoder auto-selection from detected FCC signal/service type (for example public safety -> P25, maritime -> AIS) when matching decoders are available.
59. Scan macro catalog now also includes Iridium L-band and GSM/LTE metadata sweeps for one-click workflow coverage of those decoder families.
60. Scanner preset catalog now also includes dedicated Iridium and GSM/LTE metadata scan ranges (in addition to macro shortcuts).
61. FCC area explorer status now explicitly reports the auto-selected decoder ID (or `none`) for clearer operator feedback.
62. `Presets -> Frequencies` now includes a one-click `Remove FCC Bookmarks` action that prunes FCC-imported bookmark entries from runtime and persisted settings.
63. `Scan Around Bookmark` now preserves the operator’s current scan speed and squelch settings instead of forcing fixed defaults.
64. `Presets -> Frequencies` now includes `Export SDR Bookmarks CSV`, emitting labeled bookmark inventory with source tagging (`fcc_imported` vs `manual_or_default`).
65. SDR controls now include `Export Aircraft Correlation`, which derives ADS-B/ACARS correlations (ICAO/callsign merged identity) from decode rows and exports JSON+CSV artifacts for downstream analysis.
66. SDR satcom export now emits an additional summary JSON artifact with aggregate metadata counters (protocol/decoder/band/posture/payload-parse/capture state plus coordinate and identifier-hint rollups).
67. SDR pane now surfaces a live aircraft-correlation status line showing correlated target counts and mixed ADS-B+ACARS linkage counts from current decode rows.
68. SDR pane now surfaces a live satcom summary status line with parse-state and encryption-posture counters for quick non-content policy/runtime monitoring.
69. SDR controls now include `Export Decode JSON`, which exports raw decode rows as JSON+CSV artifacts for offline triage alongside map/satcom/correlation exports.
70. SDR controls now include `Validate Decoder`, a dry-run command/dependency readiness check for the selected decoder/hardware/frequency configuration.
71. `Presets -> Frequencies` now includes `Import SDR Bookmarks CSV`, supporting `frequency_hz` or `frequency_mhz` columns with dedupe/normalization and live bookmark refresh.
72. SDR bookmark controls now include `Decode Bookmark`, which tunes to the selected bookmark and starts the selected decoder in one action (with existing hardware/dependency guardrails).
73. SDR controls now include `Export SDR Health JSON`, capturing decoder telemetry/rates plus satcom and aircraft-correlation summaries as a lightweight runtime snapshot artifact.
74. SDR controls now include `Export Decode (Filtered)`, exporting only decode rows matching active decode-table column filters as JSON+CSV artifacts.
75. `Presets -> Frequencies` now includes `Import SDR Bookmarks CSV URL`, allowing direct bookmark imports from remote CSV sources via existing retry/timeout fetch behavior.
76. Channel status pane now includes a live `Runtime Activity` heartbeat line that updates once per second with current Wi-Fi/Bluetooth/SDR runtime states.
77. Time display now defaults to local time across UI tables/status timestamps, with a Settings toggle to switch to Zulu (`UTC`) display mode.
78. `Export SDR Bookmarks CSV` now also emits a JSON companion artifact (`sdr_bookmarks.json`) with source tagging for downstream automation.
79. SDR CSV exports that include timestamp fields (decode/satcom/aircraft-correlation) now honor the local-vs-Zulu time display setting.
80. SDR decoder text log timestamp rendering now follows the same local-vs-Zulu mode selected in settings.
81. SDR summary JSON artifacts now render human-readable `generated_at`/window timestamps using the selected local-vs-Zulu display mode.
82. `Presets -> Frequencies` now includes `Import SDR Bookmarks JSON`, supporting array root or `{ "bookmarks": [...] }` schema with the same dedupe/normalization behavior as CSV import.
83. `Presets -> Frequencies` now includes `Import SDR Bookmarks JSON URL`, using existing retry/timeout fetch guardrails for remote bookmark ingest.
84. SDR bookmark import actions now auto-detect CSV vs JSON on unknown/mismatched file extensions, reducing operator errors during manual and URL-based imports.
85. GSM/LTE built-in decoder command resolution now injects explicit center-frequency args and non-RTL Soapy driver args, improving cross-hardware launch compatibility.
86. ADS-B built-in command resolution now recognizes `dump1090-fa` as an additional RTL fallback, improving out-of-box decoder availability on feeder-focused installs.
87. AIS built-in command resolution on RTL now falls back to an explicit `rtl_fm -> aisdecoder` pipeline when `rtl_ais` is unavailable.
88. ACARS built-in command resolution no longer hard-gates on `acarsdec` for non-RTL hardware, allowing valid `csdr/sox/multimon-ng` fallback pipelines to launch.
89. SDR bookmark file-import dialogs now include CSV/JSON filters (plus mixed data/all-file filters) while keeping format auto-detection enabled.
90. APRS/POCSAG/DECT built-in command resolution on RTL now falls back to an `rtl_sdr + csdr + sox + multimon-ng` IQ pipeline when `rtl_fm` is unavailable.
91. `Presets -> Frequencies` now includes `Import SDR Bookmarks URL (Auto CSV/JSON)`, using shared retry/timeout fetch behavior plus parser auto-detection.
92. SDR right-click decode menus now attach hover tooltips with the exact unavailable-reason text for disabled decoder entries.
93. Bookmark URL import actions (CSV/JSON/Auto) now use a shared status/reporting path for consistent added/duplicate/skipped/error messaging.
94. `Presets -> Frequencies` now includes `Import SDR Bookmarks File (Auto CSV/JSON)`, and file-based CSV/JSON actions share the same normalized import/status reporting path.
95. Bluetooth frequency presets now include BLE data channels (`0-36`) alongside existing Classic and BLE advertising channel presets.

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
4. ACARS/ADS-B aircraft correlation remains partial (artifact export implemented; live RF validation pending).
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

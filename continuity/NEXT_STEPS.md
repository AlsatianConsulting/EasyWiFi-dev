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
7. Validate new satcom export artifacts (CSV/parsed-only/denied-only JSON) in downstream tooling.
8. Validate decoder telemetry counters against expected decode/log rates during long SDR runs.
9. Validate map/satcom `message` + `raw` side-by-side logging in live decoder runs.
10. Validate corrected ACARS runtime integration live.
11. Validate expanded non-RTL AIS/ACARS fallback command paths on HackRF/bladeRF/B210 hosts.
12. Validate ADS-B live and correlate with ACARS by aircraft identifier when possible.
13. Validate newly added non-RTL POCSAG/DECT fallback chain quality and tune audio/FM pipeline parameters per hardware.

## Priority 4: requested SDR decoder backlog

Highest practical decoder backlog in the order requested:

1. `rtl_433`
2. `ADS-B`
3. `ACARS`
4. `AIS`
5. `POCSAG`
6. Meshtastic metadata support
7. Meshcore metadata support
8. radiosonde
9. APRS / AX.25 (implemented; prioritize live RF validation)
10. weather satellite APT
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

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
4. Re-validate BlueZ + Ubertooth routing and enumeration on the new host.
5. Decide whether filter controls should remain always visible or become toggleable.

## Priority 2: finish the remaining core export requests

1. Add/verify radiotap vs PPI export selection.
2. Add/verify GPS PPI export behavior where requested.
3. Add KMZ output with requested icon/color scheme:
   - Wi-Fi APs colored by encryption class
   - Wi-Fi clients
   - Bluetooth devices
4. Ensure KML/KMZ properties include relevant per-hit detail.
5. Ensure summary JSON reflects the current model cleanly.
6. Verify static test GPS coordinates if that remains desired.

## Priority 3: advance SDR from scaffolding to real operator workflow

1. Replace synthetic/placeholder spectrum frames with live IQ-backed FFT/waterfall.
2. Implement center-frequency geiger workflow.
3. Add bookmarks/presets.
4. Add scan-range, scan-speed, and squelch controls.
5. Add map plotting for coordinate-bearing decoders.
6. Save map source data and raw decoder text side-by-side.
7. Correct ACARS runtime integration and validate it live.
8. Validate ADS-B live and correlate with ACARS by aircraft identifier when possible.

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
9. APRS / AX.25
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
5. Channel Usage as inline AP-panel toggle.
6. richer satcom non-content metadata workflows.

## Do not re-open without explicit user confirmation

1. IP/content inspection features removed from scope.
2. user-agent extraction.
3. domains visited.
4. nationality profiling from HTTP language settings.
5. GeoIP analysis tied to packet content.

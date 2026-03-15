# Dependencies and Tools

## Core build/runtime dependencies

These are the baseline packages required to build and run the main app on Ubuntu.

### APT packages

1. `build-essential`
2. `pkg-config`
3. `curl`
4. `git`
5. `clang`
6. `cmake`
7. `libgtk-4-dev`
8. `libglib2.0-dev`
9. `libcairo2-dev`
10. `libpango1.0-dev`
11. `libgdk-pixbuf-2.0-dev`
12. `libsqlite3-dev`
13. `libssl-dev`
14. `libasound2-dev`
15. `libpcap-dev`
16. `libclang-dev`
17. `iw`
18. `tshark`
19. `gpsd`
20. `gpsd-clients`
21. `beep`
22. `bluez`
23. `gdb`
24. `ripgrep`
25. `jq`

## Rust toolchain

1. `rustup`
2. stable toolchain
3. `cargo`
4. `rustc`

Current host versions at continuity capture time:

1. `cargo 1.93.1`
2. `rustc 1.93.1`
3. `git 2.34.1`
4. Ubuntu `22.04.5 LTS`

## Runtime helper tools currently present on this host

These are installed on the current development host and are useful to reproduce the current validation surface on the new machine.

1. `iw`
2. `tshark`
3. `gpsd`
4. `gpspipe`
5. `bluetoothctl`
6. `btmgmt`
7. `rtl_sdr`
8. `rtl_433`
9. `acarsdec`
10. `rtl_ais`
11. `multimon-ng`
12. `direwolf`
13. `ubertooth-util`
14. `bladeRF-cli`
15. `hackrf_info`
16. `uhd_find_devices`
17. `welle-cli`

## SDR / RF optional tools requested or partially wired

These are either already used by the codebase, referenced in `sdr-plugins.json`, or requested for validation. Some are missing on the current host.

### Present on the current host

1. `rtl_sdr`
2. `rtl_433`
3. `acarsdec`
4. `rtl_ais`
5. `multimon-ng`
6. `direwolf`
7. `ubertooth-util`
8. `bladeRF-cli`
9. `hackrf_info`
10. `uhd_find_devices`
11. `welle-cli`

### Missing on the current host at continuity capture time

1. `dump1090-fa` or `dump1090`
2. `dumpvdl2`
3. `grgsm_livemon_headless`
4. `cell_search`
5. `satdump`
6. `op25_rx.py`
7. `rx.py`
8. `dsd`
9. `dsd-fme`
10. `csdr`
11. `droneid_receiver`
12. `droneid_decode`
13. `opendroneid_rx`
14. `opendroneid-decode`
15. `odid-decode`
16. `freedv_rx`
17. `leandvb`
18. `m17-demod`
19. `jaero`
20. `tetra-rx`
21. `osmo-tetra`
22. `dump978`
23. `dump978-fa`
24. `srsran`
25. `satdump`
26. `gr-gsm`
27. `iridium-toolkit` / `iridium-extractor`

## Hardware validated or attached during development

1. monitor-mode Wi-Fi adapters including `wlx1cbfcef8e928`
2. internal Wi-Fi adapter `wlp0s20f3`
3. BlueZ Bluetooth controller `D0:C6:37:4D:3E:05`
4. Ubertooth
5. RTL-SDR
6. HackRF One
7. bladeRF
8. Ettus B210

## Additional notes

1. B210 needs UHD images on the new box:
   - `sudo uhd_images_downloader`
2. USB SDR devices may require unplug/replug after package install or udev-rule refresh.
3. `tshark` package installation may ask about capture permissions; root-mode testing is still the most reliable path used in this project.
4. `GeoLite2-City.mmdb` support was introduced earlier, but packet-inspection/GeoIP workflows were later removed from scope and should not be reintroduced unless the user changes direction again.

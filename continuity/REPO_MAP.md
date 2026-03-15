# Repository Map

## Entry points

1. `src/main.rs`
   - top-level binary dispatch
   - starts GTK UI or noninteractive test modes
2. `src/lib.rs`
   - crate module exports
3. `src/bin/wirelessexplorer-helper.rs`
   - privileged helper used for monitor mode, channel operations, and delegated tshark execution
4. `src/bin/simplestg-helper.rs`
   - legacy helper binary retained from the older project name

## Major modules

1. `src/ui/mod.rs`
   - GTK UI, tabs, dialogs, menus, filtering, watchlists, detail panes
2. `src/capture/mod.rs`
   - Wi-Fi capture orchestration, channel hopping, monitor mode, tshark integration
3. `src/bluetooth/mod.rs`
   - BlueZ/Ubertooth discovery, active enumeration support, adapter routing
4. `src/sdr/mod.rs`
   - SDR runtime, decoder plugin handling, dependency inventory/install planning
5. `src/export/mod.rs`
   - CSV, JSON, KML, PCAP/PCAPNG export logic
6. `src/storage/mod.rs`
   - SQLite persistence
7. `src/model/mod.rs`
   - shared data structures for APs, clients, Bluetooth devices, SDR state, watchlists, etc.
8. `src/settings.rs`
   - persistent/session settings structures
9. `src/oui/mod.rs`
   - OUI/manufacturer lookup support
10. `src/gps/mod.rs`
   - GPS provider abstraction and parsing
11. `src/test_mode.rs`
   - noninteractive CLI validation for Wi-Fi, Bluetooth, and SDR
12. `src/privilege.rs`
   - helper daemon protocol and privilege coordination

## Important config/data files

1. `sdr-plugins.json`
   - external decoder/plugin definitions
2. `assets/oui.csv`
   - bundled OUI data if present
3. `assets/bt_company_ids.csv`
   - bundled Bluetooth manufacturer IDs if present
4. `assets/bt_service_uuids.csv`
   - bundled Bluetooth UUID metadata if present
5. `manuf`
   - alternative manufacturer database if present

## Scripts and packaging

1. `scripts/setup_ubuntu.sh`
   - current project bootstrap script
2. `scripts/build_deb.sh`
   - `.deb` package build
3. `scripts/run.sh`
   - convenience run script
4. `packaging/wirelessexplorer.desktop`
   - desktop entry for packaged installation

## Current structural note

`src/netintel/mod.rs` was removed after packet-inspection requirements were explicitly taken out of scope.

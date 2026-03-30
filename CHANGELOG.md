# Changelog

## 0.1.1-rc1 - 2026-03-30

### Added
- Persistent BlueZ crash blacklist for unstable selected controllers.
- BlueZ scan circuit breaker/backoff for repeated controller failures (including default controller).
- Runtime-driven scan status header (`idle`, `Wi-Fi only`, `Bluetooth only`, `Wi-Fi + Bluetooth`).

### Changed
- Hardened Bluetooth controller selection persistence in UI settings/start dialog.
- Improved stop/export sequencing to ensure session snapshots are written on scan stop.

### Validation
- Automated GUI validation matrix (`wifi_bt`, `wifi_only`, `bt_only`) passing.
- 60-minute Wi-Fi + Bluetooth soak passing with successful start/stop/export and populated outputs.

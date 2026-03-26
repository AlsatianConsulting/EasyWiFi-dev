# Current Limitations

1. Passive capture quality depends on adapter, driver, firmware, antenna, and regulatory constraints. A supported interface can still observe sparse or zero traffic in short windows.
2. Wi-Fi geiger locating uses RSSI proximity only and is not a distance measurement.
3. Audible geiger tone uses external `beep` when present.
4. AP uptime is estimated from observed beacon TSF values and can reset or jump if AP state changes.
5. WPS details are limited to passively observed management traffic.
6. GPS status reflects stream health and fix availability, but reconnect/backoff behavior is intentionally simple.
7. Table layouts, filters, and watchlist settings are still evolving and may change between releases.
8. Bluetooth MFGR/UUID resolving is best-effort from bundled sources plus local data and is not exhaustive.
9. Bluetooth scan currently supports one selected BlueZ controller and one selected Ubertooth selection per scan session (`Both` means one of each selection path).
10. Wi-Fi test mode validates interfaces per invocation; coverage depends on sample window and local RF activity.

## Policy-Restricted Features Not Implemented

The following requests are intentionally not implemented:

1. IMSI/TMSI capture, IMSI-catcher behavior, or cellular subscriber-identity interception.
2. SMS/content interception or user-plane cellular traffic interception.
3. Replay, retransmit, injection, cloning, or other active attack workflows for Wi-Fi or Bluetooth protocols.
4. Decryption/cracking workflows for encrypted Wi-Fi, Bluetooth, or cellular traffic.

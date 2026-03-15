# Current Limitations

1. Passive capture quality depends on adapter, driver, firmware, antenna, and regulatory constraints. A supported interface can still observe sparse or zero traffic in a short sample window.
2. Wi-Fi geiger locating uses RSSI proximity only and is not a distance measurement.
3. Audible geiger tone currently uses external `beep` when present.
4. AP uptime is estimated from observed beacon TSF values and can reset or jump if the AP restarts or capture continuity is lost.
5. WPS details are limited to fields present in passively observed management traffic and may be sparse.
6. GPSD status reflects current stream health and fix availability, but reconnect and backoff behavior is still basic.
7. Layout, column visibility, filters, and watchlist configuration are currently session-scoped and not yet persisted across app restarts.
8. Bluetooth manufacturer and UUID resolving is offline and best-effort from bundled data plus local BlueZ naming; coverage is not exhaustive.
9. Bluetooth currently supports one selected BlueZ controller and one selected Ubertooth device per scan session. "Both" means one of each, not arbitrarily many controllers or Ubertooth devices at once.
10. SDR currently supports one selected radio per runtime. Multiple SDRs operating simultaneously in a single SDR session are not implemented yet.
11. Wi-Fi test mode validates one interface per invocation. The GUI/runtime Wi-Fi path supports multiple enabled interfaces per session, but the test harness is still single-interface.
12. Current SDR spectrum frames are synthetic placeholders. Decoder execution and process control are real, but the live FFT and waterfall path is not yet backed by hardware IQ ingestion.

## Policy-Restricted Features Not Implemented

The following requests were explicitly not implemented due to policy restrictions:

1. Subscriber-identity interception such as IMSI/TMSI extraction, IMSI-catcher behavior, or GSMEvil-style identifier capture.
2. SMS capture, cellular traffic interception, or user-plane/content interception for GSM, LTE, CDMA, or similar networks.
3. Payload recovery from third-party satellite or cellular communications where the goal is reading communication content rather than benign RF metadata.
4. Replay, retransmit, injection, cloning, or other active attack workflows for OOK, remotes, pagers, cellular, Wi-Fi, Bluetooth, SDR, or satcom protocols.
5. Decryption or cracking of encrypted Wi-Fi, Bluetooth, cellular, satellite, or SDR-delivered traffic.

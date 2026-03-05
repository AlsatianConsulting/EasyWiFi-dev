# Current Limitations

1. This environment has not completed a full `cargo check` because GTK/GLib system development libraries are not installed (`glib-2.0`, `gio-2.0`, `gobject-2.0` pkg-config entries missing).
2. Passive capture quality depends on adapter/driver/regulatory constraints (2.4/5/6 GHz, DFS access, monitor support).
3. "Geiger" locating uses RSSI proximity only and is not a distance measurement.
4. Audible geiger tone currently uses external `beep` utility when present.
5. AP uptime is estimated from observed beacon TSF values and can reset/jump if AP restarts or capture continuity is lost.
6. WPS details are currently limited to observed fields and may be sparse depending on captured frames.
7. GPSD status reflects current stream health and fix availability, but automatic reconnect/backoff is basic in this iteration.
8. Layout and watchlist configuration are currently session-scoped (not yet persisted to disk across app restarts).
9. Bluetooth MFGR ID / UUID resolving is offline and best-effort based on bundled SIG snapshot data plus locally available BlueZ naming; coverage is not exhaustive yet.

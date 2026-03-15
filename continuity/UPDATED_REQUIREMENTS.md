# Updated Requirements and Scope Changes

This project evolved substantially after the initial request. This file captures the major changes that matter for future work.

## Architectural decisions

1. Stack selected: `Rust + GTK`.
2. Project name history:
   - initial working name: `SimpleSTG`
   - final requested name: `WirelessExplorer`
3. SQLite was accepted as the primary efficient storage path.
4. OUI source:
   - bundled local database
   - update path preferred, but local-first behavior required

## Wi-Fi scope changes

1. WPA2 handshake definition fixed to a passive 4-way handshake observation.
2. WPA3 handshake counting explicitly not required.
3. Channel hopping default set to approximately 5 channels per second.
4. PDF output was later removed from scope.
5. Preferred exports shifted toward:
   - CSV
   - PCAP/PCAPNG
   - KML/KMZ
   - JSON summaries
6. Handshakes should be saved separately, including one beacon for offline use.
7. Output naming requirement for handshakes was added.
8. GPS-in-PCAPNG support was explicitly requested.
9. User later requested option for PPI/GPS-style output in addition to normal capture formats.
10. User later requested radiotap-vs-PPI style control.
11. User requested fake/static GPS coordinates for testing and GPS export validation.
12. User requested watchlists with named entries and row-level color highlighting.
13. User requested per-column filtering with partial matching.
14. User requested current associated SSID in the Clients tab, with historical associations as optional fields.
15. User requested per-table pagination with a default visible list size of 50.
16. User requested resizable panes and scrollable detail sections.
17. User requested that selected devices stay logically selected even when table rows reorder.
18. User requested Channel Usage to become a panel that can be toggled from the Access Points tab.

## Bluetooth scope changes

1. Bluetooth support expanded from passive discovery to include:
   - controller selection
   - Ubertooth support
   - selected-device connect/enumerate workflow
   - human-readable enumeration output
2. User requested support for identifying whether Bluetooth devices are related/tethered where possible.
3. User requested multiple adapters support broadly across the project.

## Packet-inspection scope reversal

The user temporarily requested client-IP, domain, reverse-DNS, GeoIP, user-agent, and protocol analytics. Later, the user explicitly reversed this and required that anything requiring packet inspection be removed.

Final direction:

1. No domain/history/GeoIP/user-agent analysis should remain in scope.
2. No other IP-content analysis should remain in scope.
3. Unencrypted RF metadata remains in scope.

## SDR scope expansion

The SDR scope expanded far beyond the initial Wi-Fi/Bluetooth app.

### Base SDR requirements

1. SDR tab with:
   - center frequency
   - spectrogram
   - FFT
   - decode output table
2. Right-click decode actions for selected frequencies.
3. Sample recording and replay-oriented storage for passive analysis.
4. Presets/bookmarks for favorite frequencies.
5. Scan-range and scan-speed controls.
6. Squelch control tied to SDR view.
7. Gqrx-style controls and demodulator support.
8. Wideband zoom in/out.
9. Center-frequency geiger counter based on signal strength.
10. Map plotting for decoders that yield coordinates.
11. Save decoded text and map data.

### Decoder priority requested

1. `rtl_433`
2. `ADS-B`
3. `ACARS`
4. `AIS`
5. `POCSAG`
6. then Iridium, DECT, GSM/LTE, and broader satcom/digital voice/video decoders

### SDR hardware requested

1. RTL-SDR
2. HackRF
3. bladeRF
4. Ettus B210

### Additional SDR feature requests

1. PortaPack HAVOC-style capabilities where feasible:
   - wideband spectrum analysis
   - waterfall
   - AM/FM/NFM receivers
   - OOK tools
   - IQ recording
   - RSSI audio direction finding
2. Many protocol decoders requested, including:
   - P25
   - radiosonde
   - APRS / AX.25
   - weather satellites
   - AM/FM/SSB
   - DAB
   - digital voice modes
   - DVB-S/DVB-S2 and analog video references
   - VOR
   - LoRa
   - M17
   - FLEX
   - time signals
   - Inmarsat
   - TETRA
   - Iridium
   - STD-C
   - drone-related decoders
3. Meshtastic and Meshcore metadata/location support requested.
4. GSMTAP output requested.
5. Multiple SDRs simultaneously requested later.

## Cellular/satcom requests and policy boundary

The user requested verification and decoding around GSM/LTE/CDMA and satellite signals. These requests crossed into sensitive territory in several places.

Final boundary to retain:

1. Metadata-only/control-plane-safe passive workflows may be evaluated where policy permits.
2. IMSI-catcher behavior, subscriber extraction, SMS capture, user-plane interception, and payload recovery from third-party communications were declined and must remain declined.

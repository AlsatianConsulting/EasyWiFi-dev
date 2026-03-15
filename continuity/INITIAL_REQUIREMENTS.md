# Initial Requirements

The original request was for a Linux application similar to Sparrow Wi-Fi, but with a more refined interface and a passive-first workflow.

## Core Wi-Fi requirements

1. Passive monitor-mode Wi-Fi support across all supported card spectrums.
2. Access Points tab with these visible fields:
   - SSID
   - BSSID
   - OUI Manufacturer
   - Channel
   - Encryption Type
   - Number of Clients
   - First Seen
   - Last Seen
   - Handshake Count
3. Access Point details pane with:
   - displayed fields plus full detail
   - frequency
   - full encryption type
   - notes
   - OUI make
   - uptime from beacons
   - WPS information
   - packet-type pie chart
   - 802.11d country information
4. Associated clients pane for the selected AP with:
   - MAC
   - OUI
   - data transferred
   - RSSI
5. Right-click AP actions:
   - View Details
   - Locate Device
   - Lock to Channel
6. Geiger-counter style locate window:
   - lock adapter to AP channel
   - measure RSSI
   - audible tone rises with stronger signal
7. Clients tab with:
   - MAC
   - OUI
   - associated AP
   - RSSI
   - probes
   - first heard / last heard
8. Client details with:
   - seen APs
   - full collected detail
   - WPS info where available
   - handshake-related observations
   - probe detail
9. Channel Usage tab:
   - chart showing channel utilization
   - filterable by spectrum/band
10. GPS support:
   - Interface
   - GPSD
   - stream (NMEA over TCP/UDP)
   - static location
11. Exports originally requested:
   - CSV
   - JSON
   - PDF
   - PCAP/PCAPNG with GPS
   - KML summary and per-device observation exports
12. Storage originally allowed:
   - SQLite if beneficial

## UI and settings requirements

1. Interfaces under settings.
2. Channel hopping modes:
   - specified frequencies/channels
   - one band only
   - lock channel
3. Support all frequencies the selected Wi-Fi card supports.
4. Output directory chosen by user when the app runs.
5. Layout customization:
   - choose columns
   - remove columns
   - reorder columns
   - resize columns
6. Watchlist and alerting:
   - handshake highlighting
   - device/network watchlists
   - row highlighting
7. Right-click device locate actions for APs and clients.

## Bluetooth requirements added early

1. Separate Bluetooth tab.
2. Fields:
   - BT/BLE
   - MAC
   - OUI
   - identified type
   - first seen
   - last seen
3. Bluetooth detail pane:
   - geiger locate panel
   - MFGR IDs
   - UUIDs
   - device type
4. Offline manufacturer/UUID resolving preferred.

## Passive-only requirement

1. The user explicitly required the project to remain passive.

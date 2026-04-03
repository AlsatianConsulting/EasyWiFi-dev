# EasyWiFi UI Design Specification
## Complete Guide for GTK4/Rust Implementation

---

## 1. COLOR PALETTE

All colors are defined in HSL. The UI is dark-mode only.

### Core Colors

| Token               | HSL                  | Hex (approx)  | Usage                                      |
|---------------------|----------------------|---------------|---------------------------------------------|
| background          | 240, 10%, 7%        | #101018       | App window background                       |
| foreground          | 210, 20%, 92%        | #E3E7ED       | Primary text                                |
| card                | 240, 8%, 10%         | #18181F       | Header bar, footer, card backgrounds        |
| primary             | 27, 76%, 53%         | #E87722       | Brand orange — active tabs, accents, needle  |
| primary-foreground  | 0, 0%, 100%          | #FFFFFF       | Text on primary-colored backgrounds          |
| secondary           | 240, 6%, 14%         | #212126       | Subtle panels, hover backgrounds, badges     |
| secondary-foreground| 210, 20%, 92%        | #E3E7ED       | Text on secondary backgrounds               |
| muted               | 240, 6%, 14%         | #212126       | Disabled/inactive areas                     |
| muted-foreground    | 215, 15%, 55%        | #7A8699       | Secondary text, labels, column headers       |
| destructive         | 0, 72%, 51%          | #DC2626       | Stop scan button                            |
| success             | 142, 71%, 45%        | #22C55E       | Meter green zone, packet mix "Data"          |
| warning             | 38, 92%, 50%         | #F59E0B       | Meter yellow zone, packet mix "Control"      |
| border              | 240, 6%, 18%         | #2B2B33       | All borders — panels, table rows, inputs     |
| ring                | 27, 76%, 53%         | #E87722       | Focus ring on inputs                        |

### Special-Use Colors (ONLY in signal meter & pie chart)

| Context              | Color                      | Where                         |
|----------------------|----------------------------|-------------------------------|
| Meter red zone       | hsl(0, 72%, 51%)           | Left side of RSSI arc         |
| Meter yellow zone    | hsl(38, 92%, 50%)          | Middle of RSSI arc            |
| Meter green zone     | hsl(142, 71%, 45%)         | Right side of RSSI arc        |
| Pie: Management      | hsl(27, 76%, 53%)          | Orange (primary)              |
| Pie: Control         | hsl(38, 92%, 50%)          | Yellow (warning)              |
| Pie: Data            | hsl(142, 71%, 45%)         | Green (success)               |
| Pie: Other           | hsl(215, 15%, 55%)         | Gray (muted-foreground)       |

**IMPORTANT**: No other colors are used anywhere in the UI. All data values (RSSI, encryption, WPS, handshakes) are displayed in plain foreground or muted-foreground text — NO conditional coloring.

---

## 2. TYPOGRAPHY

| Element            | Font Family                      | Weight | Size    |
|--------------------|----------------------------------|--------|---------|
| Body text          | Ubuntu, system-ui, sans-serif    | 400    | varies  |
| Bold labels        | Ubuntu                           | 700    | varies  |
| Section headers    | Ubuntu                           | 600    | 10px    |
| All data values    | Ubuntu                           | 400-500| varies  |
| MAC/BSSID values   | Ubuntu (NOT monospace)           | 400    | 10-12px |
| Tab labels         | Ubuntu                           | 500    | 12px    |
| Table cells        | Ubuntu                           | 400    | 12px    |
| Detail labels      | Ubuntu                           | 400    | 9px     |
| Detail values      | Ubuntu                           | 500    | 12px    |
| Footer status      | Ubuntu                           | 400    | 10px    |

**NOTE**: No monospace fonts are used anywhere in the UI. Everything uses Ubuntu.

---

## 3. LAYOUT STRUCTURE

```
┌─────────────────────────────────────────────────────────────┐
│  HEADER BAR (bg: card, border-bottom: border)               │
│  [Logo] EasyWiFi  "Command Center"  │ [AP] [Clients] [BT] │ APs: N  Clients: N  [⚙] [Scan] │
├───────────────────────────────────────┬─────────────────────┤
│                                       │                     │
│  MAIN TABLE PANEL (65% width)        │ DETAIL PANEL (35%)  │
│  - Sub-header with title, filter,    │ - Identity card     │
│    column picker                     │ - RSSI Meter        │
│  - Sortable table                    │ - Data grid         │
│                                       │ - Sections          │
│  (Resizable split via drag handle)   │ - Pie chart         │
│                                       │                     │
├───────────────────────────────────────┴─────────────────────┤
│  FOOTER STATUS BAR (optional, bg: card) — Interface, Mode, Uptime, Scan status │
└─────────────────────────────────────────────────────────────┘
```

### Dimensions & Spacing
- Header height: ~40px, padding: 8px 16px
- Footer height: ~24px, padding: 4px 16px
- Table row height: ~32px, cell padding: 8px 12px
- Detail panel card padding: 12px
- Detail grid gap: 8px
- Border radius: 8px (lg), 6px (md), 4px (sm)
- Resizable panel split: 65/35 default, min 40% main / min 25% detail

---

## 4. HEADER BAR

**Background**: card color  
**Border**: 1px solid border on bottom

### Left Section
- Radio icon (20px, primary color) + "EasyWiFi" (18px, bold, primary color) + "Command Center" (12px, muted-foreground)

### Center Section — Tab Navigation
Three tab buttons in a row:
- **Access Points** (WiFi icon)
- **Clients & Probes** (Activity icon)
- **Bluetooth** (Bluetooth icon)

Active tab: `bg-primary text-white rounded-md px-12px py-6px`  
Inactive tab: `text-muted-foreground hover:bg-secondary hover:text-foreground`

### Right Section
- Counters: "APs: {N}" and "Clients: {N}" in muted-foreground, values in foreground
- Preferences button: Cog/Settings icon (16px), muted-foreground, hover:bg-secondary
- Scan toggle button:
  - Scanning: `bg-destructive text-white` → "Stop Scan" with pulsing white dot
  - Stopped: `bg-primary text-white` → "Start Scan" with dim dot

---

## 5. TABLE VIEWS

### 5a. Access Points Table

**Sub-header bar** (border-bottom: border):
- Left: "DISCOVERED ACCESS POINTS" (uppercase, tracking-wider, 12px, muted-foreground)
- Right: Filter input (160px wide, 24px tall, bg-secondary, border-border) + Column picker icon + "{N} found"

**Column picker**: Popover with checkboxes for each column. Columns saved to localStorage.

**Columns** (all sortable with tri-state: none → asc → desc → none):

| Column      | Key          | Alignment | Content                          |
|-------------|--------------|-----------|----------------------------------|
| SSID        | ssid         | left      | Bold foreground; "Hidden" italic muted if null |
| BSSID       | bssid        | left      | muted-foreground                 |
| OUI         | oui          | left      | muted-foreground, truncate 120px |
| CH          | channel      | center    | plain                            |
| Encryption  | encryption   | center    | plain text, NO color, NO icon    |
| RSSI        | rssi         | center    | plain number, NO color           |
| WPS         | wps          | center    | "Yes" or "—", NO color           |
| Clients     | clients      | center    | plain number                     |
| First Seen  | firstSeen    | center    | muted-foreground                 |
| Last Seen   | lastSeen     | center    | muted-foreground                 |
| Handshakes  | handshakes   | center    | plain number, NO color           |

**Sort indicators**: ArrowUpDown (opacity 30%) when unsorted, ArrowUp for asc, ArrowDown for desc.

**Selected row**: `bg-primary/10` (10% opacity primary) + 2px left border in primary color  
**Hover row**: `bg-secondary/50`  
**Row border**: `border-bottom: 1px solid border at 50% opacity`

### 5b. Clients & Probes Table

**Additional features**:
- Filter toggles: All | Associated | Probing | Unassociated — each showing count
  - Active toggle: `bg-primary text-white`
  - Inactive: `text-muted-foreground hover:bg-secondary`
- AP filter badge: When navigated from AP detail, shows "AP: {bssid}" with × dismiss button

**Columns**:

| Column        | Key           | Alignment | Content                          |
|---------------|---------------|-----------|----------------------------------|
| MAC           | mac           | left      | muted-foreground                 |
| OUI           | oui           | left      | muted-foreground, truncate 100px |
| Associated AP | associatedAp  | left      | 10px size; "—" if null           |
| RSSI          | rssi          | center    | plain number                     |
| WPS           | wps           | center    | "Yes" or "—"                     |
| Probes        | probes        | left      | 10px, muted, truncate 150px      |
| First Seen    | firstSeen     | center    | muted-foreground                 |
| Last Seen     | lastSeen      | center    | muted-foreground                 |
| Data          | data          | center    | formatted bytes (e.g. "14.5 MB") |

### 5c. Bluetooth Table

**Columns**:

| Column      | Key        | Alignment | Content                            |
|-------------|------------|-----------|-------------------------------------|
| Name        | name       | left      | Bluetooth icon (primary) + name; "Unknown" italic if null |
| MAC         | mac        | left      | muted-foreground                   |
| OUI         | oui        | left      | muted-foreground, truncate 100px   |
| RSSI        | rssi       | center    | plain number                       |
| Mfgr IDs    | mfgrIds    | left      | 10px, muted, truncate 100px        |
| First Seen  | firstSeen  | center    | muted-foreground                   |
| Last Seen   | lastSeen   | center    | muted-foreground                   |
| Mfgr Names  | mfgrNames  | left      | 10px, muted, truncate 120px        |
| UUIDs       | uuids      | left      | 10px, muted, truncate 140px        |

---

## 6. DETAIL PANELS

All detail panels share the same visual language. Background: `bg-secondary/30` with `border-border` rounded-lg cards.

### 6a. AP Detail Panel

**Identity card**: SSID (14px bold) + BSSID (10px muted) + OUI manufacturer (10px muted)

**RSSI Meter** (see Section 7)

**Data grid** (2 columns):
Each cell is a rounded card with border:
- Label: 9px uppercase tracking-wider muted-foreground
- Value: 12px font-medium foreground

Fields: Channel, Frequency (MHz), Band, Encryption (short), Full Encryption, First Seen, Last Seen, Handshakes, Beacons, Country, Total Packets, Source Adapters

**Clients count**: Clickable card → navigates to Clients tab filtered by this AP's BSSID. Shows "(click to view)" hint.

**WPS section** (if present): 2-column grid with Version, State, Config Methods, Manufacturer, Model, Model Number, Serial Number

**Packet Mix**: Donut/pie chart (see Section 8)

**Notes** (if present): Simple text block

### 6b. Client Detail Panel

**Identity card**: OUI manufacturer (14px bold) + MAC (10px muted)

**RSSI Meter**

**Data grid**: Data Transferred, First/Last Seen, Source Adapters, Seen APs count, Handshake Networks count, Band, Uplink, Downlink, Retry Frames, Power Save, EAPOL Frames, PMKID Count

**Associated AP**: Single card showing BSSID

**Probed SSIDs**: Tags/badges in `bg-secondary rounded px-8px py-2px text-10px`

**Seen Access Points**: Same tag style

**Handshake Networks**: Same tag style

**Network Intel**: Key-value rows for IPv4, IPv6, Hostnames, DNS

**WPS section** (if present)

**Packet Mix**: Same donut chart

### 6c. Bluetooth Detail Panel

**Identity card**: Bluetooth icon (primary) + name (14px bold) + MAC (10px muted) + OUI (10px muted)

**RSSI Meter**

**Data grid**: Transport, Address Type, Device Type, Class of Device, Alias, First/Last Seen, Source Adapters

**Manufacturer IDs**: Tags + manufacturer names below

**Services / UUIDs**: Tags for UUID names + raw UUIDs listed below in 9px muted

**Active Enumeration** (if present): 2-column grid — Connected, Paired, Trusted, Blocked, Services Resolved, TX Power, Battery %, Appearance, Icon, Modalias + GATT Services tags

---

## 7. RSSI SIGNAL STRENGTH METER

An SVG-based analog gauge/meter:

- **SVG viewBox**: 300 × 170
- **Arc**: Semicircular from 30,130 to 270,130 with 120px radius
  - Track: 16px stroke in border color
  - Colored arc: 14px stroke with linear gradient red→yellow→green (opacity 0.8)
- **Tick marks**: 8 ticks from -100 to -30 dBm at 10 dBm intervals
  - Tick lines: foreground at 40% opacity, 1.5px
  - Labels: 8px muted-foreground
- **Needle**: 2.5px line from center (150,130) to top (150,35)
  - Color: primary (orange)
  - Animated rotation with cubic-bezier(0.34, 1.56, 0.64, 1) over 0.6s
  - Rotation maps: -100dBm → -135°, -30dBm → +135°
- **Pivot**: 6px circle (primary) with 3px inner circle (background)
- **Label**: Below center — "Excellent" / "Good" / "Fair" / "Weak" / "Very Weak"
  - ≥ -40: Excellent, ≥ -55: Good, ≥ -70: Fair, ≥ -85: Weak, else: Very Weak
- **Header**: "SIGNAL STRENGTH METER" (10px uppercase muted) + "{rssi} dBm" (14px bold foreground)

---

## 8. PACKET MIX PIE CHART

A donut/ring chart:
- Inner radius: 30px (AP detail) / 25px (client detail)
- Outer radius: 50px (AP) / 42px (client)
- Padding angle between segments: 3°
- No stroke on segments
- Tooltip: bg-card, border-border, 6px radius, 11px text
- Legend below: colored dots (8px) + category name (10px muted)

---

## 9. PREFERENCES DIALOG

Modal dialog with bg-card, max-width 480px, max-height 80vh, scrollable.

Title: "Preferences" in primary color.

### Sections (separated by horizontal rules):

**View**: Show Status Bar, Show Detail Pane, Show Device Pane, Show Column Filters, Inline Channel Usage, Dark Mode, Use Zulu Time (UTC), Rows Per Page (dropdown: 25/50/100/200)

**WiFi Capture**: Packet Header Mode (Radiotap/PPI dropdown), Enable WiFi Frame Parsing

**Bluetooth**: Bluetooth Scanning, Scan Source (BlueZ/Ubertooth/Both), Scan Timeout (number input), Scan Pause (number input)

**GPS**: GPS Provider (Disabled/GPSD/Serial/Static)

**Output & Storage**: Output to Files, Output Directory (text input), Store SQLite, Auto-create Exports on Startup

**Data Sources**: OUI Source Path (text input), Auto-check OUI Updates, GeoIP City DB Path (text input)

**Alerts**: Handshake Alerts, Watchlist Alerts

Each setting row: Label left (12px), control right (Switch toggles or Select dropdowns or Input fields).
Section titles: 12px uppercase primary color.

---

## 10. FOOTER STATUS BAR (Optional)

Thin bar at bottom (24px):
- bg: card, border-top: border
- Left: "Interface: wlan0mon" + "Mode: Monitor"
- Right: "Uptime: HH:MM:SS" + scanning indicator (pulsing primary dot + "Scanning" or dim dot + "Idle")
- All text: 10px, labels in muted-foreground, values in foreground

---

## 11. DATA MODELS

### AccessPointRecord
```
bssid: String (MAC format)
ssid: Option<String>
oui_manufacturer: Option<String>
source_adapters: Vec<String>
country_code_80211d: Option<String>
channel: Option<u32>
frequency_mhz: Option<u32>
band: Enum("2.4 GHz", "5 GHz", "6 GHz", "Unknown")
encryption_short: String (e.g. "WPA3", "WPA2", "OPEN")
encryption_full: String (e.g. "WPA3-SAE CCMP")
rssi_dbm: Option<i32>
number_of_clients: u32
first_seen: String (time format)
last_seen: String (time format)
handshake_count: u32
notes: Option<String>
uptime_beacons: Option<u64>
wps: Option<WpsInfo>
packet_mix: PacketTypeBreakdown
observations: Vec<GeoObservation>
```

### ClientRecord
```
mac: String
oui_manufacturer: Option<String>
source_adapters: Vec<String>
associated_ap: Option<String> (BSSID)
data_transferred_bytes: u64
rssi_dbm: Option<i32>
probes: Vec<String>
first_seen: String
last_seen: String
seen_access_points: Vec<String>
wps: Option<WpsInfo>
handshake_networks: Vec<String>
network_intel: ClientNetworkIntel
observations: Vec<GeoObservation>
```

### ClientNetworkIntel
```
local_ipv4_addresses: Vec<String>
local_ipv6_addresses: Vec<String>
dhcp_hostnames: Vec<String>
dns_names: Vec<String>
packet_mix: PacketTypeBreakdown
uplink_bytes: u64
downlink_bytes: u64
retry_frame_count: u32
power_save_observed: bool
eapol_frame_count: u32
pmkid_count: u32
band: Enum("2.4 GHz", "5 GHz", "6 GHz", "Unknown")
```

### BluetoothDeviceRecord
```
mac: String
address_type: Option<String>
transport: String ("le", "bredr", "dual")
oui_manufacturer: Option<String>
source_adapters: Vec<String>
advertised_name: Option<String>
alias: Option<String>
device_type: Option<String>
class_of_device: Option<String>
rssi_dbm: Option<i32>
first_seen: String
last_seen: String
mfgr_ids: Vec<String>
mfgr_names: Vec<String>
uuids: Vec<String>
uuid_names: Vec<String>
active_enumeration: Option<BluetoothActiveEnumeration>
observations: Vec<GeoObservation>
```

### BluetoothActiveEnumeration
```
connected: bool
paired: bool
trusted: bool
blocked: bool
services_resolved: bool
tx_power_dbm: Option<i32>
battery_percent: Option<u8>
appearance_name: Option<String>
icon: Option<String>
modalias: Option<String>
services: Vec<{ uuid: String, name: Option<String>, primary: bool }>
```

### WpsInfo
```
version: Option<String>
state: Option<String>
config_methods: Option<String>
manufacturer: Option<String>
model_name: Option<String>
model_number: Option<String>
serial_number: Option<String>
```

### PacketTypeBreakdown
```
management: u64
control: u64
data: u64
other: u64
```

### AppSettings
```
show_status_bar: bool
show_detail_pane: bool
show_device_pane: bool
show_column_filters: bool
show_ap_inline_channel_usage: bool
dark_mode: bool
default_rows_per_page: u32 (25, 50, 100, 200)
oui_source_path: String
wifi_packet_header_mode: Enum("radiotap", "ppi")
enable_wifi_frame_parsing: bool
output_to_files: bool
output_root: String
geoip_city_db_path: String
bluetooth_enabled: bool
bluetooth_scan_source: Enum("bluez", "ubertooth", "both")
bluetooth_controller: Option<String>
ubertooth_device: Option<String>
bluetooth_scan_timeout_secs: u32
bluetooth_scan_pause_ms: u32
gps: Enum("disabled", "gpsd", "serial", "static")
enable_handshake_alerts: bool
enable_watchlist_alerts: bool
store_sqlite: bool
auto_create_exports_on_startup: bool
auto_check_oui_updates: bool
use_zulu_time: bool
```

---

## 12. INTERACTION BEHAVIORS

1. **Table sorting**: Click any column header to cycle through none → ascending → descending → none
2. **Column visibility**: Click column picker icon → popover with checkboxes. At least 1 column must remain visible. Persists to storage.
3. **Row selection**: Click a table row → highlight it + show details in right panel
4. **AP → Clients navigation**: In AP detail panel, click the "Clients" count → switches to Clients tab with BSSID filter applied. Filter shows as dismissible badge.
5. **Client type filters**: Toggle between All/Associated/Probing/Unassociated
6. **Text filter**: Real-time filtering across SSID, BSSID, OUI, encryption (AP) or MAC, OUI, AP, probes (clients) or name, MAC, OUI (bluetooth)
7. **Resizable panels**: Horizontal drag handle between main table and detail panel
8. **Preferences**: Opens modal dialog, changes apply immediately

---

## 13. GTK4 CSS MAPPING GUIDE

For GTK4 implementation, map these design tokens to CSS custom properties:

```css
@define-color bg_color rgb(16, 16, 24);           /* background */
@define-color fg_color rgb(227, 231, 237);         /* foreground */
@define-color card_color rgb(24, 24, 31);          /* card */
@define-color primary_color rgb(232, 119, 34);     /* #E87722 */
@define-color primary_fg rgb(255, 255, 255);       /* white */
@define-color secondary_color rgb(33, 33, 38);     /* secondary */
@define-color muted_fg rgb(122, 134, 153);         /* muted-foreground */
@define-color border_color rgb(43, 43, 51);        /* border */
@define-color destructive_color rgb(220, 38, 38);  /* destructive */
@define-color success_color rgb(34, 197, 94);      /* success */
@define-color warning_color rgb(245, 158, 11);     /* warning */

window {
    background-color: @bg_color;
    color: @fg_color;
    font-family: "Ubuntu", sans-serif;
}

headerbar {
    background-color: @card_color;
    border-bottom: 1px solid @border_color;
}
```

---

*Generated from EasyWiFi Lovable prototype — April 2026*

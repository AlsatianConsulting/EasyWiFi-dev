export interface PacketTypeBreakdown {
  management: number;
  control: number;
  data: number;
  other: number;
}

export interface WpsInfo {
  version: string | null;
  state: string | null;
  configMethods: string | null;
  manufacturer: string | null;
  modelName: string | null;
  modelNumber: string | null;
  serialNumber: string | null;
}

export interface GeoObservation {
  timestamp: string;
  latitude: number;
  longitude: number;
  altitudeM: number | null;
  rssiDbm: number | null;
}

export interface AccessPointRecord {
  bssid: string;
  ssid: string | null;
  ouiManufacturer: string | null;
  sourceAdapters: string[];
  countryCode80211d: string | null;
  channel: number | null;
  frequencyMhz: number | null;
  band: "2.4 GHz" | "5 GHz" | "6 GHz" | "Unknown";
  encryptionShort: string;
  encryptionFull: string;
  rssiDbm: number | null;
  numberOfClients: number;
  firstSeen: string;
  lastSeen: string;
  handshakeCount: number;
  notes: string | null;
  uptimeBeacons: number | null;
  wps: WpsInfo | null;
  packetMix: PacketTypeBreakdown;
  observations: GeoObservation[];
}

export interface ClientNetworkIntel {
  localIpv4Addresses: string[];
  localIpv6Addresses: string[];
  dhcpHostnames: string[];
  dnsNames: string[];
  packetMix: PacketTypeBreakdown;
  uplinkBytes: number;
  downlinkBytes: number;
  retryFrameCount: number;
  powerSaveObserved: boolean;
  eapolFrameCount: number;
  pmkidCount: number;
  band: "2.4 GHz" | "5 GHz" | "6 GHz" | "Unknown";
}

export interface ClientRecord {
  mac: string;
  ouiManufacturer: string | null;
  sourceAdapters: string[];
  associatedAp: string | null;
  dataTransferredBytes: number;
  rssiDbm: number | null;
  probes: string[];
  firstSeen: string;
  lastSeen: string;
  seenAccessPoints: string[];
  wps: WpsInfo | null;
  handshakeNetworks: string[];
  networkIntel: ClientNetworkIntel;
  observations: GeoObservation[];
}

export interface BluetoothActiveEnumeration {
  connected: boolean;
  paired: boolean;
  trusted: boolean;
  blocked: boolean;
  servicesResolved: boolean;
  txPowerDbm: number | null;
  batteryPercent: number | null;
  appearanceName: string | null;
  icon: string | null;
  modalias: string | null;
  services: { uuid: string; name: string | null; primary: boolean }[];
}

export interface BluetoothDeviceRecord {
  mac: string;
  addressType: string | null;
  transport: string;
  ouiManufacturer: string | null;
  sourceAdapters: string[];
  advertisedName: string | null;
  alias: string | null;
  deviceType: string | null;
  classOfDevice: string | null;
  rssiDbm: number | null;
  firstSeen: string;
  lastSeen: string;
  mfgrIds: string[];
  mfgrNames: string[];
  uuids: string[];
  uuidNames: string[];
  activeEnumeration: BluetoothActiveEnumeration | null;
  observations: GeoObservation[];
}

// --- Mock Data ---

export const accessPoints: AccessPointRecord[] = [
  {
    bssid: "AA:BB:CC:11:22:33", ssid: "CorpNet-5G", ouiManufacturer: "Cisco Systems",
    sourceAdapters: ["wlan0mon"], countryCode80211d: "US", channel: 36, frequencyMhz: 5180,
    band: "5 GHz", encryptionShort: "WPA3", encryptionFull: "WPA3-SAE CCMP",
    rssiDbm: -42, numberOfClients: 14, firstSeen: "14:22:01", lastSeen: "14:35:12",
    handshakeCount: 2, notes: null, uptimeBeacons: 4521, wps: null,
    packetMix: { management: 4521, control: 1200, data: 12304, other: 45 }, observations: [],
  },
  {
    bssid: "DD:EE:FF:44:55:66", ssid: "GuestWiFi", ouiManufacturer: "Ubiquiti Inc",
    sourceAdapters: ["wlan0mon"], countryCode80211d: "US", channel: 6, frequencyMhz: 2437,
    band: "2.4 GHz", encryptionShort: "WPA2", encryptionFull: "WPA2-PSK CCMP",
    rssiDbm: -58, numberOfClients: 8, firstSeen: "14:22:01", lastSeen: "14:35:10",
    handshakeCount: 0, notes: null, uptimeBeacons: 3890, wps: { version: "2.0", state: "Configured", configMethods: "PBC", manufacturer: "Ubiquiti", modelName: "UAP-AC-Pro", modelNumber: null, serialNumber: null },
    packetMix: { management: 3890, control: 800, data: 5420, other: 12 }, observations: [],
  },
  {
    bssid: "11:22:33:AA:BB:CC", ssid: "IoT-Devices", ouiManufacturer: "TP-Link Technologies",
    sourceAdapters: ["wlan0mon"], countryCode80211d: null, channel: 1, frequencyMhz: 2412,
    band: "2.4 GHz", encryptionShort: "WPA2", encryptionFull: "WPA2-PSK TKIP+CCMP",
    rssiDbm: -65, numberOfClients: 23, firstSeen: "14:22:03", lastSeen: "14:35:11",
    handshakeCount: 0, notes: null, uptimeBeacons: 4100,  wps: null,
    packetMix: { management: 4100, control: 600, data: 8901, other: 30 }, observations: [],
  },
  {
    bssid: "44:55:66:DD:EE:FF", ssid: "NETGEAR-Home", ouiManufacturer: "Netgear Inc",
    sourceAdapters: ["wlan0mon"], countryCode80211d: null, channel: 11, frequencyMhz: 2462,
    band: "2.4 GHz", encryptionShort: "WPA", encryptionFull: "WPA-PSK TKIP",
    rssiDbm: -71, numberOfClients: 3, firstSeen: "14:22:05", lastSeen: "14:35:08",
    handshakeCount: 0, notes: null, uptimeBeacons: 3200, wps: { version: "1.0", state: "Not Configured", configMethods: "PBC Label", manufacturer: "Netgear", modelName: "R6700", modelNumber: null, serialNumber: null },
    packetMix: { management: 3200, control: 300, data: 1205, other: 5 }, observations: [],
  },
  {
    bssid: "77:88:99:AA:BB:CC", ssid: "FreeWiFi", ouiManufacturer: null,
    sourceAdapters: ["wlan0mon"], countryCode80211d: null, channel: 44, frequencyMhz: 5220,
    band: "5 GHz", encryptionShort: "OPEN", encryptionFull: "Open",
    rssiDbm: -78, numberOfClients: 1, firstSeen: "14:25:10", lastSeen: "14:34:55",
    handshakeCount: 0, notes: null, uptimeBeacons: 1800, wps: null,
    packetMix: { management: 1800, control: 100, data: 320, other: 2 }, observations: [],
  },
  {
    bssid: "CC:DD:EE:11:22:33", ssid: "Lab-6GHz", ouiManufacturer: "Intel Corporation",
    sourceAdapters: ["wlan0mon"], countryCode80211d: "US", channel: 1, frequencyMhz: 5955,
    band: "6 GHz", encryptionShort: "WPA3", encryptionFull: "WPA3-SAE CCMP-256",
    rssiDbm: -35, numberOfClients: 5, firstSeen: "14:22:00", lastSeen: "14:35:12",
    handshakeCount: 1, notes: null, uptimeBeacons: 5100, wps: null,
    packetMix: { management: 5100, control: 2000, data: 18200, other: 80 }, observations: [],
  },
  {
    bssid: "FF:11:22:33:44:55", ssid: "xfinitywifi", ouiManufacturer: "Comcast",
    sourceAdapters: ["wlan0mon"], countryCode80211d: null, channel: 149, frequencyMhz: 5745,
    band: "5 GHz", encryptionShort: "OPEN", encryptionFull: "Open",
    rssiDbm: -82, numberOfClients: 0, firstSeen: "14:28:15", lastSeen: "14:33:40",
    handshakeCount: 0, notes: null, uptimeBeacons: 900, wps: null,
    packetMix: { management: 900, control: 50, data: 45, other: 0 }, observations: [],
  },
  {
    bssid: "AA:CC:EE:22:44:66", ssid: "SecureVault", ouiManufacturer: "Aruba Networks",
    sourceAdapters: ["wlan0mon"], countryCode80211d: "US", channel: 48, frequencyMhz: 5240,
    band: "5 GHz", encryptionShort: "WPA3", encryptionFull: "WPA3-Enterprise CCMP",
    rssiDbm: -51, numberOfClients: 7, firstSeen: "14:22:01", lastSeen: "14:35:12",
    handshakeCount: 0, notes: null, uptimeBeacons: 4300, wps: null,
    packetMix: { management: 4300, control: 1500, data: 9800, other: 35 }, observations: [],
  },
];

const defaultNetIntel: ClientNetworkIntel = {
  localIpv4Addresses: [], localIpv6Addresses: [], dhcpHostnames: [], dnsNames: [],
  packetMix: { management: 0, control: 0, data: 0, other: 0 },
  uplinkBytes: 0, downlinkBytes: 0, retryFrameCount: 0, powerSaveObserved: false,
  eapolFrameCount: 0, pmkidCount: 0, band: "Unknown",
};

export const allClients: ClientRecord[] = [
  {
    mac: "A1:B2:C3:D4:E5:F6", ouiManufacturer: "Apple Inc", sourceAdapters: ["wlan0mon"],
    associatedAp: "AA:BB:CC:11:22:33", dataTransferredBytes: 15234000, rssiDbm: -38,
    probes: ["MyHomeWiFi"], firstSeen: "14:22:05", lastSeen: "14:35:12",
    seenAccessPoints: ["AA:BB:CC:11:22:33"], wps: null, handshakeNetworks: [],
    networkIntel: { ...defaultNetIntel, localIpv4Addresses: ["192.168.1.42"], dhcpHostnames: ["Johns-iPhone"], packetMix: { management: 120, control: 80, data: 12540, other: 5 }, uplinkBytes: 8200000, downlinkBytes: 7034000 },
    observations: [],
  },
  {
    mac: "F6:E5:D4:C3:B2:A1", ouiManufacturer: "Samsung Electronics", sourceAdapters: ["wlan0mon"],
    associatedAp: "AA:BB:CC:11:22:33", dataTransferredBytes: 8320000, rssiDbm: -52,
    probes: ["Starbucks WiFi", "HOME-5G"], firstSeen: "14:23:10", lastSeen: "14:35:10",
    seenAccessPoints: ["AA:BB:CC:11:22:33", "DD:EE:FF:44:55:66"], wps: null, handshakeNetworks: [],
    networkIntel: { ...defaultNetIntel, localIpv4Addresses: ["192.168.1.58"], packetMix: { management: 85, control: 60, data: 8320, other: 3 }, uplinkBytes: 4100000, downlinkBytes: 4220000 },
    observations: [],
  },
  {
    mac: "11:AA:22:BB:33:CC", ouiManufacturer: "Dell Technologies", sourceAdapters: ["wlan0mon"],
    associatedAp: "AA:BB:CC:11:22:33", dataTransferredBytes: 25600000, rssiDbm: -45,
    probes: [], firstSeen: "14:22:01", lastSeen: "14:35:11",
    seenAccessPoints: ["AA:BB:CC:11:22:33"], wps: null, handshakeNetworks: ["AA:BB:CC:11:22:33"],
    networkIntel: { ...defaultNetIntel, localIpv4Addresses: ["192.168.1.10"], dhcpHostnames: ["LAPTOP-ADMIN"], packetMix: { management: 200, control: 150, data: 15600, other: 10 }, uplinkBytes: 12800000, downlinkBytes: 12800000, eapolFrameCount: 4 },
    observations: [],
  },
  {
    mac: "DD:44:EE:55:FF:66", ouiManufacturer: "Google LLC", sourceAdapters: ["wlan0mon"],
    associatedAp: "DD:EE:FF:44:55:66", dataTransferredBytes: 3200000, rssiDbm: -61,
    probes: ["GoogleGuest"], firstSeen: "14:25:30", lastSeen: "14:35:08",
    seenAccessPoints: ["DD:EE:FF:44:55:66"], wps: null, handshakeNetworks: [],
    networkIntel: { ...defaultNetIntel, localIpv4Addresses: ["192.168.2.15"], packetMix: { management: 40, control: 30, data: 3200, other: 1 }, uplinkBytes: 1600000, downlinkBytes: 1600000 },
    observations: [],
  },
  {
    mac: "77:88:99:AA:BB:CC", ouiManufacturer: "Raspberry Pi Foundation", sourceAdapters: ["wlan0mon"],
    associatedAp: "11:22:33:AA:BB:CC", dataTransferredBytes: 890000, rssiDbm: -68,
    probes: [], firstSeen: "14:22:03", lastSeen: "14:34:55",
    seenAccessPoints: ["11:22:33:AA:BB:CC"], wps: null, handshakeNetworks: [],
    networkIntel: { ...defaultNetIntel, packetMix: { management: 15, control: 10, data: 890, other: 0 }, uplinkBytes: 445000, downlinkBytes: 445000, powerSaveObserved: true },
    observations: [],
  },
  {
    mac: "CC:DD:EE:FF:00:11", ouiManufacturer: "OnePlus Technology", sourceAdapters: ["wlan0mon"],
    associatedAp: null, dataTransferredBytes: 0, rssiDbm: -70,
    probes: ["AirportFree", "HOME-NET"], firstSeen: "14:28:15", lastSeen: "14:34:15",
    seenAccessPoints: [], wps: null, handshakeNetworks: [],
    networkIntel: { ...defaultNetIntel, packetMix: { management: 45, control: 0, data: 0, other: 0 } },
    observations: [],
  },
  {
    mac: "22:33:44:55:66:77", ouiManufacturer: "Intel Corporation", sourceAdapters: ["wlan0mon"],
    associatedAp: null, dataTransferredBytes: 0, rssiDbm: -75,
    probes: [""], firstSeen: "14:30:00", lastSeen: "14:34:30",
    seenAccessPoints: [], wps: null, handshakeNetworks: [],
    networkIntel: { ...defaultNetIntel, packetMix: { management: 12, control: 0, data: 0, other: 0 } },
    observations: [],
  },
  {
    mac: "88:99:AA:BB:CC:DD", ouiManufacturer: "Motorola Mobility", sourceAdapters: ["wlan0mon"],
    associatedAp: null, dataTransferredBytes: 0, rssiDbm: -80,
    probes: [], firstSeen: "14:31:00", lastSeen: "14:33:50",
    seenAccessPoints: [], wps: null, handshakeNetworks: [],
    networkIntel: { ...defaultNetIntel, packetMix: { management: 8, control: 0, data: 0, other: 0 } },
    observations: [],
  },
  {
    mac: "EE:FF:00:11:22:33", ouiManufacturer: "Xiaomi Communications", sourceAdapters: ["wlan0mon"],
    associatedAp: null, dataTransferredBytes: 0, rssiDbm: -66,
    probes: ["Mi-Home", "CoffeeShop"], firstSeen: "14:29:10", lastSeen: "14:34:40",
    seenAccessPoints: [], wps: null, handshakeNetworks: [],
    networkIntel: { ...defaultNetIntel, packetMix: { management: 22, control: 0, data: 0, other: 0 } },
    observations: [],
  },
];

export const clients = allClients.filter(c => c.associatedAp !== null);

export const bluetoothDevices: BluetoothDeviceRecord[] = [
  {
    mac: "A1:B2:C3:D4:E5:01", addressType: "public", transport: "le", ouiManufacturer: "Apple Inc",
    sourceAdapters: ["hci0"], advertisedName: "AirPods Pro", alias: "AirPods Pro",
    deviceType: "Audio", classOfDevice: null, rssiDbm: -45, firstSeen: "14:22:10", lastSeen: "14:35:12",
    mfgrIds: ["0x004C"], mfgrNames: ["Apple, Inc."], uuids: ["0000110B-0000-1000-8000-00805F9B34FB"],
    uuidNames: ["Audio Sink"], activeEnumeration: null, observations: [],
  },
  {
    mac: "A1:B2:C3:D4:E5:02", addressType: "public", transport: "le", ouiManufacturer: "Samsung Electronics",
    sourceAdapters: ["hci0"], advertisedName: "Galaxy Watch5", alias: "Galaxy Watch5",
    deviceType: "Wearable", classOfDevice: null, rssiDbm: -58, firstSeen: "14:23:05", lastSeen: "14:35:10",
    mfgrIds: ["0x0075"], mfgrNames: ["Samsung Electronics"], uuids: ["0000180D-0000-1000-8000-00805F9B34FB", "0000180F-0000-1000-8000-00805F9B34FB"],
    uuidNames: ["Heart Rate", "Battery Service"], activeEnumeration: null, observations: [],
  },
  {
    mac: "A1:B2:C3:D4:E5:03", addressType: "public", transport: "bredr", ouiManufacturer: "Harman Intl",
    sourceAdapters: ["hci0"], advertisedName: "JBL Flip 6", alias: "JBL Flip 6",
    deviceType: "Audio", classOfDevice: "0x240404", rssiDbm: -62, firstSeen: "14:24:30", lastSeen: "14:34:55",
    mfgrIds: ["0x0057"], mfgrNames: ["Harman International"], uuids: ["0000110A-0000-1000-8000-00805F9B34FB", "0000110E-0000-1000-8000-00805F9B34FB"],
    uuidNames: ["A2DP", "AVRCP"], activeEnumeration: null, observations: [],
  },
  {
    mac: "A1:B2:C3:D4:E5:04", addressType: "random", transport: "le", ouiManufacturer: null,
    sourceAdapters: ["hci0"], advertisedName: null, alias: null,
    deviceType: null, classOfDevice: null, rssiDbm: -78, firstSeen: "14:28:15", lastSeen: "14:33:40",
    mfgrIds: [], mfgrNames: [], uuids: [], uuidNames: [], activeEnumeration: null, observations: [],
  },
  {
    mac: "A1:B2:C3:D4:E5:05", addressType: "public", transport: "dual", ouiManufacturer: "Logitech",
    sourceAdapters: ["hci0"], advertisedName: "Logitech MX Master 3S", alias: "Logitech MX",
    deviceType: "Peripheral", classOfDevice: "0x002580", rssiDbm: -33, firstSeen: "14:22:01", lastSeen: "14:35:12",
    mfgrIds: ["0x0046"], mfgrNames: ["Logitech International"], uuids: ["00001812-0000-1000-8000-00805F9B34FB", "0000180F-0000-1000-8000-00805F9B34FB"],
    uuidNames: ["Human Interface Device", "Battery Service"],
    activeEnumeration: { connected: true, paired: true, trusted: true, blocked: false, servicesResolved: true, txPowerDbm: 4, batteryPercent: 85, appearanceName: "Mouse", icon: "input-mouse", modalias: "usb:v046DpB034d0001", services: [{ uuid: "00001812-0000-1000-8000-00805F9B34FB", name: "Human Interface Device", primary: true }] },
    observations: [],
  },
];

// --- Settings types matching AppSettings from Rust ---

export interface AppSettings {
  showStatusBar: boolean;
  showDetailPane: boolean;
  showDevicePane: boolean;
  showColumnFilters: boolean;
  showApInlineChannelUsage: boolean;
  darkMode: boolean;
  defaultRowsPerPage: number;
  ouiSourcePath: string;
  wifiPacketHeaderMode: "radiotap" | "ppi";
  enableWifiFrameParsing: boolean;
  outputToFiles: boolean;
  outputRoot: string;
  geoipCityDbPath: string;
  bluetoothEnabled: boolean;
  bluetoothScanSource: "bluez" | "ubertooth" | "both";
  bluetoothController: string | null;
  ubertoothDevice: string | null;
  bluetoothScanTimeoutSecs: number;
  bluetoothScanPauseMs: number;
  gps: "disabled" | "gpsd" | "serial" | "static";
  enableHandshakeAlerts: boolean;
  enableWatchlistAlerts: boolean;
  storeSqlite: boolean;
  autoCreateExportsOnStartup: boolean;
  autoCheckOuiUpdates: boolean;
  useZuluTime: boolean;
}

export const defaultSettings: AppSettings = {
  showStatusBar: false,
  showDetailPane: true,
  showDevicePane: true,
  showColumnFilters: true,
  showApInlineChannelUsage: false,
  darkMode: true,
  defaultRowsPerPage: 200,
  ouiSourcePath: "/usr/share/easywifi/manuf",
  wifiPacketHeaderMode: "radiotap",
  enableWifiFrameParsing: true,
  outputToFiles: false,
  outputRoot: "~/.local/share/EasyWiFi/output",
  geoipCityDbPath: "GeoLite2-City.mmdb",
  bluetoothEnabled: true,
  bluetoothScanSource: "bluez",
  bluetoothController: null,
  ubertoothDevice: null,
  bluetoothScanTimeoutSecs: 4,
  bluetoothScanPauseMs: 500,
  gps: "disabled",
  enableHandshakeAlerts: true,
  enableWatchlistAlerts: true,
  storeSqlite: true,
  autoCreateExportsOnStartup: true,
  autoCheckOuiUpdates: true,
  useZuluTime: false,
};

import {
  AccessPointRecord,
  BluetoothDeviceRecord,
  ClientRecord,
  PacketTypeBreakdown,
  WpsInfo,
} from "@/data/mockData";

export interface LiveStateResponse {
  scanning_wifi: boolean;
  scanning_bluetooth: boolean;
  access_points: unknown[];
  clients: unknown[];
  bluetooth_devices: unknown[];
  bt_enumeration_status?: Record<string, { message: string; is_error: boolean }>;
  channel_usage: unknown[];
  logs: string[];
}

const emptyPacketMix: PacketTypeBreakdown = {
  management: 0,
  control: 0,
  data: 0,
  other: 0,
};

const asString = (v: unknown, fallback = ""): string =>
  typeof v === "string" ? v : fallback;
const asNumber = (v: unknown, fallback = 0): number =>
  typeof v === "number" && Number.isFinite(v) ? v : fallback;
const asNullableNumber = (v: unknown): number | null =>
  typeof v === "number" && Number.isFinite(v) ? v : null;
const asBool = (v: unknown, fallback = false): boolean =>
  typeof v === "boolean" ? v : fallback;
const asArray = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

const toBand = (band: unknown): "2.4 GHz" | "5 GHz" | "6 GHz" | "Unknown" => {
  const b = asString(band);
  if (b === "Ghz2_4") return "2.4 GHz";
  if (b === "Ghz5") return "5 GHz";
  if (b === "Ghz6") return "6 GHz";
  return "Unknown";
};

const toDisplayTime = (value: unknown): string => {
  const ts = asString(value);
  if (!ts) return "—";
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  return d.toLocaleString();
};

const mapWps = (raw: unknown): WpsInfo | null => {
  if (!raw || typeof raw !== "object") return null;
  const w = raw as Record<string, unknown>;
  return {
    version: asString(w.version) || null,
    state: asString(w.state) || null,
    configMethods: asString(w.config_methods) || null,
    manufacturer: asString(w.manufacturer) || null,
    modelName: asString(w.model_name) || null,
    modelNumber: asString(w.model_number) || null,
    serialNumber: asString(w.serial_number) || null,
  };
};

const mapPacketMix = (raw: unknown): PacketTypeBreakdown => {
  if (!raw || typeof raw !== "object") return emptyPacketMix;
  const p = raw as Record<string, unknown>;
  return {
    management: asNumber(p.management, 0),
    control: asNumber(p.control, 0),
    data: asNumber(p.data, 0),
    other: asNumber(p.other, 0),
  };
};

export const mapAccessPoint = (raw: unknown): AccessPointRecord => {
  const r = (raw || {}) as Record<string, unknown>;
  return {
    bssid: asString(r.bssid),
    ssid: asString(r.ssid) || null,
    ouiManufacturer: asString(r.oui_manufacturer) || null,
    sourceAdapters: asArray(r.source_adapters).map((v) => asString(v)).filter(Boolean),
    countryCode80211d: asString(r.country_code_80211d) || null,
    channel: asNullableNumber(r.channel),
    frequencyMhz: asNullableNumber(r.frequency_mhz),
    channelWidthMhz: asNullableNumber(r.channel_width_mhz),
    band: toBand(r.band),
    encryptionShort: asString(r.encryption_short, "Unknown"),
    encryptionFull: asString(r.encryption_full, "Unknown"),
    rssiDbm: asNullableNumber(r.rssi_dbm),
    numberOfClients: asNumber(r.number_of_clients, 0),
    firstSeen: toDisplayTime(r.first_seen),
    lastSeen: toDisplayTime(r.last_seen),
    handshakeCount: asNumber(r.handshake_count, 0),
    notes: asString(r.notes) || null,
    uptimeBeacons: asNullableNumber(r.uptime_beacons),
    wps: mapWps(r.wps),
    packetMix: mapPacketMix(r.packet_mix),
    observations: [],
  };
};

export const mapClient = (raw: unknown): ClientRecord => {
  const r = (raw || {}) as Record<string, unknown>;
  const ni = (r.network_intel || {}) as Record<string, unknown>;
  return {
    mac: asString(r.mac),
    ouiManufacturer: asString(r.oui_manufacturer) || null,
    sourceAdapters: asArray(r.source_adapters).map((v) => asString(v)).filter(Boolean),
    associatedAp: asString(r.associated_ap) || null,
    dataTransferredBytes: asNumber(r.data_transferred_bytes, 0),
    rssiDbm: asNullableNumber(r.rssi_dbm),
    probes: asArray(r.probes).map((v) => asString(v)),
    firstSeen: toDisplayTime(r.first_seen),
    lastSeen: toDisplayTime(r.last_seen),
    seenAccessPoints: asArray(r.seen_access_points).map((v) => asString(v)),
    wps: mapWps(r.wps),
    handshakeNetworks: asArray(r.handshake_networks).map((v) => asString(v)),
    networkIntel: {
      localIpv4Addresses: asArray(ni.local_ipv4_addresses).map((v) => asString(v)),
      localIpv6Addresses: asArray(ni.local_ipv6_addresses).map((v) => asString(v)),
      dhcpHostnames: asArray(ni.dhcp_hostnames).map((v) => asString(v)),
      dnsNames: asArray(ni.dns_names).map((v) => asString(v)),
      packetMix: mapPacketMix(ni.packet_mix),
      uplinkBytes: asNumber(ni.uplink_bytes, 0),
      downlinkBytes: asNumber(ni.downlink_bytes, 0),
      retryFrameCount: asNumber(ni.retry_frame_count, 0),
      powerSaveObserved: asBool(ni.power_save_observed),
      eapolFrameCount: asNumber(ni.eapol_frame_count, 0),
      pmkidCount: asNumber(ni.pmkid_count, 0),
      band: toBand(ni.band),
    },
    observations: [],
  };
};

const mapActiveEnumeration = (raw: unknown): BluetoothDeviceRecord["activeEnumeration"] => {
  if (!raw || typeof raw !== "object") return null;
  const ae = raw as Record<string, unknown>;
  const services = asArray(ae.services).map((svc) => {
    const s = (svc || {}) as Record<string, unknown>;
    return {
      uuid: asString(s.uuid),
      name: asString(s.name) || null,
      primary: asBool(s.primary),
    };
  });
  return {
    connected: asBool(ae.connected),
    paired: asBool(ae.paired),
    trusted: asBool(ae.trusted),
    blocked: asBool(ae.blocked),
    servicesResolved: asBool(ae.services_resolved),
    txPowerDbm: asNullableNumber(ae.tx_power_dbm),
    batteryPercent: asNullableNumber(ae.battery_percent),
    appearanceName: asString(ae.appearance_name) || null,
    icon: asString(ae.icon) || null,
    modalias: asString(ae.modalias) || null,
    services,
  };
};

const normalizeBluetoothUuidNames = (namesRaw: unknown[]): string[] => {
  const names = namesRaw.map((v) => asString(v)).map((v) => v.trim()).filter(Boolean);
  let unknownCount = 0;
  const known = new Set<string>();

  for (const name of names) {
    const lowered = name.toLowerCase();
    const isUnknown =
      lowered === "unknown" ||
      lowered === "unknown uuid" ||
      lowered.startsWith("unknown uuid ");
    if (isUnknown) {
      unknownCount += 1;
      continue;
    }
    known.add(name);
  }

  const out = Array.from(known);
  if (unknownCount > 0) {
    out.push(`*Unknown* (${unknownCount})`);
  }
  return out;
};

export const mapBluetooth = (raw: unknown): BluetoothDeviceRecord => {
  const r = (raw || {}) as Record<string, unknown>;
  return {
    mac: asString(r.mac),
    addressType: asString(r.address_type) || null,
    transport: asString(r.transport, "unknown"),
    ouiManufacturer: asString(r.oui_manufacturer) || null,
    sourceAdapters: asArray(r.source_adapters).map((v) => asString(v)).filter(Boolean),
    advertisedName: asString(r.advertised_name) || null,
    alias: asString(r.alias) || null,
    deviceType: asString(r.device_type) || null,
    classOfDevice: asString(r.class_of_device) || null,
    rssiDbm: asNullableNumber(r.rssi_dbm),
    firstSeen: toDisplayTime(r.first_seen),
    lastSeen: toDisplayTime(r.last_seen),
    mfgrIds: asArray(r.mfgr_ids).map((v) => asString(v)).filter(Boolean),
    mfgrNames: asArray(r.mfgr_names).map((v) => asString(v)).filter(Boolean),
    uuids: asArray(r.uuids).map((v) => asString(v)).filter(Boolean),
    uuidNames: normalizeBluetoothUuidNames(asArray(r.uuid_names)),
    activeEnumeration: mapActiveEnumeration(r.active_enumeration),
    observations: [],
  };
};

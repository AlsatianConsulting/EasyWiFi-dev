use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SpectrumBand {
    Ghz2_4,
    Ghz5,
    Ghz6,
    Unknown,
}

impl Default for SpectrumBand {
    fn default() -> Self {
        Self::Unknown
    }
}

impl SpectrumBand {
    pub fn from_frequency_mhz(frequency_mhz: Option<u32>) -> Self {
        match frequency_mhz {
            Some(f) if (2400..=2500).contains(&f) => Self::Ghz2_4,
            Some(f) if (4900..=5925).contains(&f) => Self::Ghz5,
            Some(f) if (5926..=7125).contains(&f) => Self::Ghz6,
            _ => Self::Unknown,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Ghz2_4 => "2.4 GHz",
            Self::Ghz5 => "5 GHz",
            Self::Ghz6 => "6 GHz",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PacketTypeBreakdown {
    pub management: u64,
    pub control: u64,
    pub data: u64,
    pub other: u64,
}

impl PacketTypeBreakdown {
    pub fn total(&self) -> u64 {
        self.management + self.control + self.data + self.other
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WpsInfo {
    pub version: Option<String>,
    pub state: Option<String>,
    pub config_methods: Option<String>,
    pub manufacturer: Option<String>,
    pub model_name: Option<String>,
    pub model_number: Option<String>,
    pub serial_number: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ClientNetworkIntel {
    pub packet_mix: PacketTypeBreakdown,
    pub uplink_bytes: u64,
    pub downlink_bytes: u64,
    pub retry_frame_count: u64,
    pub power_save_observed: bool,
    pub qos_priorities: Vec<u8>,
    pub eapol_frame_count: u32,
    pub pmkid_count: u32,
    pub last_frame_type: Option<u8>,
    pub last_frame_subtype: Option<u8>,
    pub last_channel: Option<u16>,
    pub last_frequency_mhz: Option<u32>,
    pub band: SpectrumBand,
    pub last_reason_code: Option<u16>,
    pub last_status_code: Option<u16>,
    pub listen_interval: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoObservation {
    pub timestamp: DateTime<Utc>,
    pub latitude: f64,
    pub longitude: f64,
    pub altitude_m: Option<f64>,
    pub rssi_dbm: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeRecord {
    pub bssid: String,
    pub client_mac: String,
    pub timestamp: DateTime<Utc>,
    pub full_wpa2_4way: bool,
    pub pcap_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessPointRecord {
    pub bssid: String,
    pub ssid: Option<String>,
    pub oui_manufacturer: Option<String>,
    #[serde(default)]
    pub source_adapters: Vec<String>,
    pub country_code_80211d: Option<String>,
    pub channel: Option<u16>,
    pub frequency_mhz: Option<u32>,
    pub band: SpectrumBand,
    pub encryption_short: String,
    pub encryption_full: String,
    pub rssi_dbm: Option<i32>,
    pub number_of_clients: u32,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub handshake_count: u32,
    pub notes: Option<String>,
    pub uptime_beacons: Option<u64>,
    pub wps: Option<WpsInfo>,
    pub packet_mix: PacketTypeBreakdown,
    pub observations: Vec<GeoObservation>,
}

impl AccessPointRecord {
    pub fn new(bssid: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            bssid: bssid.into(),
            ssid: None,
            oui_manufacturer: None,
            source_adapters: Vec::new(),
            country_code_80211d: None,
            channel: None,
            frequency_mhz: None,
            band: SpectrumBand::Unknown,
            encryption_short: "Unknown".to_string(),
            encryption_full: "Unknown".to_string(),
            rssi_dbm: None,
            number_of_clients: 0,
            first_seen: now,
            last_seen: now,
            handshake_count: 0,
            notes: None,
            uptime_beacons: None,
            wps: None,
            packet_mix: PacketTypeBreakdown::default(),
            observations: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRecord {
    pub mac: String,
    pub oui_manufacturer: Option<String>,
    #[serde(default)]
    pub source_adapters: Vec<String>,
    pub associated_ap: Option<String>,
    pub data_transferred_bytes: u64,
    pub rssi_dbm: Option<i32>,
    pub probes: Vec<String>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub seen_access_points: Vec<String>,
    pub wps: Option<WpsInfo>,
    pub handshake_networks: Vec<String>,
    pub network_intel: ClientNetworkIntel,
    pub observations: Vec<GeoObservation>,
}

impl ClientRecord {
    pub fn new(mac: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            mac: mac.into(),
            oui_manufacturer: None,
            source_adapters: Vec::new(),
            associated_ap: None,
            data_transferred_bytes: 0,
            rssi_dbm: None,
            probes: Vec::new(),
            first_seen: now,
            last_seen: now,
            seen_access_points: Vec::new(),
            wps: None,
            handshake_networks: Vec::new(),
            network_intel: ClientNetworkIntel::default(),
            observations: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelUsagePoint {
    pub timestamp: DateTime<Utc>,
    pub channel: u16,
    pub band: SpectrumBand,
    pub utilization_percent: f32,
    pub packets: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub output_dir: String,
    pub selected_interfaces: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BluetoothDeviceRecord {
    pub mac: String,
    pub address_type: Option<String>,
    pub transport: String,
    pub oui_manufacturer: Option<String>,
    #[serde(default)]
    pub source_adapters: Vec<String>,
    pub advertised_name: Option<String>,
    pub alias: Option<String>,
    pub device_type: Option<String>,
    pub class_of_device: Option<String>,
    pub rssi_dbm: Option<i32>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub mfgr_ids: Vec<String>,
    pub mfgr_names: Vec<String>,
    pub uuids: Vec<String>,
    pub uuid_names: Vec<String>,
    pub active_enumeration: Option<BluetoothActiveEnumeration>,
    pub observations: Vec<GeoObservation>,
}

impl BluetoothDeviceRecord {
    pub fn new(mac: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            mac: mac.into(),
            address_type: None,
            transport: "Unknown".to_string(),
            oui_manufacturer: None,
            source_adapters: Vec::new(),
            advertised_name: None,
            alias: None,
            device_type: None,
            class_of_device: None,
            rssi_dbm: None,
            first_seen: now,
            last_seen: now,
            mfgr_ids: Vec::new(),
            mfgr_names: Vec::new(),
            uuids: Vec::new(),
            uuid_names: Vec::new(),
            active_enumeration: None,
            observations: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BluetoothActiveEnumeration {
    pub last_enumerated: Option<DateTime<Utc>>,
    pub connected: bool,
    pub paired: bool,
    pub trusted: bool,
    pub blocked: bool,
    pub services_resolved: bool,
    pub tx_power_dbm: Option<i32>,
    pub battery_percent: Option<u8>,
    pub appearance_code: Option<u16>,
    pub appearance_name: Option<String>,
    pub icon: Option<String>,
    pub modalias: Option<String>,
    pub services: Vec<BluetoothGattServiceRecord>,
    pub characteristics: Vec<BluetoothGattCharacteristicRecord>,
    pub descriptors: Vec<BluetoothGattDescriptorRecord>,
    pub readable_attributes: Vec<BluetoothReadableAttributeRecord>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BluetoothGattServiceRecord {
    pub path: String,
    pub uuid: String,
    pub name: Option<String>,
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BluetoothGattCharacteristicRecord {
    pub path: String,
    pub uuid: String,
    pub name: Option<String>,
    pub service_uuid: Option<String>,
    pub service_name: Option<String>,
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BluetoothGattDescriptorRecord {
    pub path: String,
    pub uuid: String,
    pub name: Option<String>,
    pub characteristic_uuid: Option<String>,
    pub characteristic_name: Option<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BluetoothReadableAttributeRecord {
    pub uuid: String,
    pub name: Option<String>,
    pub value: String,
}

#[derive(Debug, Clone, Default)]
pub struct ObservationHighlights {
    pub first: Option<GeoObservation>,
    pub last: Option<GeoObservation>,
    pub strongest: Option<GeoObservation>,
}

pub fn observation_highlights(observations: &[GeoObservation]) -> ObservationHighlights {
    if observations.is_empty() {
        return ObservationHighlights::default();
    }

    let first = observations.iter().min_by_key(|obs| obs.timestamp).cloned();
    let last = observations.iter().max_by_key(|obs| obs.timestamp).cloned();
    let strongest = observations
        .iter()
        .filter(|obs| obs.rssi_dbm.is_some())
        .max_by(|a, b| {
            a.rssi_dbm
                .unwrap_or(i32::MIN)
                .cmp(&b.rssi_dbm.unwrap_or(i32::MIN))
                .then_with(|| a.timestamp.cmp(&b.timestamp))
        })
        .cloned();

    ObservationHighlights {
        first,
        last,
        strongest,
    }
}

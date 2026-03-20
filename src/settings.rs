use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub fn default_oui_source_path() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidates = Vec::new();
    if let Some(base) = app_data_root() {
        candidates.push(base.join("manuf"));
        candidates.push(base.join("oui.csv"));
    }
    candidates.extend([
        PathBuf::from("/usr/share/wirelessexplorer/manuf"),
        PathBuf::from("/usr/share/wirelessexplorer/oui.csv"),
        PathBuf::from("/usr/share/wirelessexplorer/assets/oui.csv"),
        PathBuf::from("/usr/share/WirelessExplorer/manuf"),
        PathBuf::from("/usr/share/WirelessExplorer/oui.csv"),
        PathBuf::from("/usr/share/WirelessExplorer/assets/oui.csv"),
        PathBuf::from("/usr/share/wireshark/manuf"),
        root.join("manuf"),
        root.join("oui.csv"),
        root.join("assets").join("oui.csv"),
    ]);
    first_existing_or(candidates, app_data_root().unwrap_or(root).join("manuf"))
}

pub fn default_output_root() -> PathBuf {
    app_data_root()
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
        .join("output")
}

pub fn default_show_status_bar() -> bool {
    false
}

pub fn default_show_detail_pane() -> bool {
    true
}

pub fn default_show_device_pane() -> bool {
    true
}

pub fn default_show_column_filters() -> bool {
    true
}

pub fn default_show_ap_inline_channel_usage() -> bool {
    false
}

pub fn default_default_rows_per_page() -> usize {
    50
}

pub fn default_bluetooth_enabled() -> bool {
    true
}

pub fn default_bluetooth_scan_timeout_secs() -> u64 {
    4
}

pub fn default_bluetooth_scan_pause_ms() -> u64 {
    500
}

pub fn default_bluetooth_scan_source() -> BluetoothScanSource {
    BluetoothScanSource::Bluez
}

pub fn default_enable_handshake_alerts() -> bool {
    true
}

pub fn default_enable_watchlist_alerts() -> bool {
    true
}

pub fn default_watchlist_color_hex() -> String {
    "#2ECC71".to_string()
}

pub fn default_store_sqlite() -> bool {
    true
}

pub fn default_auto_create_exports_on_startup() -> bool {
    true
}

pub fn default_auto_check_oui_updates() -> bool {
    true
}

pub fn default_wifi_packet_header_mode() -> WifiPacketHeaderMode {
    WifiPacketHeaderMode::Radiotap
}

pub fn default_enable_wifi_frame_parsing() -> bool {
    false
}

pub fn default_sdr_satcom_parse_denylist() -> Vec<String> {
    Vec::new()
}

pub fn default_sdr_satcom_payload_capture_enabled() -> bool {
    false
}

pub fn default_use_zulu_time() -> bool {
    false
}

pub fn default_bluetooth_identity_expanded() -> bool {
    true
}

pub fn default_bluetooth_passive_expanded() -> bool {
    true
}

pub fn default_bluetooth_active_summary_expanded() -> bool {
    true
}

pub fn default_bluetooth_readable_expanded() -> bool {
    true
}

pub fn default_bluetooth_services_expanded() -> bool {
    false
}

pub fn default_bluetooth_characteristics_expanded() -> bool {
    false
}

pub fn default_bluetooth_descriptors_expanded() -> bool {
    false
}

pub fn settings_file_path() -> PathBuf {
    app_config_root()
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
        .join("wirelessexplorer-settings.json")
}

pub fn legacy_settings_file_path() -> PathBuf {
    app_config_root()
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
        .join("simplestg-settings.json")
}

fn app_data_root() -> Option<PathBuf> {
    dirs::data_local_dir().map(|path| path.join("WirelessExplorer"))
}

fn app_config_root() -> Option<PathBuf> {
    dirs::config_dir().map(|path| path.join("WirelessExplorer"))
}

fn first_existing_or(candidates: Vec<PathBuf>, fallback: PathBuf) -> PathBuf {
    candidates
        .into_iter()
        .find(|candidate| candidate.exists())
        .unwrap_or(fallback)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableColumnLayout {
    pub id: String,
    pub visible: bool,
    pub width_chars: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableLayout {
    pub columns: Vec<TableColumnLayout>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WatchlistSettings {
    #[serde(default)]
    pub entries: Vec<WatchlistEntry>,
    #[serde(default)]
    pub networks: Vec<String>,
    #[serde(default)]
    pub devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WatchlistDeviceType {
    #[default]
    Wifi,
    Bluetooth,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BluetoothScanSource {
    #[default]
    Bluez,
    Ubertooth,
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WatchlistEntry {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub device_type: WatchlistDeviceType,
    #[serde(default)]
    pub mac: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_watchlist_color_hex")]
    pub color_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SdrBookmarkSetting {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub frequency_hz: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SdrOperatorPresetSetting {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub center_freq_hz: u64,
    #[serde(default)]
    pub sample_rate_hz: u32,
    #[serde(default)]
    pub scan_enabled: bool,
    #[serde(default)]
    pub scan_start_hz: u64,
    #[serde(default)]
    pub scan_end_hz: u64,
    #[serde(default)]
    pub scan_step_hz: u64,
    #[serde(default)]
    pub scan_steps_per_sec: f64,
    #[serde(default)]
    pub squelch_dbm: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BluetoothDetailViewSettings {
    #[serde(default = "default_bluetooth_identity_expanded")]
    pub identity_expanded: bool,
    #[serde(default = "default_bluetooth_passive_expanded")]
    pub passive_expanded: bool,
    #[serde(default = "default_bluetooth_active_summary_expanded")]
    pub active_summary_expanded: bool,
    #[serde(default = "default_bluetooth_readable_expanded")]
    pub readable_expanded: bool,
    #[serde(default = "default_bluetooth_services_expanded")]
    pub services_expanded: bool,
    #[serde(default = "default_bluetooth_characteristics_expanded")]
    pub characteristics_expanded: bool,
    #[serde(default = "default_bluetooth_descriptors_expanded")]
    pub descriptors_expanded: bool,
}

impl Default for BluetoothDetailViewSettings {
    fn default() -> Self {
        Self {
            identity_expanded: default_bluetooth_identity_expanded(),
            passive_expanded: default_bluetooth_passive_expanded(),
            active_summary_expanded: default_bluetooth_active_summary_expanded(),
            readable_expanded: default_bluetooth_readable_expanded(),
            services_expanded: default_bluetooth_services_expanded(),
            characteristics_expanded: default_bluetooth_characteristics_expanded(),
            descriptors_expanded: default_bluetooth_descriptors_expanded(),
        }
    }
}

pub fn default_ap_table_layout() -> TableLayout {
    TableLayout {
        columns: vec![
            TableColumnLayout {
                id: "watchlist_entry".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "ssid".to_string(),
                visible: true,
                width_chars: 16,
            },
            TableColumnLayout {
                id: "bssid".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "oui".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "channel".to_string(),
                visible: true,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "encryption".to_string(),
                visible: true,
                width_chars: 16,
            },
            TableColumnLayout {
                id: "rssi".to_string(),
                visible: true,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "wps".to_string(),
                visible: true,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "clients".to_string(),
                visible: true,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "first_seen".to_string(),
                visible: true,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "last_seen".to_string(),
                visible: true,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "handshakes".to_string(),
                visible: true,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "band".to_string(),
                visible: false,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "frequency".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "country".to_string(),
                visible: false,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "full_encryption".to_string(),
                visible: false,
                width_chars: 28,
            },
            TableColumnLayout {
                id: "hidden_ssid".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "uptime".to_string(),
                visible: false,
                width_chars: 16,
            },
            TableColumnLayout {
                id: "observation_count".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "avg_rssi".to_string(),
                visible: false,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "min_rssi".to_string(),
                visible: false,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "max_rssi".to_string(),
                visible: false,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "packet_total".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "notes".to_string(),
                visible: false,
                width_chars: 24,
            },
            TableColumnLayout {
                id: "first_location".to_string(),
                visible: false,
                width_chars: 24,
            },
            TableColumnLayout {
                id: "last_location".to_string(),
                visible: false,
                width_chars: 24,
            },
            TableColumnLayout {
                id: "strongest_location".to_string(),
                visible: false,
                width_chars: 24,
            },
            TableColumnLayout {
                id: "band".to_string(),
                visible: false,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "channel".to_string(),
                visible: false,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "frequency".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "uplink_bytes".to_string(),
                visible: false,
                width_chars: 14,
            },
            TableColumnLayout {
                id: "downlink_bytes".to_string(),
                visible: false,
                width_chars: 14,
            },
            TableColumnLayout {
                id: "retry_count".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "retry_rate".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "power_save".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "eapol_frames".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "pmkid_count".to_string(),
                visible: false,
                width_chars: 12,
            },
        ],
    }
}

pub fn default_client_table_layout() -> TableLayout {
    TableLayout {
        columns: vec![
            TableColumnLayout {
                id: "watchlist_entry".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "mac".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "oui".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "associated_ap".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "associated_ssid".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "rssi".to_string(),
                visible: true,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "wps".to_string(),
                visible: true,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "probes".to_string(),
                visible: true,
                width_chars: 28,
            },
            TableColumnLayout {
                id: "first_heard".to_string(),
                visible: true,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "last_heard".to_string(),
                visible: true,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "data_transferred".to_string(),
                visible: false,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "probe_count".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "seen_ap_count".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "handshake_network_count".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "observation_count".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "avg_rssi".to_string(),
                visible: false,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "min_rssi".to_string(),
                visible: false,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "max_rssi".to_string(),
                visible: false,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "seen_aps".to_string(),
                visible: false,
                width_chars: 24,
            },
            TableColumnLayout {
                id: "handshake_networks".to_string(),
                visible: false,
                width_chars: 24,
            },
            TableColumnLayout {
                id: "first_location".to_string(),
                visible: false,
                width_chars: 24,
            },
            TableColumnLayout {
                id: "last_location".to_string(),
                visible: false,
                width_chars: 24,
            },
            TableColumnLayout {
                id: "strongest_location".to_string(),
                visible: false,
                width_chars: 24,
            },
        ],
    }
}

pub fn default_assoc_client_table_layout() -> TableLayout {
    TableLayout {
        columns: vec![
            TableColumnLayout {
                id: "mac".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "status".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "current_ap".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "current_ssid".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "oui".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "data_transferred".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "rssi".to_string(),
                visible: true,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "first_heard".to_string(),
                visible: true,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "last_heard".to_string(),
                visible: true,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "wps".to_string(),
                visible: false,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "probe_count".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "seen_ap_count".to_string(),
                visible: false,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "handshake_network_count".to_string(),
                visible: false,
                width_chars: 12,
            },
        ],
    }
}

pub fn default_bluetooth_table_layout() -> TableLayout {
    TableLayout {
        columns: vec![
            TableColumnLayout {
                id: "watchlist_entry".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "mac".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "oui".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "name".to_string(),
                visible: true,
                width_chars: 22,
            },
            TableColumnLayout {
                id: "rssi".to_string(),
                visible: true,
                width_chars: 10,
            },
            TableColumnLayout {
                id: "mfgr_ids".to_string(),
                visible: true,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "first_seen".to_string(),
                visible: true,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "last_seen".to_string(),
                visible: true,
                width_chars: 12,
            },
            TableColumnLayout {
                id: "transport".to_string(),
                visible: false,
                width_chars: 8,
            },
            TableColumnLayout {
                id: "alias".to_string(),
                visible: false,
                width_chars: 20,
            },
            TableColumnLayout {
                id: "advertised_name".to_string(),
                visible: false,
                width_chars: 22,
            },
            TableColumnLayout {
                id: "address_type".to_string(),
                visible: false,
                width_chars: 14,
            },
            TableColumnLayout {
                id: "type".to_string(),
                visible: false,
                width_chars: 16,
            },
            TableColumnLayout {
                id: "class_of_device".to_string(),
                visible: false,
                width_chars: 18,
            },
            TableColumnLayout {
                id: "mfgr_names".to_string(),
                visible: false,
                width_chars: 20,
            },
            TableColumnLayout {
                id: "uuids".to_string(),
                visible: false,
                width_chars: 24,
            },
        ],
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelSelectionMode {
    HopAll {
        channels: Vec<u16>,
        dwell_ms: u64,
    },
    HopBand {
        band: crate::model::SpectrumBand,
        channels: Vec<u16>,
        dwell_ms: u64,
    },
    Locked {
        channel: u16,
        ht_mode: String,
    },
}

impl Default for ChannelSelectionMode {
    fn default() -> Self {
        Self::HopAll {
            channels: vec![1, 6, 11, 36, 40, 44, 48],
            dwell_ms: 200,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceSettings {
    pub interface_name: String,
    pub monitor_interface_name: Option<String>,
    pub channel_mode: ChannelSelectionMode,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WifiPacketHeaderMode {
    #[default]
    Radiotap,
    Ppi,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StreamProtocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum GpsSettings {
    Disabled,
    Interface {
        device_path: String,
    },
    Gpsd {
        host: String,
        port: u16,
    },
    Stream {
        protocol: StreamProtocol,
        host: String,
        port: u16,
    },
    Static {
        latitude: f64,
        longitude: f64,
        altitude_m: Option<f64>,
    },
}

impl Default for GpsSettings {
    fn default() -> Self {
        Self::Disabled
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    #[serde(default = "default_show_status_bar")]
    pub show_status_bar: bool,
    #[serde(default = "default_show_detail_pane")]
    pub show_detail_pane: bool,
    #[serde(default = "default_show_device_pane")]
    pub show_device_pane: bool,
    #[serde(default = "default_show_column_filters")]
    pub show_column_filters: bool,
    #[serde(default = "default_show_ap_inline_channel_usage")]
    pub show_ap_inline_channel_usage: bool,
    #[serde(default = "default_default_rows_per_page")]
    pub default_rows_per_page: usize,
    #[serde(default = "default_oui_source_path")]
    pub oui_source_path: PathBuf,
    #[serde(default = "default_wifi_packet_header_mode")]
    pub wifi_packet_header_mode: WifiPacketHeaderMode,
    #[serde(default = "default_enable_wifi_frame_parsing")]
    pub enable_wifi_frame_parsing: bool,
    #[serde(default)]
    pub output_to_files: bool,
    #[serde(default = "default_output_root")]
    pub output_root: PathBuf,
    #[serde(default)]
    pub interfaces: Vec<InterfaceSettings>,
    #[serde(default = "default_bluetooth_enabled")]
    pub bluetooth_enabled: bool,
    #[serde(default = "default_bluetooth_scan_source")]
    pub bluetooth_scan_source: BluetoothScanSource,
    #[serde(default)]
    pub bluetooth_controller: Option<String>,
    #[serde(default)]
    pub ubertooth_device: Option<String>,
    #[serde(default = "default_bluetooth_scan_timeout_secs")]
    pub bluetooth_scan_timeout_secs: u64,
    #[serde(default = "default_bluetooth_scan_pause_ms")]
    pub bluetooth_scan_pause_ms: u64,
    #[serde(default)]
    pub gps: GpsSettings,
    #[serde(default = "default_ap_table_layout")]
    pub ap_table_layout: TableLayout,
    #[serde(default = "default_client_table_layout")]
    pub client_table_layout: TableLayout,
    #[serde(default = "default_assoc_client_table_layout")]
    pub assoc_client_table_layout: TableLayout,
    #[serde(default = "default_bluetooth_table_layout")]
    pub bluetooth_table_layout: TableLayout,
    #[serde(default)]
    pub bluetooth_detail_view: BluetoothDetailViewSettings,
    #[serde(default)]
    pub watchlists: WatchlistSettings,
    #[serde(default = "default_enable_handshake_alerts")]
    pub enable_handshake_alerts: bool,
    #[serde(default = "default_enable_watchlist_alerts")]
    pub enable_watchlist_alerts: bool,
    #[serde(default = "default_store_sqlite")]
    pub store_sqlite: bool,
    #[serde(default = "default_auto_create_exports_on_startup")]
    pub auto_create_exports_on_startup: bool,
    #[serde(default = "default_auto_check_oui_updates")]
    pub auto_check_oui_updates: bool,
    #[serde(default)]
    pub sdr_bookmarks: Vec<SdrBookmarkSetting>,
    #[serde(default)]
    pub sdr_operator_presets: Vec<SdrOperatorPresetSetting>,
    #[serde(default = "default_sdr_satcom_payload_capture_enabled")]
    pub sdr_satcom_payload_capture_enabled: bool,
    #[serde(default = "default_sdr_satcom_parse_denylist")]
    pub sdr_satcom_parse_denylist: Vec<String>,
    #[serde(default = "default_use_zulu_time")]
    pub use_zulu_time: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_status_bar: default_show_status_bar(),
            show_detail_pane: default_show_detail_pane(),
            show_device_pane: default_show_device_pane(),
            show_column_filters: default_show_column_filters(),
            show_ap_inline_channel_usage: default_show_ap_inline_channel_usage(),
            default_rows_per_page: default_default_rows_per_page(),
            oui_source_path: default_oui_source_path(),
            wifi_packet_header_mode: default_wifi_packet_header_mode(),
            enable_wifi_frame_parsing: default_enable_wifi_frame_parsing(),
            output_to_files: false,
            output_root: default_output_root(),
            interfaces: Vec::new(),
            bluetooth_enabled: default_bluetooth_enabled(),
            bluetooth_scan_source: default_bluetooth_scan_source(),
            bluetooth_controller: None,
            ubertooth_device: None,
            bluetooth_scan_timeout_secs: default_bluetooth_scan_timeout_secs(),
            bluetooth_scan_pause_ms: default_bluetooth_scan_pause_ms(),
            gps: GpsSettings::Disabled,
            ap_table_layout: default_ap_table_layout(),
            client_table_layout: default_client_table_layout(),
            assoc_client_table_layout: default_assoc_client_table_layout(),
            bluetooth_table_layout: default_bluetooth_table_layout(),
            bluetooth_detail_view: BluetoothDetailViewSettings::default(),
            watchlists: WatchlistSettings::default(),
            enable_handshake_alerts: default_enable_handshake_alerts(),
            enable_watchlist_alerts: default_enable_watchlist_alerts(),
            store_sqlite: default_store_sqlite(),
            auto_create_exports_on_startup: default_auto_create_exports_on_startup(),
            auto_check_oui_updates: default_auto_check_oui_updates(),
            sdr_bookmarks: Vec::new(),
            sdr_operator_presets: Vec::new(),
            sdr_satcom_payload_capture_enabled: default_sdr_satcom_payload_capture_enabled(),
            sdr_satcom_parse_denylist: default_sdr_satcom_parse_denylist(),
            use_zulu_time: default_use_zulu_time(),
        }
    }
}

impl AppSettings {
    pub fn load_from_disk() -> Result<Self> {
        let primary_path = settings_file_path();
        let legacy_path = legacy_settings_file_path();
        let read_path = if primary_path.exists() {
            primary_path
        } else if legacy_path.exists() {
            legacy_path
        } else {
            primary_path
        };
        let raw = fs::read_to_string(&read_path)
            .with_context(|| format!("failed to read settings file {}", read_path.display()))?;
        let settings = serde_json::from_str::<Self>(&raw)
            .with_context(|| format!("failed to parse settings file {}", read_path.display()))?;
        Ok(settings)
    }

    pub fn save_to_disk(&self) -> Result<()> {
        let path = settings_file_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create settings directory {}", parent.display())
            })?;
        }
        let mut sanitized = self.clone();
        for iface in &mut sanitized.interfaces {
            iface.monitor_interface_name = None;
        }
        let serialized = serde_json::to_string_pretty(&sanitized)
            .context("failed to serialize settings to JSON")?;
        fs::write(&path, serialized)
            .with_context(|| format!("failed to write settings file {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_loads_settings_from_project_root_file() {
        let path = settings_file_path();
        let backup_path = path.with_extension("json.test-backup");
        let original = fs::read(&path).ok();
        let legacy_path = legacy_settings_file_path();
        let legacy_backup_path = legacy_path.with_extension("json.test-backup");
        let legacy_original = fs::read(&legacy_path).ok();
        if backup_path.exists() {
            let _ = fs::remove_file(&backup_path);
        }
        if legacy_backup_path.exists() {
            let _ = fs::remove_file(&legacy_backup_path);
        }
        if original.is_some() {
            fs::rename(&path, &backup_path).expect("backup current settings file");
        }
        if legacy_original.is_some() {
            fs::rename(&legacy_path, &legacy_backup_path).expect("backup legacy settings file");
        }

        let mut settings = AppSettings::default();
        settings.show_status_bar = true;
        settings.show_column_filters = false;
        settings.show_ap_inline_channel_usage = true;
        settings.default_rows_per_page = 100;
        settings.bluetooth_enabled = false;
        settings.enable_wifi_frame_parsing = true;
        settings.use_zulu_time = true;
        settings.sdr_satcom_payload_capture_enabled = true;
        settings.sdr_satcom_parse_denylist = vec!["inmarsat".to_string(), "iridium".to_string()];
        settings.oui_source_path = PathBuf::from("/tmp/test-manuf");
        settings.bluetooth_detail_view.descriptors_expanded = true;
        settings.sdr_bookmarks = vec![SdrBookmarkSetting {
            label: "Test Bookmark".to_string(),
            frequency_hz: 915_000_000,
        }];
        settings.sdr_operator_presets = vec![SdrOperatorPresetSetting {
            label: "Airband Fast".to_string(),
            center_freq_hz: 127_500_000,
            sample_rate_hz: 2_400_000,
            scan_enabled: true,
            scan_start_hz: 118_000_000,
            scan_end_hz: 137_000_000,
            scan_step_hz: 25_000,
            scan_steps_per_sec: 8.0,
            squelch_dbm: -72.0,
        }];
        settings.save_to_disk().expect("save settings");

        let loaded = AppSettings::load_from_disk().expect("load settings");
        assert!(loaded.show_status_bar);
        assert!(!loaded.show_column_filters);
        assert!(loaded.show_ap_inline_channel_usage);
        assert_eq!(loaded.default_rows_per_page, 100);
        assert!(!loaded.bluetooth_enabled);
        assert!(loaded.enable_wifi_frame_parsing);
        assert!(loaded.use_zulu_time);
        assert!(loaded.sdr_satcom_payload_capture_enabled);
        assert_eq!(
            loaded.sdr_satcom_parse_denylist,
            vec!["inmarsat".to_string(), "iridium".to_string()]
        );
        assert_eq!(loaded.oui_source_path, PathBuf::from("/tmp/test-manuf"));
        assert!(loaded.bluetooth_detail_view.descriptors_expanded);
        assert_eq!(
            loaded.sdr_bookmarks,
            vec![SdrBookmarkSetting {
                label: "Test Bookmark".to_string(),
                frequency_hz: 915_000_000,
            }]
        );
        assert_eq!(
            loaded.sdr_operator_presets,
            vec![SdrOperatorPresetSetting {
                label: "Airband Fast".to_string(),
                center_freq_hz: 127_500_000,
                sample_rate_hz: 2_400_000,
                scan_enabled: true,
                scan_start_hz: 118_000_000,
                scan_end_hz: 137_000_000,
                scan_step_hz: 25_000,
                scan_steps_per_sec: 8.0,
                squelch_dbm: -72.0,
            }]
        );

        let _ = fs::remove_file(&path);
        if backup_path.exists() {
            fs::rename(&backup_path, &path).expect("restore original settings file");
        }
        let _ = fs::remove_file(&legacy_path);
        if legacy_backup_path.exists() {
            fs::rename(&legacy_backup_path, &legacy_path)
                .expect("restore original legacy settings file");
        }
    }
}

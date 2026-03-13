use crate::bluetooth::{self, BluetoothEvent, BluetoothRuntime, BluetoothScanConfig};
use crate::capture::{self, CaptureConfig, CaptureEvent, CaptureRuntime, GeigerUpdate};
use crate::export::ExportManager;
use crate::gps::{self, GpsProvider};
use crate::model::{
    observation_highlights, AccessPointRecord, BluetoothDeviceRecord, ChannelUsagePoint,
    ClientNetworkIntel, ClientRecord, GeoObservation, HandshakeRecord, PacketTypeBreakdown,
    SessionMetadata, SpectrumBand,
};
use crate::oui::OuiDatabase;
use crate::sdr::{
    self, SdrConfig, SdrDecodeRow, SdrDecoderKind, SdrDependencyStatus, SdrEvent, SdrHardware,
    SdrMapPoint, SdrRuntime, SdrSatcomObservation, SdrSpectrumFrame,
};
use crate::settings::{
    default_ap_table_layout, default_assoc_client_table_layout, default_bluetooth_table_layout,
    default_client_table_layout, settings_file_path, AppSettings, BluetoothScanSource,
    ChannelSelectionMode, GpsSettings, InterfaceSettings, StreamProtocol, TableColumnLayout,
    TableLayout, WatchlistDeviceType, WatchlistEntry, WatchlistSettings, WifiPacketHeaderMode,
};
use crate::storage::StorageEngine;
use anyhow::{Context, Result};
use chrono::Utc;
use crossbeam_channel::{unbounded, Receiver, Sender};
use gtk::cairo;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::{
    Application, ApplicationWindow, Box as GtkBox, Button, CheckButton, ComboBoxText, Dialog,
    DrawingArea, Entry, EventControllerKey, Expander, FileChooserAction, FileChooserDialog,
    GestureClick, Grid, Label, ListBox, ListBoxRow, Notebook, Orientation, Paned, Popover,
    ProgressBar, ResponseType, ScrolledWindow, SpinButton, Stack, StackSidebar, TextView,
    Window as GtkWindow,
};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
#[cfg(target_family = "unix")]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

pub fn run() -> Result<()> {
    if capture::running_as_root() && std::env::var_os("NO_AT_BRIDGE").is_none() {
        std::env::set_var("NO_AT_BRIDGE", "1");
    }

    let app = Application::builder()
        .application_id("com.wirelessexplorer.app")
        .build();

    app.connect_activate(|app| {
        if let Err(err) = build_ui(app) {
            eprintln!("startup failed: {err:#}");
        }
    });

    app.run();
    Ok(())
}

const DETAIL_GEIGER_TAB_INDEX: u32 = 1;
const ACCESS_POINTS_TAB_INDEX: u32 = 0;
const CLIENTS_TAB_INDEX: u32 = 1;
const BLUETOOTH_TAB_INDEX: u32 = 2;
const CHANNEL_USAGE_TAB_INDEX: u32 = 3;
const SDR_TAB_INDEX: u32 = 4;
const UI_POLL_INTERVAL_MS: u64 = 120;
const MAX_CAPTURE_EVENTS_PER_TICK: usize = 1200;
const MAX_BLUETOOTH_EVENTS_PER_TICK: usize = 200;
const MAX_SDR_EVENTS_PER_TICK: usize = 200;
const MAX_WIFI_GEIGER_UPDATES_PER_TICK: usize = 8;
const MIN_LIST_REFRESH_INTERVAL_MS: u64 = 140;
const TABLE_CHAR_WIDTH_PX: i32 = 10;
const DEFAULT_TABLE_PAGE_SIZE: usize = 50;
const TABLE_PAGE_SIZE_OPTIONS: &[usize] = &[25, 50, 100, 200];
const DEFAULT_WINDOW_WIDTH: i32 = 1500;
const DEFAULT_WINDOW_HEIGHT: i32 = 950;
const DEFAULT_CONTENT_PANE_POSITION: i32 = 760;
const DEFAULT_AP_ROOT_POSITION: i32 = 330;
const DEFAULT_AP_SUMMARY_ROW_POSITION: i32 = 760;
const DEFAULT_AP_DETAIL_SECTIONS_POSITION: i32 = 470;
const DEFAULT_AP_BOTTOM_POSITION: i32 = 820;
const DEFAULT_CLIENT_ROOT_POSITION: i32 = 380;
const DEFAULT_BLUETOOTH_BOTTOM_POSITION: i32 = 360;
const DEFAULT_BLUETOOTH_ROOT_POSITION: i32 = 360;
const DEFAULT_CHANNEL_ROOT_POSITION: i32 = 430;
const DEFAULT_SDR_ROOT_POSITION: i32 = 420;
const FAKE_GPS_LATITUDE: f64 = 35.145_395_7;
const FAKE_GPS_LONGITUDE: f64 = -79.474_718_1;

#[derive(Clone)]
enum PersistenceCommand {
    ReplaceStorage(StorageEngine),
    UpsertAccessPoint(AccessPointRecord),
    UpsertClient(ClientRecord),
    UpsertBluetoothDevice(BluetoothDeviceRecord),
    AddObservation {
        device_type: String,
        device_id: String,
        observation: GeoObservation,
    },
    AddHandshake(HandshakeRecord),
    IncrementHandshakeCount(String),
    AddChannelUsage(ChannelUsagePoint),
    AddGpsTrackPoint(GeoObservation),
    Shutdown,
}

#[derive(Debug, Clone)]
struct TableSortState {
    column_id: String,
    descending: bool,
}

impl TableSortState {
    fn new(column_id: impl Into<String>, descending: bool) -> Self {
        Self {
            column_id: column_id.into(),
            descending,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SortableTable {
    AccessPoints,
    Clients,
    AssocClients,
    Bluetooth,
}

struct AppState {
    settings: AppSettings,
    storage: StorageEngine,
    persistence_sender: Sender<PersistenceCommand>,
    exporter: ExportManager,
    oui: OuiDatabase,
    gps_provider: Arc<dyn GpsProvider>,
    access_points: Vec<AccessPointRecord>,
    clients: Vec<ClientRecord>,
    bluetooth_devices: Vec<BluetoothDeviceRecord>,
    channel_usage: Vec<ChannelUsagePoint>,
    capture_runtime: Option<CaptureRuntime>,
    capture_sender: Sender<CaptureEvent>,
    bluetooth_runtime: Option<BluetoothRuntime>,
    bluetooth_sender: Sender<BluetoothEvent>,
    sdr_runtime: Option<SdrRuntime>,
    sdr_sender: Sender<SdrEvent>,
    session_capture_path: PathBuf,
    gps_track: Vec<GeoObservation>,
    last_gps_track_point_at: Option<chrono::DateTime<Utc>>,
    status_lines: Vec<String>,
    last_observation_by_device: HashMap<String, chrono::DateTime<Utc>>,
    last_storage_persist_by_device: HashMap<String, chrono::DateTime<Utc>>,
    alerted_watch_entities: HashSet<String>,
    watchlist_css_provider: gtk::CssProvider,
    layout_dirty: bool,
    ap_sort: TableSortState,
    client_sort: TableSortState,
    assoc_sort: TableSortState,
    bluetooth_sort: TableSortState,
    pending_privilege_alert: Option<String>,
    wifi_lock_restore_mode: Option<ChannelSelectionMode>,
    wifi_locked_target: Option<String>,
    wifi_interface_restore_types: HashMap<String, String>,
    scan_start_in_progress: bool,
    scan_stop_in_progress: bool,
    pending_start_completion: Option<Receiver<StartCompletion>>,
    pending_stop_completion: Option<Receiver<StopCompletion>>,
    pending_scan_restart_message: Option<String>,
}

impl AppState {
    fn fixed_gps_observation(&self, rssi_dbm: Option<i32>) -> GeoObservation {
        GeoObservation {
            timestamp: Utc::now(),
            latitude: FAKE_GPS_LATITUDE,
            longitude: FAKE_GPS_LONGITUDE,
            altitude_m: None,
            rssi_dbm,
        }
    }

    fn gps_track_for_export(&self) -> Vec<GeoObservation> {
        if self.gps_track.is_empty() {
            vec![self.fixed_gps_observation(None)]
        } else {
            self.gps_track.clone()
        }
    }

    fn enqueue_persistence(&self, command: PersistenceCommand) {
        let _ = self.persistence_sender.send(command);
    }

    fn push_status(&mut self, message: impl Into<String>) {
        self.status_lines.push(message.into());
        if self.status_lines.len() > 12 {
            let keep_from = self.status_lines.len() - 12;
            self.status_lines = self.status_lines.split_off(keep_from);
        }
    }

    fn toggle_table_sort(&mut self, table: SortableTable, column_id: impl Into<String>) {
        let column_id = column_id.into();
        let sort_state = match table {
            SortableTable::AccessPoints => &mut self.ap_sort,
            SortableTable::Clients => &mut self.client_sort,
            SortableTable::AssocClients => &mut self.assoc_sort,
            SortableTable::Bluetooth => &mut self.bluetooth_sort,
        };

        if sort_state.column_id == column_id {
            sort_state.descending = !sort_state.descending;
        } else {
            sort_state.column_id = column_id.clone();
            sort_state.descending = default_sort_descending(table, &column_id);
        }

        self.layout_dirty = true;
    }

    fn backfill_oui_labels(&mut self) {
        for ap in &mut self.access_points {
            ap.oui_manufacturer = self.oui.lookup(&ap.bssid).map(str::to_string);
        }
        for client in &mut self.clients {
            client.oui_manufacturer = self.oui.lookup(&client.mac).map(str::to_string);
        }
        for device in &mut self.bluetooth_devices {
            device.oui_manufacturer = self.oui.lookup(&device.mac).map(str::to_string);
        }
    }

    fn reload_oui_from_settings(&mut self) -> Result<usize> {
        let db = OuiDatabase::load_with_override(Some(&self.settings.oui_source_path))
            .or_else(|_| OuiDatabase::load_default())?;
        let count = db.count();
        self.oui = db;
        self.backfill_oui_labels();
        Ok(count)
    }

    fn save_settings_to_disk(&mut self) {
        if let Err(err) = self.settings.save_to_disk() {
            self.push_status(format!("failed to save preferences: {err}"));
        }
    }

    fn status_text(&self) -> String {
        self.status_lines.join("\n")
    }

    fn gps_status_text(&self) -> String {
        let status = self.gps_provider.status();
        let state = if status.connected {
            "Connected"
        } else {
            "Disconnected"
        };
        let last_fix = status
            .last_fix_timestamp
            .map(|ts| ts.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "No fix".to_string());

        format!(
            "GPS {} | {} | Last Fix: {} | {} | Output GPS: {}, {}",
            status.mode, state, last_fix, status.detail, FAKE_GPS_LATITUDE, FAKE_GPS_LONGITUDE
        )
    }

    fn reset_output_session(
        &mut self,
        output_root: PathBuf,
        announce_selection: bool,
        remember_output_root: bool,
    ) -> Result<()> {
        std::fs::create_dir_all(&output_root).with_context(|| {
            format!(
                "failed to create selected output directory {}",
                output_root.display()
            )
        })?;

        if remember_output_root {
            self.settings.output_root = output_root.clone();
        }

        let session_id = Uuid::new_v4().to_string();
        let exporter = ExportManager::new(&output_root, &session_id)?;
        exporter.create_initial_outputs()?;

        let sqlite_path = exporter.paths.session_dir.join("wirelessexplorer.sqlite");
        let storage = StorageEngine::open(&sqlite_path)?;

        let session_meta = SessionMetadata {
            id: session_id.clone(),
            started_at: Utc::now(),
            output_dir: exporter.paths.session_dir.to_string_lossy().to_string(),
            selected_interfaces: self
                .settings
                .interfaces
                .iter()
                .map(|i| i.interface_name.clone())
                .collect(),
        };
        storage.save_session(&session_meta)?;

        self.exporter = exporter;
        self.storage = storage;
        self.enqueue_persistence(PersistenceCommand::ReplaceStorage(self.storage.clone()));
        self.session_capture_path = prepare_live_capture_path(&session_id)?;
        self.gps_track.clear();
        self.last_gps_track_point_at = None;
        self.last_observation_by_device.clear();
        self.last_storage_persist_by_device.clear();
        self.alerted_watch_entities.clear();
        self.access_points.clear();
        self.clients.clear();
        self.bluetooth_devices.clear();
        self.channel_usage.clear();
        if announce_selection {
            self.push_status(format!(
                "output folder selected: {}",
                self.exporter.paths.session_dir.display()
            ));
        }
        Ok(())
    }

    fn switch_to_internal_output_session(&mut self) -> Result<()> {
        self.reset_output_session(internal_runtime_output_root(), false, false)?;
        self.push_status("file output disabled; using internal temporary session".to_string());
        Ok(())
    }

    fn apply_capture_event(&mut self, event: CaptureEvent) -> Result<UiRefreshHint> {
        match event {
            CaptureEvent::AccessPointSeen(mut ap) => {
                if ap.oui_manufacturer.is_none() {
                    ap.oui_manufacturer = self.oui.lookup(&ap.bssid).map(str::to_string);
                }

                if let Some(obs) = self.build_geo_observation(ap.rssi_dbm) {
                    if should_record_observation(
                        &mut self.last_observation_by_device,
                        &format!("ap:{}", ap.bssid),
                        obs.timestamp,
                    ) {
                        ap.observations.push(obs.clone());
                        self.enqueue_persistence(PersistenceCommand::AddObservation {
                            device_type: "ap".to_string(),
                            device_id: ap.bssid.clone(),
                            observation: obs,
                        });
                    }
                }

                merge_ap(&mut self.access_points, ap.clone());
                refresh_ap_client_count_for_bssid(
                    &mut self.access_points,
                    &self.clients,
                    &ap.bssid,
                );
                if let Some(current) = self
                    .access_points
                    .iter()
                    .find(|current| current.bssid == ap.bssid)
                    .cloned()
                {
                    if should_persist_device_update(
                        &mut self.last_storage_persist_by_device,
                        &format!("ap:{}", current.bssid),
                        current.last_seen,
                    ) {
                        self.enqueue_persistence(PersistenceCommand::UpsertAccessPoint(current));
                    }
                }
                let watch_alert = self.maybe_alert_watchlisted_ap(&ap);
                Ok(UiRefreshHint {
                    ap_list: true,
                    client_list: false,
                    bluetooth_list: false,
                    channel_chart: false,
                    status: watch_alert,
                })
            }
            CaptureEvent::ClientSeen(mut client) => {
                if client.oui_manufacturer.is_none() {
                    client.oui_manufacturer = self.oui.lookup(&client.mac).map(str::to_string);
                }

                if let Some(obs) = self.build_geo_observation(client.rssi_dbm) {
                    if should_record_observation(
                        &mut self.last_observation_by_device,
                        &format!("client:{}", client.mac),
                        obs.timestamp,
                    ) {
                        client.observations.push(obs.clone());
                        self.enqueue_persistence(PersistenceCommand::AddObservation {
                            device_type: "client".to_string(),
                            device_id: client.mac.clone(),
                            observation: obs,
                        });
                    }
                }

                merge_client(&mut self.clients, client.clone());
                let updated_ap_bssids = refresh_ap_client_counts_for_client(
                    &mut self.access_points,
                    &self.clients,
                    &client,
                );
                for ap_bssid in updated_ap_bssids {
                    if let Some(ap) = self
                        .access_points
                        .iter()
                        .find(|current| current.bssid == ap_bssid)
                        .cloned()
                    {
                        self.enqueue_persistence(PersistenceCommand::UpsertAccessPoint(ap));
                    }
                }
                if let Some(current) = self
                    .clients
                    .iter()
                    .find(|current| current.mac == client.mac)
                    .cloned()
                {
                    if should_persist_device_update(
                        &mut self.last_storage_persist_by_device,
                        &format!("client:{}", current.mac),
                        current.last_seen,
                    ) {
                        self.enqueue_persistence(PersistenceCommand::UpsertClient(current));
                    }
                }
                let watch_alert = self.maybe_alert_watchlisted_client(&client);
                Ok(UiRefreshHint {
                    ap_list: true,
                    client_list: true,
                    bluetooth_list: false,
                    channel_chart: false,
                    status: watch_alert,
                })
            }
            CaptureEvent::Observation {
                device_type,
                device_id,
                observation,
            } => {
                self.enqueue_persistence(PersistenceCommand::AddObservation {
                    device_type,
                    device_id,
                    observation,
                });
                Ok(UiRefreshHint::none())
            }
            CaptureEvent::HandshakeSeen(handshake) => {
                self.enqueue_persistence(PersistenceCommand::AddHandshake(handshake.clone()));
                self.enqueue_persistence(PersistenceCommand::IncrementHandshakeCount(
                    handshake.bssid.clone(),
                ));
                let ap_ssid = self
                    .access_points
                    .iter()
                    .find(|ap| ap.bssid == handshake.bssid)
                    .and_then(|ap| ap.ssid.clone());
                if let Some(ap) = self
                    .access_points
                    .iter_mut()
                    .find(|ap| ap.bssid == handshake.bssid)
                {
                    ap.handshake_count += 1;
                }
                match self.exporter.export_handshake_capture(
                    &self.session_capture_path,
                    ap_ssid.as_deref(),
                    &handshake.bssid,
                    &handshake.client_mac,
                    handshake.timestamp,
                    &self.gps_track_for_export(),
                ) {
                    Ok(path) => self.push_status(format!(
                        "saved handshake capture: {}",
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.display().to_string())
                    )),
                    Err(err) => self.push_status(format!("handshake capture export failed: {err}")),
                }
                if self.settings.enable_handshake_alerts {
                    self.push_status(format!(
                        "ALERT handshake complete on {} with client {}",
                        handshake.bssid, handshake.client_mac
                    ));
                    emit_alert_tone(1400, 80);
                }
                Ok(UiRefreshHint {
                    ap_list: true,
                    client_list: false,
                    bluetooth_list: false,
                    channel_chart: false,
                    status: self.settings.enable_handshake_alerts,
                })
            }
            CaptureEvent::ChannelUsage(usage) => {
                self.enqueue_persistence(PersistenceCommand::AddChannelUsage(usage.clone()));
                self.channel_usage.push(usage);
                if self.channel_usage.len() > 800 {
                    let keep = self.channel_usage.len() - 800;
                    self.channel_usage = self.channel_usage.split_off(keep);
                }
                Ok(UiRefreshHint {
                    ap_list: false,
                    client_list: false,
                    bluetooth_list: false,
                    channel_chart: true,
                    status: false,
                })
            }
            CaptureEvent::Log(text) => {
                self.push_status(text);
                Ok(UiRefreshHint {
                    ap_list: false,
                    client_list: false,
                    bluetooth_list: false,
                    channel_chart: false,
                    status: true,
                })
            }
        }
    }

    fn apply_bluetooth_event(&mut self, event: BluetoothEvent) -> Result<UiRefreshHint> {
        match event {
            BluetoothEvent::DeviceSeen(mut device) => {
                if device.oui_manufacturer.is_none() {
                    device.oui_manufacturer = self.oui.lookup(&device.mac).map(str::to_string);
                }

                if let Some(obs) = self.build_geo_observation(device.rssi_dbm) {
                    if should_record_observation(
                        &mut self.last_observation_by_device,
                        &format!("bluetooth:{}", device.mac),
                        obs.timestamp,
                    ) {
                        device.observations.push(obs.clone());
                        self.enqueue_persistence(PersistenceCommand::AddObservation {
                            device_type: "bluetooth".to_string(),
                            device_id: device.mac.clone(),
                            observation: obs,
                        });
                    }
                }

                merge_bluetooth_device(&mut self.bluetooth_devices, device.clone());
                if let Some(current) = self
                    .bluetooth_devices
                    .iter()
                    .find(|current| current.mac == device.mac)
                    .cloned()
                {
                    if should_persist_device_update(
                        &mut self.last_storage_persist_by_device,
                        &format!("bluetooth:{}", current.mac),
                        current.last_seen,
                    ) {
                        self.enqueue_persistence(PersistenceCommand::UpsertBluetoothDevice(
                            current,
                        ));
                    }
                }

                let watch_alert = if self.settings.enable_watchlist_alerts {
                    if let Some(matched) =
                        bluetooth_watchlist_match(&device, &self.settings.watchlists)
                    {
                        let key = format!("bluetooth:{}", matched.alert_key);
                        if self.alerted_watch_entities.insert(key) {
                            self.push_status(format!(
                                "ALERT {}: bluetooth device {}",
                                matched.label, device.mac
                            ));
                            emit_alert_tone(1050, 70);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };

                Ok(UiRefreshHint {
                    ap_list: false,
                    client_list: false,
                    bluetooth_list: true,
                    channel_chart: false,
                    status: watch_alert,
                })
            }
            BluetoothEvent::Log(text) => {
                self.push_status(text);
                Ok(UiRefreshHint {
                    ap_list: false,
                    client_list: false,
                    bluetooth_list: false,
                    channel_chart: false,
                    status: true,
                })
            }
        }
    }

    fn build_geo_observation(&self, rssi_dbm: Option<i32>) -> Option<GeoObservation> {
        Some(self.fixed_gps_observation(rssi_dbm))
    }

    fn maybe_record_gps_track_point(&mut self) {
        if self.capture_runtime.is_none() && self.bluetooth_runtime.is_none() {
            return;
        }

        let now = Utc::now();
        if let Some(last) = self.last_gps_track_point_at {
            if now - last < chrono::Duration::seconds(1) {
                return;
            }
        }

        let point = GeoObservation {
            timestamp: now,
            latitude: FAKE_GPS_LATITUDE,
            longitude: FAKE_GPS_LONGITUDE,
            altitude_m: None,
            rssi_dbm: None,
        };
        self.gps_track.push(point.clone());
        self.last_gps_track_point_at = Some(now);
        self.enqueue_persistence(PersistenceCommand::AddGpsTrackPoint(point));
    }

    fn start_sdr_runtime(&mut self, config: SdrConfig) {
        if let Some(runtime) = self.sdr_runtime.take() {
            runtime.stop();
        }
        self.push_status(format!(
            "starting SDR runtime ({}) at {} Hz",
            config.hardware.label(),
            config.center_freq_hz
        ));
        self.sdr_runtime = Some(sdr::start_runtime(config, self.sdr_sender.clone()));
    }

    fn stop_sdr_runtime(&mut self) {
        if let Some(runtime) = self.sdr_runtime.take() {
            runtime.stop();
            self.push_status("SDR runtime stopped".to_string());
        }
    }

    fn start_scanning(&mut self) {
        if self.scan_start_in_progress {
            self.push_status("scan start is already in progress".to_string());
            return;
        }
        if self.scan_stop_in_progress {
            self.push_status("scan stop is still in progress".to_string());
            return;
        }

        let need_wifi = self.capture_runtime.is_none();
        let need_bluetooth = self.settings.bluetooth_enabled && self.bluetooth_runtime.is_none();
        if !need_wifi && !need_bluetooth {
            self.push_status("scanning already running".to_string());
            return;
        }

        let (tx, rx) = unbounded::<StartCompletion>();
        self.pending_start_completion = Some(rx);
        self.scan_start_in_progress = true;
        self.push_status("starting scans...".to_string());

        let interfaces = self.settings.interfaces.clone();
        let session_capture_path = self.session_capture_path.clone();
        let wifi_packet_header_mode = self.settings.wifi_packet_header_mode;
        let gps_enabled = !matches!(self.settings.gps, GpsSettings::Disabled);
        let capture_sender = self.capture_sender.clone();
        let bluetooth_sender = self.bluetooth_sender.clone();
        let bluetooth_config = BluetoothScanConfig {
            controller: self.settings.bluetooth_controller.clone(),
            source: self.settings.bluetooth_scan_source,
            ubertooth_device: self.settings.ubertooth_device.clone(),
            scan_timeout_secs: self.settings.bluetooth_scan_timeout_secs,
            pause_ms: self.settings.bluetooth_scan_pause_ms,
        };

        thread::spawn(move || {
            let mut updated_interfaces = None;
            let mut wifi_interface_restore_types = HashMap::new();
            let mut capture_runtime = None;
            let mut bluetooth_runtime = None;
            let mut status_lines = Vec::new();
            let mut privilege_alert = None;
            let mut wifi_started = false;
            let mut wifi_failed = false;

            if need_wifi {
                let wifi_result = prepare_and_start_wifi_capture(
                    interfaces,
                    session_capture_path,
                    wifi_packet_header_mode,
                    gps_enabled,
                    capture_sender,
                );
                updated_interfaces = Some(wifi_result.interfaces);
                wifi_interface_restore_types = wifi_result.wifi_interface_restore_types;
                capture_runtime = wifi_result.runtime;
                status_lines.extend(wifi_result.status_lines);
                privilege_alert = wifi_result.privilege_alert;
                wifi_started = wifi_result.started;
                wifi_failed = wifi_result.failed;
            }

            let bluetooth_started = if need_bluetooth {
                bluetooth_runtime = Some(bluetooth::start_scan(bluetooth_config, bluetooth_sender));
                true
            } else {
                false
            };

            let _ = tx.send(StartCompletion {
                updated_interfaces,
                wifi_interface_restore_types,
                capture_runtime,
                bluetooth_runtime,
                status_lines,
                privilege_alert,
                wifi_started,
                wifi_failed,
                bluetooth_started,
            });
        });
    }

    fn stop_scanning(&mut self) {
        let _ = self.begin_async_scan_shutdown(None);
    }

    fn restart_bluetooth_scan(&mut self) {
        if let Some(runtime) = self.bluetooth_runtime.take() {
            runtime.stop();
        }
        if !self.settings.bluetooth_enabled {
            self.push_status("bluetooth scanning disabled".to_string());
            return;
        }

        let runtime = bluetooth::start_scan(
            BluetoothScanConfig {
                controller: self.settings.bluetooth_controller.clone(),
                source: self.settings.bluetooth_scan_source,
                ubertooth_device: self.settings.ubertooth_device.clone(),
                scan_timeout_secs: self.settings.bluetooth_scan_timeout_secs,
                pause_ms: self.settings.bluetooth_scan_pause_ms,
            },
            self.bluetooth_sender.clone(),
        );
        self.bluetooth_runtime = Some(runtime);
    }

    fn active_wifi_interface_name(&self) -> Option<String> {
        self.settings
            .interfaces
            .iter()
            .find(|iface| iface.enabled)
            .map(|iface| {
                iface
                    .monitor_interface_name
                    .clone()
                    .unwrap_or_else(|| iface.interface_name.clone())
            })
    }

    fn lock_wifi_to_channel(
        &mut self,
        channel: u16,
        ht_mode: &str,
        target_label: impl Into<String>,
    ) -> bool {
        let Some(iface) = self.settings.interfaces.first_mut() else {
            self.push_status("no Wi-Fi interface configured for AP lock".to_string());
            return false;
        };

        if self.wifi_lock_restore_mode.is_none() {
            self.wifi_lock_restore_mode = Some(iface.channel_mode.clone());
        }

        iface.channel_mode = ChannelSelectionMode::Locked {
            channel,
            ht_mode: ht_mode.to_string(),
        };
        let target = target_label.into();
        self.wifi_locked_target = Some(target.clone());

        let iface_name = iface
            .monitor_interface_name
            .clone()
            .unwrap_or_else(|| iface.interface_name.clone());
        let restart_message = format!(
            "applying AP lock on {} channel {} ({}) for {}",
            iface_name, channel, ht_mode, target
        );
        if self.capture_runtime.is_some() || self.bluetooth_runtime.is_some() {
            self.begin_async_scan_shutdown(Some(restart_message))
        } else {
            self.push_status(restart_message);
            self.start_scanning();
            self.capture_runtime.is_some() || self.bluetooth_runtime.is_some()
        }
    }

    fn unlock_wifi_card(&mut self) -> bool {
        let Some(restore_mode) = self.wifi_lock_restore_mode.take() else {
            self.push_status("Wi-Fi card is not locked to an AP".to_string());
            return false;
        };
        let Some(iface) = self.settings.interfaces.first_mut() else {
            self.push_status("no Wi-Fi interface configured to unlock".to_string());
            return false;
        };

        iface.channel_mode = restore_mode;
        let iface_name = iface
            .monitor_interface_name
            .clone()
            .unwrap_or_else(|| iface.interface_name.clone());
        let locked_target = self.wifi_locked_target.take();
        let restart_message = format!(
            "unlocking {}{}",
            iface_name,
            locked_target
                .map(|target| format!(" from {}", target))
                .unwrap_or_default()
        );
        if self.capture_runtime.is_some() || self.bluetooth_runtime.is_some() {
            self.begin_async_scan_shutdown(Some(restart_message))
        } else {
            self.push_status(restart_message);
            self.start_scanning();
            self.capture_runtime.is_some() || self.bluetooth_runtime.is_some()
        }
    }

    fn wifi_lock_status_text(&self) -> String {
        match (&self.wifi_locked_target, &self.wifi_lock_restore_mode) {
            (Some(target), Some(_)) => format!("Locked to AP: {}", target),
            _ => "Unlocked".to_string(),
        }
    }

    fn begin_async_scan_shutdown(&mut self, restart_message: Option<String>) -> bool {
        if self.scan_start_in_progress || self.scan_stop_in_progress {
            self.push_status(if restart_message.is_some() {
                "scan transition already in progress".to_string()
            } else {
                "scan transition already in progress".to_string()
            });
            return false;
        }

        let capture_runtime = self.capture_runtime.take();
        let bluetooth_runtime = self.bluetooth_runtime.take();
        if capture_runtime.is_none() && bluetooth_runtime.is_none() {
            if let Some(message) = restart_message {
                self.push_status(message);
                self.start_scanning();
                return self.capture_runtime.is_some() || self.bluetooth_runtime.is_some();
            }
            self.push_status("scanning already stopped".to_string());
            return false;
        }

        self.scan_stop_in_progress = true;
        self.pending_scan_restart_message = restart_message;
        self.push_status(if self.pending_scan_restart_message.is_some() {
            "restarting scans...".to_string()
        } else {
            "stopping scans...".to_string()
        });

        let (tx, rx) = unbounded::<StopCompletion>();
        self.pending_stop_completion = Some(rx);
        let session_capture_path = self.session_capture_path.clone();
        let session_capture_target = self
            .exporter
            .paths
            .pcap_dir
            .join("consolidated_capture.pcapng");
        let interfaces = self.settings.interfaces.clone();
        let restore_types = self.wifi_interface_restore_types.clone();

        thread::spawn(move || {
            if let Some(runtime) = capture_runtime {
                runtime.stop();
            }
            if let Some(runtime) = bluetooth_runtime {
                runtime.stop();
            }

            let mut status_lines = Vec::new();
            if session_capture_path != session_capture_target && session_capture_path.exists() {
                match fs::copy(&session_capture_path, &session_capture_target) {
                    Ok(_) => status_lines.push(format!(
                        "synced live capture into {}",
                        session_capture_target.display()
                    )),
                    Err(err) => status_lines.push(format!(
                        "failed to sync live capture into session directory: {}",
                        err
                    )),
                }
            }

            if !interfaces.is_empty() {
                status_lines.extend(restore_wifi_interfaces(&interfaces, &restore_types));
            }

            let _ = tx.send(StopCompletion {
                status_lines,
                cleared_interfaces: Some(clear_runtime_interface_state(&interfaces)),
            });
        });

        true
    }

    fn update_gps_provider(&mut self, gps_settings: GpsSettings) {
        self.gps_provider.shutdown();
        self.settings.gps = gps_settings.clone();
        self.gps_provider = Arc::from(gps::create_provider(&gps_settings));
    }

    fn maybe_alert_watchlisted_ap(&mut self, ap: &AccessPointRecord) -> bool {
        if !self.settings.enable_watchlist_alerts {
            return false;
        }
        let Some(matched) = ap_watchlist_match(ap, &self.settings.watchlists) else {
            return false;
        };
        let key = format!("ap:{}", matched.alert_key);
        if !self.alerted_watch_entities.insert(key) {
            return false;
        }

        self.push_status(format!(
            "ALERT {}: AP {} ({})",
            matched.label,
            ap.ssid.clone().unwrap_or_else(|| "<hidden>".to_string()),
            ap.bssid
        ));
        emit_alert_tone(1100, 70);
        true
    }

    fn maybe_alert_watchlisted_client(&mut self, client: &ClientRecord) -> bool {
        if !self.settings.enable_watchlist_alerts {
            return false;
        }
        let Some(matched) =
            client_watchlist_match(client, &self.access_points, &self.settings.watchlists)
        else {
            return false;
        };
        let key = format!("client:{}", matched.alert_key);
        if !self.alerted_watch_entities.insert(key) {
            return false;
        }

        self.push_status(format!("ALERT {}: client {}", matched.label, client.mac));
        emit_alert_tone(1000, 70);
        true
    }
}

fn prepare_live_capture_path(session_id: &str) -> Result<PathBuf> {
    let live_root = std::env::temp_dir().join("wirelessexplorer-live");
    fs::create_dir_all(&live_root)
        .with_context(|| format!("failed to create {}", live_root.display()))?;
    #[cfg(target_family = "unix")]
    {
        let _ = fs::set_permissions(&live_root, fs::Permissions::from_mode(0o777));
    }

    let path = live_root.join(format!("live_capture_{}.pcapng", sanitize_name(session_id)));
    if path.exists() {
        let _ = fs::remove_file(&path);
    }
    std::fs::File::create(&path)
        .with_context(|| format!("failed to create live capture file {}", path.display()))?;
    #[cfg(target_family = "unix")]
    {
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o666));
    }
    Ok(path)
}

fn start_persistence_worker(storage: StorageEngine) -> Sender<PersistenceCommand> {
    let (tx, rx) = unbounded::<PersistenceCommand>();
    thread::spawn(move || {
        let mut storage = storage;
        while let Ok(command) = rx.recv() {
            match command {
                PersistenceCommand::ReplaceStorage(new_storage) => storage = new_storage,
                PersistenceCommand::UpsertAccessPoint(ap) => {
                    let _ = storage.upsert_access_point(&ap);
                }
                PersistenceCommand::UpsertClient(client) => {
                    let _ = storage.upsert_client(&client);
                }
                PersistenceCommand::UpsertBluetoothDevice(device) => {
                    let _ = storage.upsert_bluetooth_device(&device);
                }
                PersistenceCommand::AddObservation {
                    device_type,
                    device_id,
                    observation,
                } => {
                    let _ = storage.add_observation(&device_type, &device_id, &observation);
                }
                PersistenceCommand::AddHandshake(handshake) => {
                    let _ = storage.add_handshake(&handshake);
                }
                PersistenceCommand::IncrementHandshakeCount(bssid) => {
                    let _ = storage.increment_handshake_count(&bssid);
                }
                PersistenceCommand::AddChannelUsage(usage) => {
                    let _ = storage.add_channel_usage(&usage);
                }
                PersistenceCommand::AddGpsTrackPoint(point) => {
                    let _ = storage.add_gps_track_point(&point);
                }
                PersistenceCommand::Shutdown => break,
            }
        }
    });
    tx
}

fn internal_runtime_output_root() -> PathBuf {
    let base = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
    let uid = unsafe { libc::geteuid() };
    base.join(format!("wirelessexplorer-runtime-uid{}", uid))
}

fn normalize_rssi_fraction(rssi_dbm: i32) -> f64 {
    ((rssi_dbm + 100) as f64 / 70.0).clamp(0.0, 1.0)
}

struct StopCompletion {
    status_lines: Vec<String>,
    cleared_interfaces: Option<Vec<InterfaceSettings>>,
}

struct StartCompletion {
    updated_interfaces: Option<Vec<InterfaceSettings>>,
    wifi_interface_restore_types: HashMap<String, String>,
    capture_runtime: Option<CaptureRuntime>,
    bluetooth_runtime: Option<BluetoothRuntime>,
    status_lines: Vec<String>,
    privilege_alert: Option<String>,
    wifi_started: bool,
    wifi_failed: bool,
    bluetooth_started: bool,
}

struct WifiStartResult {
    interfaces: Vec<InterfaceSettings>,
    wifi_interface_restore_types: HashMap<String, String>,
    runtime: Option<CaptureRuntime>,
    status_lines: Vec<String>,
    privilege_alert: Option<String>,
    started: bool,
    failed: bool,
}

#[derive(Default)]
struct UiRefreshHint {
    ap_list: bool,
    client_list: bool,
    bluetooth_list: bool,
    channel_chart: bool,
    status: bool,
}

impl UiRefreshHint {
    fn none() -> Self {
        Self::default()
    }

    fn merge(&mut self, other: UiRefreshHint) {
        self.ap_list |= other.ap_list;
        self.client_list |= other.client_list;
        self.bluetooth_list |= other.bluetooth_list;
        self.channel_chart |= other.channel_chart;
        self.status |= other.status;
    }
}

#[derive(Default)]
struct BluetoothGeigerUiState {
    receiver: Option<Receiver<GeigerUpdate>>,
    stop: Option<Arc<AtomicBool>>,
    target_mac: Option<String>,
}

#[derive(Clone)]
struct WifiGeigerTarget {
    track_id: String,
    display_name: String,
    channel: u16,
}

#[derive(Default)]
struct WifiGeigerUiState {
    receiver: Option<Receiver<GeigerUpdate>>,
    stop: Option<Arc<AtomicBool>>,
    target: Option<WifiGeigerTarget>,
    latest_update: Option<GeigerUpdate>,
    last_update_at: Option<Instant>,
    needle_fraction: f64,
    target_fraction: f64,
    last_animation_at: Option<Instant>,
}

#[derive(Debug, Default)]
struct SdrUiModel {
    current_freq_hz: u64,
    sample_rate_hz: u32,
    sweep_paused: bool,
    decoder_running: Option<String>,
    squelch_dbm: f32,
    spectrum_bins: Vec<f32>,
    spectrogram_rows: Vec<Vec<f32>>,
    decode_rows: Vec<SdrDecodeRow>,
    map_points: Vec<SdrMapPoint>,
    satcom_observations: Vec<SdrSatcomObservation>,
    dependency_status: Vec<SdrDependencyStatus>,
}

#[derive(Clone)]
struct UiWidgets {
    ap_root: Paned,
    ap_bottom: Paned,
    ap_detail_notebook: Notebook,
    ap_assoc_box: GtkBox,
    ap_header_holder: GtkBox,
    ap_list: ListBox,
    ap_pagination: TablePaginationUi,
    ap_selection_suppressed: Rc<RefCell<bool>>,
    ap_selected_key: Rc<RefCell<Option<String>>>,
    ap_detail_label: Label,
    ap_notes_view: TextView,
    ap_assoc_header_holder: GtkBox,
    ap_assoc_list: ListBox,
    ap_assoc_pagination: TablePaginationUi,
    ap_packet_draw: DrawingArea,
    ap_selected_packet_mix: Rc<RefCell<PacketTypeBreakdown>>,
    client_header_holder: GtkBox,
    client_list: ListBox,
    client_pagination: TablePaginationUi,
    client_selection_suppressed: Rc<RefCell<bool>>,
    client_selected_key: Rc<RefCell<Option<String>>>,
    client_detail_label: Label,
    client_root: Paned,
    client_detail_notebook: Notebook,
    ap_wifi_geiger_target_label: Label,
    ap_wifi_geiger_lock_label: Label,
    ap_wifi_geiger_rssi: Label,
    ap_wifi_geiger_tone: Label,
    ap_wifi_geiger_meter: DrawingArea,
    client_wifi_geiger_target_label: Label,
    client_wifi_geiger_lock_label: Label,
    client_wifi_geiger_rssi: Label,
    client_wifi_geiger_tone: Label,
    client_wifi_geiger_meter: DrawingArea,
    wifi_geiger_state: Rc<RefCell<WifiGeigerUiState>>,
    bluetooth_list: ListBox,
    bluetooth_header_holder: GtkBox,
    bluetooth_pagination: TablePaginationUi,
    bluetooth_selection_suppressed: Rc<RefCell<bool>>,
    bluetooth_selected_key: Rc<RefCell<Option<String>>>,
    bluetooth_detail_box: GtkBox,
    bluetooth_identity_label: Label,
    bluetooth_passive_label: Label,
    bluetooth_active_summary_label: Label,
    bluetooth_readable_label: Label,
    bluetooth_services_label: Label,
    bluetooth_characteristics_label: Label,
    bluetooth_descriptors_label: Label,
    bluetooth_root: Paned,
    bluetooth_bottom: Paned,
    bluetooth_geiger_rssi: Label,
    bluetooth_geiger_tone: Label,
    bluetooth_geiger_progress: ProgressBar,
    bluetooth_geiger_state: Rc<RefCell<BluetoothGeigerUiState>>,
    channel_draw: DrawingArea,
    sdr_frequency_label: Label,
    sdr_decoder_label: Label,
    sdr_dependency_label: Label,
    sdr_fft_draw: DrawingArea,
    sdr_spectrogram_draw: DrawingArea,
    sdr_map_draw: DrawingArea,
    sdr_decode_header_holder: GtkBox,
    sdr_decode_list: ListBox,
    sdr_decode_pagination: TablePaginationUi,
    sdr_satcom_header_holder: GtkBox,
    sdr_satcom_list: ListBox,
    sdr_satcom_pagination: TablePaginationUi,
    sdr_model: Rc<RefCell<SdrUiModel>>,
    status_label: Label,
    gps_status_label: Label,
}

#[derive(Clone)]
struct TablePaginationUi {
    current_page: Rc<Cell<usize>>,
    page_size: Rc<Cell<usize>>,
    generation: Rc<Cell<u64>>,
    page_size_combo: ComboBoxText,
    prev_button: Button,
    next_button: Button,
    page_entry: Entry,
    page_go_button: Button,
    filter_bar: Grid,
    filter_entries: Rc<RefCell<HashMap<String, Entry>>>,
    filter_order: Rc<Vec<String>>,
    summary_label: Label,
}

fn build_table_pagination_controls(
    default_page_size: usize,
    filter_columns: Vec<(String, String, i32)>,
) -> (GtkBox, TablePaginationUi) {
    let current_page = Rc::new(Cell::new(0_usize));
    let page_size = Rc::new(Cell::new(default_page_size.max(1)));
    let generation = Rc::new(Cell::new(0_u64));
    let filter_entries: Rc<RefCell<HashMap<String, Entry>>> = Rc::new(RefCell::new(HashMap::new()));
    let filter_order = Rc::new(
        filter_columns
            .iter()
            .map(|(id, _, _)| id.clone())
            .collect::<Vec<_>>(),
    );

    let container = GtkBox::new(Orientation::Horizontal, 8);
    container.set_margin_top(4);
    let controls_row = GtkBox::new(Orientation::Horizontal, 8);
    controls_row.set_hexpand(true);
    let filter_bar = Grid::new();
    filter_bar.set_column_spacing(14);
    filter_bar.set_hexpand(true);

    let rows_label = Label::new(Some("Rows"));
    rows_label.set_xalign(0.0);

    let page_size_combo = ComboBoxText::new();
    for size in TABLE_PAGE_SIZE_OPTIONS {
        let text = size.to_string();
        page_size_combo.append(Some(&text), &text);
    }
    page_size_combo.set_active_id(Some(&default_page_size.to_string()));

    let prev_button = Button::with_label("Previous");
    let next_button = Button::with_label("Next");
    let page_label = Label::new(Some("Page"));
    let page_entry = Entry::new();
    page_entry.set_width_chars(4);
    page_entry.set_max_width_chars(6);
    page_entry.set_text("1");
    let page_go_button = Button::with_label("Go");
    let clear_filters_button = Button::with_label("Clear");
    let filter_summary_label = Label::new(Some("No active column filters"));
    filter_summary_label.set_xalign(0.0);
    filter_summary_label.set_hexpand(true);
    let summary_label = Label::new(Some("Showing 0 of 0 | Page 1 of 1"));
    summary_label.set_xalign(0.0);
    summary_label.set_hexpand(true);

    controls_row.append(&rows_label);
    controls_row.append(&page_size_combo);
    controls_row.append(&prev_button);
    controls_row.append(&next_button);
    controls_row.append(&page_label);
    controls_row.append(&page_entry);
    controls_row.append(&page_go_button);
    controls_row.append(&clear_filters_button);
    controls_row.append(&summary_label);

    for (column_index, (column_id, column_label, width_chars)) in filter_columns.iter().enumerate()
    {
        let entry = Entry::new();
        let entry_width = (*width_chars).max(6);
        entry.add_css_class("table-cell");
        entry.add_css_class("column-filter");
        gtk::prelude::EntryExt::set_alignment(&entry, 0.0);
        entry.set_has_frame(false);
        entry.set_width_chars(entry_width);
        entry.set_max_width_chars(entry_width);
        entry.set_size_request(entry_width * TABLE_CHAR_WIDTH_PX, -1);
        entry.set_margin_end(6);
        entry.set_placeholder_text(Some(column_label));
        entry.set_tooltip_text(Some(&format!("Filter {}", column_label)));
        filter_bar.attach(&entry, column_index as i32, 0, 1, 1);
        filter_entries
            .borrow_mut()
            .insert(column_id.clone(), entry.clone());
        let current_page = current_page.clone();
        let generation = generation.clone();
        let filter_entries_for_change = filter_entries.clone();
        let filter_summary_label_for_change = filter_summary_label.clone();
        let filter_columns_for_change = filter_columns.clone();
        entry.connect_changed(move |_| {
            current_page.set(0);
            generation.set(generation.get().saturating_add(1));
            update_filter_summary_label(
                &filter_summary_label_for_change,
                &filter_columns_for_change
                    .iter()
                    .map(|(id, label, _)| (id.clone(), label.clone()))
                    .collect::<Vec<_>>(),
                &filter_entries_for_change.borrow(),
            );
        });
    }
    controls_row.append(&filter_summary_label);

    container.append(&controls_row);

    {
        let page_size = page_size.clone();
        let current_page = current_page.clone();
        let generation = generation.clone();
        page_size_combo.connect_changed(move |combo| {
            let selected = combo
                .active_id()
                .and_then(|value| value.as_str().parse::<usize>().ok())
                .unwrap_or(DEFAULT_TABLE_PAGE_SIZE)
                .max(1);
            page_size.set(selected);
            current_page.set(0);
            generation.set(generation.get().saturating_add(1));
        });
    }

    {
        let current_page = current_page.clone();
        let generation = generation.clone();
        prev_button.connect_clicked(move |_| {
            let page = current_page.get();
            if page > 0 {
                current_page.set(page - 1);
                generation.set(generation.get().saturating_add(1));
            }
        });
    }

    {
        let current_page = current_page.clone();
        let generation = generation.clone();
        next_button.connect_clicked(move |_| {
            current_page.set(current_page.get().saturating_add(1));
            generation.set(generation.get().saturating_add(1));
        });
    }

    {
        let current_page = current_page.clone();
        let generation = generation.clone();
        let page_entry_for_button = page_entry.clone();
        page_go_button.connect_clicked(move |_| {
            let requested_page = page_entry_for_button
                .text()
                .trim()
                .parse::<usize>()
                .ok()
                .unwrap_or(1)
                .max(1);
            current_page.set(requested_page - 1);
            generation.set(generation.get().saturating_add(1));
        });
    }

    {
        let current_page = current_page.clone();
        let generation = generation.clone();
        let page_entry_for_activate = page_entry.clone();
        page_entry.connect_activate(move |_| {
            let requested_page = page_entry_for_activate
                .text()
                .trim()
                .parse::<usize>()
                .ok()
                .unwrap_or(1)
                .max(1);
            current_page.set(requested_page - 1);
            generation.set(generation.get().saturating_add(1));
        });
    }

    {
        let current_page = current_page.clone();
        let generation = generation.clone();
        let filter_entries_for_clear = filter_entries.clone();
        let filter_summary_label_for_clear = filter_summary_label.clone();
        let filter_columns_for_clear = filter_columns.clone();
        clear_filters_button.connect_clicked(move |_| {
            let entries = filter_entries_for_clear.borrow();
            let had_filters = entries
                .values()
                .any(|entry| !entry.text().trim().is_empty());
            for entry in entries.values() {
                if !entry.text().is_empty() {
                    entry.set_text("");
                }
            }
            drop(entries);
            update_filter_summary_label(
                &filter_summary_label_for_clear,
                &filter_columns_for_clear
                    .iter()
                    .map(|(id, label, _)| (id.clone(), label.clone()))
                    .collect::<Vec<_>>(),
                &filter_entries_for_clear.borrow(),
            );
            if had_filters {
                current_page.set(0);
                generation.set(generation.get().saturating_add(1));
            }
        });
    }

    update_filter_summary_label(
        &filter_summary_label,
        &filter_columns
            .iter()
            .map(|(id, label, _)| (id.clone(), label.clone()))
            .collect::<Vec<_>>(),
        &filter_entries.borrow(),
    );

    (
        container,
        TablePaginationUi {
            current_page,
            page_size,
            generation,
            page_size_combo,
            prev_button,
            next_button,
            page_entry,
            page_go_button,
            filter_bar,
            filter_entries,
            filter_order,
            summary_label,
        },
    )
}

fn update_filter_summary_label(
    label: &Label,
    filter_columns: &[(String, String)],
    entries: &HashMap<String, Entry>,
) {
    let mut active = Vec::new();
    for (column_id, column_label) in filter_columns {
        let Some(entry) = entries.get(column_id) else {
            continue;
        };
        let text = entry.text().trim().to_string();
        if !text.is_empty() {
            active.push(format!("{column_label}: {text}"));
        }
    }

    if active.is_empty() {
        label.set_text("No active column filters");
    } else {
        label.set_text(&active.join(" | "));
    }
}

#[derive(Clone)]
struct PaginationDefaultsUi {
    ap: TablePaginationUi,
    client: TablePaginationUi,
    assoc: TablePaginationUi,
    bluetooth: TablePaginationUi,
}

fn table_filter_columns(
    layout: &TableLayout,
    label_for: fn(&str) -> &'static str,
) -> Vec<(String, String, i32)> {
    layout
        .columns
        .iter()
        .filter(|column| column.visible)
        .map(|column| {
            (
                column.id.clone(),
                label_for(&column.id).to_string(),
                column.width_chars,
            )
        })
        .collect()
}

fn build_ui(app: &Application) -> Result<()> {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("WirelessExplorer")
        .default_width(DEFAULT_WINDOW_WIDTH)
        .default_height(DEFAULT_WINDOW_HEIGHT)
        .build();
    let output_dir = {
        let fallback = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        fallback.join("output")
    };
    let runtime_output_dir = internal_runtime_output_root();
    std::fs::create_dir_all(&runtime_output_dir).with_context(|| {
        format!(
            "failed to create internal runtime output directory {}",
            runtime_output_dir.display()
        )
    })?;

    let settings_path = settings_file_path();
    let (mut settings, settings_status_line) = match AppSettings::load_from_disk() {
        Ok(settings) => (
            settings,
            Some(format!(
                "loaded preferences from {}",
                settings_path.display()
            )),
        ),
        Err(err) if !settings_path.exists() => (AppSettings::default(), None),
        Err(err) => (
            AppSettings::default(),
            Some(format!(
                "failed to load preferences from {}: {}; using defaults",
                settings_path.display(),
                err
            )),
        ),
    };
    if settings.output_root.as_os_str().is_empty() {
        settings.output_root = output_dir.clone();
    }
    sanitize_table_layout(&mut settings.ap_table_layout, &default_ap_table_layout());
    sanitize_table_layout(
        &mut settings.client_table_layout,
        &default_client_table_layout(),
    );
    sanitize_table_layout(
        &mut settings.assoc_client_table_layout,
        &default_assoc_client_table_layout(),
    );
    migrate_assoc_client_table_layout(&mut settings.assoc_client_table_layout);
    sanitize_table_layout(
        &mut settings.bluetooth_table_layout,
        &default_bluetooth_table_layout(),
    );
    migrate_legacy_bluetooth_table_layout(&mut settings.bluetooth_table_layout);
    migrate_watchlist_settings(&mut settings.watchlists);
    if !TABLE_PAGE_SIZE_OPTIONS.contains(&settings.default_rows_per_page) {
        settings.default_rows_per_page = DEFAULT_TABLE_PAGE_SIZE;
    }
    let watchlist_css_provider = install_ui_css();
    apply_watchlist_css(&watchlist_css_provider, &settings.watchlists);

    let interface_settings = detect_interface_settings();
    settings.interfaces = if interface_settings.is_empty() {
        vec![InterfaceSettings {
            interface_name: "wlan0".to_string(),
            monitor_interface_name: None,
            channel_mode: ChannelSelectionMode::HopAll {
                channels: vec![1, 6, 11, 36, 40, 44, 48],
                dwell_ms: 200,
            },
            enabled: true,
        }]
    } else {
        if settings.interfaces.is_empty() {
            interface_settings
        } else {
            settings.interfaces.clone()
        }
    };

    let session_id = Uuid::new_v4().to_string();
    let exporter = ExportManager::new(&runtime_output_dir, &session_id)?;
    exporter.create_initial_outputs()?;

    let sqlite_path = exporter.paths.session_dir.join("wirelessexplorer.sqlite");
    let storage = StorageEngine::open(&sqlite_path)?;
    let persistence_sender = start_persistence_worker(storage.clone());
    let existing_gps_track = storage.load_gps_track().unwrap_or_default();
    let existing_bluetooth_devices = storage.load_bluetooth_devices().unwrap_or_default();
    let existing_bluetooth_count = existing_bluetooth_devices.len();

    let session_meta = SessionMetadata {
        id: session_id.clone(),
        started_at: Utc::now(),
        output_dir: exporter.paths.session_dir.to_string_lossy().to_string(),
        selected_interfaces: settings
            .interfaces
            .iter()
            .map(|i| i.interface_name.clone())
            .collect(),
    };
    storage.save_session(&session_meta)?;

    let oui = OuiDatabase::load_with_override(Some(&settings.oui_source_path))
        .or_else(|_| OuiDatabase::load_default())
        .unwrap_or_else(|_| OuiDatabase::empty());

    let gps_provider = Arc::from(gps::create_provider(&settings.gps));

    if settings.bluetooth_controller.is_none() {
        if let Ok(controllers) = bluetooth::list_controllers() {
            if let Some(default_ctrl) = controllers
                .iter()
                .find(|c| c.is_default)
                .or_else(|| controllers.first())
            {
                settings.bluetooth_controller = Some(default_ctrl.id.clone());
            }
        }
    }

    let (capture_tx, capture_rx) = unbounded::<CaptureEvent>();
    let (bluetooth_tx, bluetooth_rx) = unbounded::<BluetoothEvent>();
    let (sdr_tx, sdr_rx) = unbounded::<SdrEvent>();
    let session_capture_path = prepare_live_capture_path(&session_id)?;

    let runtime: Option<CaptureRuntime> = None;
    let bluetooth_runtime: Option<BluetoothRuntime> = None;
    let sdr_runtime: Option<SdrRuntime> = None;

    let initial_gps_track_points = existing_gps_track.len();
    let bluetooth_controller_status = settings
        .bluetooth_controller
        .clone()
        .unwrap_or_else(|| "<default>".to_string());
    let bluetooth_source_status = match settings.bluetooth_scan_source {
        BluetoothScanSource::Bluez => "BlueZ",
        BluetoothScanSource::Ubertooth => "Ubertooth",
        BluetoothScanSource::Both => "BlueZ + Ubertooth",
    };
    let ubertooth_device_status = settings
        .ubertooth_device
        .clone()
        .unwrap_or_else(|| "<default>".to_string());

    let state = Rc::new(RefCell::new(AppState {
        settings,
        storage,
        persistence_sender,
        exporter,
        oui: oui.clone(),
        gps_provider,
        access_points: Vec::new(),
        clients: Vec::new(),
        bluetooth_devices: existing_bluetooth_devices,
        channel_usage: Vec::new(),
        capture_runtime: runtime,
        capture_sender: capture_tx,
        bluetooth_runtime,
        bluetooth_sender: bluetooth_tx,
        sdr_runtime,
        sdr_sender: sdr_tx,
        session_capture_path,
        gps_track: existing_gps_track,
        last_gps_track_point_at: None,
        status_lines: {
            let mut lines = vec!["scanning idle (click Start)".to_string()];
            if let Some(line) = settings_status_line {
                lines.push(line);
            }
            lines.push(format!(
                "privilege mode: {}",
                capture::privilege_mode_summary()
            ));
            lines.push(format!("loaded local OUI entries: {}", oui.count()));
            lines.push(format!(
                "loaded bluetooth devices: {}",
                existing_bluetooth_count
            ));
            lines.push(format!(
                "bluetooth controller: {}",
                bluetooth_controller_status
            ));
            lines.push(format!("bluetooth source: {}", bluetooth_source_status));
            lines.push(format!("ubertooth device: {}", ubertooth_device_status));
            lines.push(format!(
                "loaded GPS track points: {}",
                initial_gps_track_points
            ));
            lines.push(format!(
                "GPS output coordinates fixed to {}, {}",
                FAKE_GPS_LATITUDE, FAKE_GPS_LONGITUDE
            ));
            lines
        },
        last_observation_by_device: HashMap::new(),
        last_storage_persist_by_device: HashMap::new(),
        alerted_watch_entities: HashSet::new(),
        watchlist_css_provider,
        layout_dirty: false,
        ap_sort: TableSortState::new("last_seen", true),
        client_sort: TableSortState::new("last_heard", true),
        assoc_sort: TableSortState::new("last_heard", true),
        bluetooth_sort: TableSortState::new("last_seen", true),
        pending_privilege_alert: None,
        wifi_lock_restore_mode: None,
        wifi_locked_target: None,
        wifi_interface_restore_types: HashMap::new(),
        scan_start_in_progress: false,
        scan_stop_in_progress: false,
        pending_start_completion: None,
        pending_stop_completion: None,
        pending_scan_restart_message: None,
    }));
    state.borrow_mut().backfill_oui_labels();

    let global_status_label = Label::new(Some("starting"));
    global_status_label.set_xalign(0.0);
    global_status_label.set_wrap(true);
    global_status_label.set_selectable(true);

    let global_gps_status_label = Label::new(Some("GPS status initializing"));
    global_gps_status_label.set_xalign(0.0);
    global_gps_status_label.set_wrap(true);
    global_gps_status_label.set_selectable(true);

    let global_status_box = GtkBox::new(Orientation::Vertical, 4);
    global_status_box.set_margin_top(6);
    global_status_box.set_margin_bottom(8);
    global_status_box.set_margin_start(8);
    global_status_box.set_margin_end(8);
    global_status_box.append(&Label::new(Some("Status")));
    global_status_box.append(&global_status_label);
    global_status_box.append(&Label::new(Some("GPS Status")));
    global_status_box.append(&global_gps_status_label);

    let global_status_scrolled = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .min_content_height(160)
        .child(&global_status_box)
        .build();
    let global_status_container = GtkBox::new(Orientation::Vertical, 0);
    global_status_container.append(&global_status_scrolled);

    let root = GtkBox::new(Orientation::Vertical, 8);
    let (notebook, widgets) = build_tabs(&window, state.clone());
    notebook.set_hexpand(true);
    notebook.set_vexpand(true);
    let content_paned = Paned::new(Orientation::Vertical);
    content_paned.set_wide_handle(true);
    content_paned.set_position(DEFAULT_CONTENT_PANE_POSITION);
    content_paned.set_resize_start_child(true);
    content_paned.set_resize_end_child(true);
    content_paned.set_shrink_start_child(false);
    content_paned.set_shrink_end_child(false);
    content_paned.set_start_child(Some(&notebook));
    content_paned.set_end_child(Some(&global_status_container));
    let pagination_defaults = PaginationDefaultsUi {
        ap: widgets.ap_pagination.clone(),
        client: widgets.client_pagination.clone(),
        assoc: widgets.ap_assoc_pagination.clone(),
        bluetooth: widgets.bluetooth_pagination.clone(),
    };
    let menu = build_menubar(
        app,
        &window,
        state.clone(),
        &content_paned,
        &global_status_container,
        &pagination_defaults,
        &widgets,
    );
    root.append(&menu);
    let (controls, capture_start_btn, capture_stop_btn) =
        build_capture_controls(&window, state.clone());
    root.append(&controls);
    root.append(&content_paned);

    {
        let notebook = notebook.clone();
        let widgets = widgets.clone();
        let key_controller = EventControllerKey::new();
        key_controller.connect_key_pressed(move |_, key, _, modifier| {
            if modifier.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::f {
                let pagination = match notebook.current_page().unwrap_or(ACCESS_POINTS_TAB_INDEX) {
                    ACCESS_POINTS_TAB_INDEX => widgets.ap_pagination.clone(),
                    CLIENTS_TAB_INDEX => widgets.client_pagination.clone(),
                    BLUETOOTH_TAB_INDEX => widgets.bluetooth_pagination.clone(),
                    _ => return glib::Propagation::Proceed,
                };
                focus_first_filter_entry(&pagination);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(key_controller);
    }

    window.set_child(Some(&root));
    apply_view_visibility(
        &state.borrow().settings,
        &content_paned,
        &global_status_container,
        &widgets,
    );
    window.present();

    bind_poll_loop(
        capture_rx,
        bluetooth_rx,
        sdr_rx,
        state.clone(),
        widgets,
        capture_start_btn,
        capture_stop_btn,
        global_status_label,
        global_gps_status_label,
        notebook.clone(),
        &window,
    );

    if std::env::var_os("WIRELESSEXPLORER_AUTOSTART").is_some()
        || std::env::var_os("SIMPLESTG_AUTOSTART").is_some()
    {
        state.borrow_mut().start_scanning();
    }

    if let Some(value) = std::env::var_os("WIRELESSEXPLORER_AUTOSTOP_AFTER_SECS")
        .or_else(|| std::env::var_os("SIMPLESTG_AUTOSTOP_AFTER_SECS"))
    {
        if let Ok(delay_secs) = value.to_string_lossy().parse::<u32>() {
            let state_for_autostop = state.clone();
            let armed_at = Rc::new(Cell::new(None::<Instant>));
            glib::timeout_add_seconds_local(1, move || {
                let mut state = state_for_autostop.borrow_mut();
                let runtime_active =
                    state.capture_runtime.is_some() || state.bluetooth_runtime.is_some();
                let transition_active = state.scan_start_in_progress || state.scan_stop_in_progress;

                if !runtime_active {
                    armed_at.set(None);
                    return glib::ControlFlow::Continue;
                }

                if transition_active {
                    return glib::ControlFlow::Continue;
                }

                let now = Instant::now();
                let started = armed_at.get().unwrap_or(now);
                if armed_at.get().is_none() {
                    armed_at.set(Some(now));
                    return glib::ControlFlow::Continue;
                }

                if now.duration_since(started) >= Duration::from_secs(delay_secs as u64) {
                    state.stop_scanning();
                    return glib::ControlFlow::Break;
                }

                glib::ControlFlow::Continue
            });
        }
    }

    let state_for_shutdown = state.clone();
    app.connect_shutdown(move |_| {
        let mut state = state_for_shutdown.borrow_mut();
        if let Some(runtime) = state.capture_runtime.take() {
            runtime.stop();
        }
        if let Some(runtime) = state.bluetooth_runtime.take() {
            runtime.stop();
        }
        if let Some(runtime) = state.sdr_runtime.take() {
            runtime.stop();
        }
        let restore_status = restore_wifi_interfaces(
            &state.settings.interfaces,
            &state.wifi_interface_restore_types,
        );
        state.settings.interfaces = clear_runtime_interface_state(&state.settings.interfaces);
        state.wifi_interface_restore_types.clear();
        for line in restore_status {
            state.push_status(line);
        }
        let _ = state.persistence_sender.send(PersistenceCommand::Shutdown);
        capture::shutdown_privileged_helper();
        state.gps_provider.shutdown();
    });

    Ok(())
}

fn build_menubar(
    app: &Application,
    window: &ApplicationWindow,
    state: Rc<RefCell<AppState>>,
    content_paned: &Paned,
    global_status_box: &GtkBox,
    pagination_defaults: &PaginationDefaultsUi,
    widgets: &UiWidgets,
) -> gtk::PopoverMenuBar {
    let export_all_action = gio::SimpleAction::new("export_all", None);
    {
        let state = state.clone();
        export_all_action.connect_activate(move |_, _| {
            let mut s = state.borrow_mut();
            let ap_csv = s.exporter.export_access_points_csv(&s.access_points);
            let client_csv = s.exporter.export_clients_csv(&s.clients);
            let summary_json =
                s.exporter
                    .export_summary_json(&s.access_points, &s.clients, &s.bluetooth_devices);
            let gps_track = s.gps_track_for_export();
            let gps_pcap = s
                .exporter
                .export_session_pcap_with_gps(&s.session_capture_path, &gps_track);
            match (ap_csv, client_csv, summary_json, gps_pcap) {
                (Ok(_), Ok(_), Ok(_), Ok(_)) => s.push_status(
                    "exported AP/client CSV + summary JSON + consolidated GPS PCAPNG".to_string(),
                ),
                (ap_res, client_res, json_res, pcap_res) => s.push_status(format!(
                    "export incomplete: ap_csv={} client_csv={} summary_json={} pcap={}",
                    ap_res
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "ok".to_string()),
                    client_res
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "ok".to_string()),
                    json_res
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "ok".to_string()),
                    pcap_res
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "ok".to_string())
                )),
            };
        });
    }
    app.add_action(&export_all_action);

    let export_locations_action = gio::SimpleAction::new("export_locations", None);
    {
        let state = state.clone();
        export_locations_action.connect_activate(move |_, _| {
            let mut s = state.borrow_mut();
            let csv = s.exporter.export_location_logs_csv(
                &s.access_points,
                &s.clients,
                &s.bluetooth_devices,
            );
            let kml = s.exporter.export_location_logs_kml(
                &s.access_points,
                &s.clients,
                &s.bluetooth_devices,
            );
            let kmz = s.exporter.export_location_logs_kmz(
                &s.access_points,
                &s.clients,
                &s.bluetooth_devices,
            );
            let summary_json =
                s.exporter
                    .export_summary_json(&s.access_points, &s.clients, &s.bluetooth_devices);
            match (csv, kml, kmz, summary_json) {
                (Ok(_), Ok(_), Ok(_), Ok(_)) => s.push_status(
                    "exported location logs (CSV + KML + KMZ) and summary JSON".to_string(),
                ),
                (csv_res, kml_res, kmz_res, json_res) => s.push_status(format!(
                    "location export incomplete: csv={} kml={} kmz={} summary_json={}",
                    csv_res
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "ok".to_string()),
                    kml_res
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "ok".to_string()),
                    kmz_res
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "ok".to_string()),
                    json_res
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "ok".to_string())
                )),
            }
        });
    }
    app.add_action(&export_locations_action);

    let settings_window_action = gio::SimpleAction::new("settings_preferences", None);
    {
        let window = window.clone();
        let state = state.clone();
        let content_paned = content_paned.clone();
        let global_status_box = global_status_box.clone();
        let pagination_defaults = pagination_defaults.clone();
        let widgets = widgets.clone();
        settings_window_action.connect_activate(move |_, _| {
            open_preferences_window(
                &window,
                state.clone(),
                &content_paned,
                &global_status_box,
                &pagination_defaults,
                &widgets,
            );
        });
    }
    app.add_action(&settings_window_action);

    let settings_interface_action = gio::SimpleAction::new("settings_interface", None);
    {
        let window = window.clone();
        let state = state.clone();
        settings_interface_action.connect_activate(move |_, _| {
            open_interface_settings_dialog(&window, state.clone());
        });
    }
    app.add_action(&settings_interface_action);

    let settings_gps_action = gio::SimpleAction::new("settings_gps", None);
    {
        let window = window.clone();
        let state = state.clone();
        settings_gps_action.connect_activate(move |_, _| {
            open_gps_settings_dialog(&window, state.clone());
        });
    }
    app.add_action(&settings_gps_action);

    let settings_bluetooth_action = gio::SimpleAction::new("settings_bluetooth", None);
    {
        let window = window.clone();
        let state = state.clone();
        settings_bluetooth_action.connect_activate(move |_, _| {
            open_bluetooth_settings_dialog(&window, state.clone());
        });
    }
    app.add_action(&settings_bluetooth_action);

    let show_status_bar_initial = state.borrow().settings.show_status_bar;
    let settings_show_status_bar_action = gio::SimpleAction::new_stateful(
        "settings_show_status_bar",
        None,
        &glib::Variant::from(show_status_bar_initial),
    );
    {
        let state = state.clone();
        let content_paned = content_paned.clone();
        let global_status_box = global_status_box.clone();
        let widgets = widgets.clone();
        settings_show_status_bar_action.connect_activate(move |action, _| {
            let current = action
                .state()
                .and_then(|variant| variant.get::<bool>())
                .unwrap_or(false);
            let next = !current;
            action.set_state(&glib::Variant::from(next));
            apply_view_preferences(
                &state,
                &content_paned,
                &global_status_box,
                &widgets,
                Some(next),
                None,
                None,
            );
        });
    }
    app.add_action(&settings_show_status_bar_action);

    let show_detail_pane_initial = state.borrow().settings.show_detail_pane;
    let settings_show_detail_pane_action = gio::SimpleAction::new_stateful(
        "settings_show_detail_pane",
        None,
        &glib::Variant::from(show_detail_pane_initial),
    );
    {
        let state = state.clone();
        let content_paned = content_paned.clone();
        let global_status_box = global_status_box.clone();
        let widgets = widgets.clone();
        settings_show_detail_pane_action.connect_activate(move |action, _| {
            let current = action
                .state()
                .and_then(|variant| variant.get::<bool>())
                .unwrap_or(true);
            let next = !current;
            action.set_state(&glib::Variant::from(next));
            apply_view_preferences(
                &state,
                &content_paned,
                &global_status_box,
                &widgets,
                None,
                Some(next),
                None,
            );
        });
    }
    app.add_action(&settings_show_detail_pane_action);

    let show_device_pane_initial = state.borrow().settings.show_device_pane;
    let settings_show_device_pane_action = gio::SimpleAction::new_stateful(
        "settings_show_device_pane",
        None,
        &glib::Variant::from(show_device_pane_initial),
    );
    {
        let state = state.clone();
        let content_paned = content_paned.clone();
        let global_status_box = global_status_box.clone();
        let widgets = widgets.clone();
        settings_show_device_pane_action.connect_activate(move |action, _| {
            let current = action
                .state()
                .and_then(|variant| variant.get::<bool>())
                .unwrap_or(true);
            let next = !current;
            action.set_state(&glib::Variant::from(next));
            apply_view_preferences(
                &state,
                &content_paned,
                &global_status_box,
                &widgets,
                None,
                None,
                Some(next),
            );
        });
    }
    app.add_action(&settings_show_device_pane_action);

    let set_default_rows_per_page =
        |rows: usize, state: &Rc<RefCell<AppState>>, pagination_defaults: &PaginationDefaultsUi| {
            {
                let mut s = state.borrow_mut();
                s.settings.default_rows_per_page = rows.max(1);
                s.push_status(format!("default rows per page set to {}", rows.max(1)));
            }
            for pagination in [
                &pagination_defaults.ap,
                &pagination_defaults.client,
                &pagination_defaults.assoc,
                &pagination_defaults.bluetooth,
            ] {
                pagination
                    .page_size_combo
                    .set_active_id(Some(&rows.max(1).to_string()));
                pagination.current_page.set(0);
                pagination
                    .generation
                    .set(pagination.generation.get().saturating_add(1));
            }
        };

    for rows in TABLE_PAGE_SIZE_OPTIONS {
        let action_name = format!("settings_default_rows_{}", rows);
        let action = gio::SimpleAction::new(&action_name, None);
        let state = state.clone();
        let pagination_defaults = pagination_defaults.clone();
        let rows = *rows;
        action.connect_activate(move |_, _| {
            set_default_rows_per_page(rows, &state, &pagination_defaults);
        });
        app.add_action(&action);
    }

    let layout_action = gio::SimpleAction::new("layout_config", None);
    {
        let window = window.clone();
        let state = state.clone();
        layout_action.connect_activate(move |_, _| {
            open_layout_dialog(&window, state.clone());
        });
    }
    app.add_action(&layout_action);

    let update_oui_action = gio::SimpleAction::new("update_oui", None);
    {
        let state = state.clone();
        update_oui_action.connect_activate(move |_, _| {
            let mut s = state.borrow_mut();
            let target = if s.settings.oui_source_path.as_os_str().is_empty() {
                OuiDatabase::persistent_cache_path()
                    .unwrap_or_else(|| s.exporter.paths.session_dir.join("oui_latest.csv"))
            } else {
                s.settings.oui_source_path.clone()
            };
            match s.oui.refresh_from_ieee(&target) {
                Ok(_) => match OuiDatabase::load_from_path(&target) {
                    Ok(db) => {
                        s.oui = db;
                        s.backfill_oui_labels();
                        let oui_count = s.oui.count();
                        s.push_status(format!(
                            "OUI database updated from IEEE ({} entries)",
                            oui_count
                        ));
                    }
                    Err(err) => {
                        s.push_status(format!("OUI update downloaded but reload failed: {err}"));
                    }
                },
                Err(err) => {
                    match OuiDatabase::load_with_override(Some(&s.settings.oui_source_path))
                        .or_else(|_| OuiDatabase::load_default())
                    {
                        Ok(db) if db.count() > 0 => {
                            let oui_count = db.count();
                            s.oui = db;
                            s.backfill_oui_labels();
                            s.push_status(format!(
                            "OUI update failed: {err}; reloaded local OUI database ({} entries)",
                            oui_count
                        ));
                        }
                        _ => s.push_status(format!("OUI update failed: {err}")),
                    }
                }
            }
        });
    }
    app.add_action(&update_oui_action);

    let quit_action = gio::SimpleAction::new("quit_app", None);
    {
        let app = app.clone();
        quit_action.connect_activate(move |_, _| {
            app.quit();
        });
    }
    app.add_action(&quit_action);

    let file_menu = gio::Menu::new();
    file_menu.append(
        Some("Export CSV + Summary JSON + Consolidated PCAP"),
        Some("app.export_all"),
    );
    file_menu.append(
        Some("Export Location Logs (CSV + KML + KMZ + JSON)"),
        Some("app.export_locations"),
    );
    file_menu.append(Some("Update OUI Database"), Some("app.update_oui"));
    file_menu.append(Some("Quit"), Some("app.quit_app"));

    let view_menu = gio::Menu::new();
    view_menu.append(Some("Device Pane"), Some("app.settings_show_device_pane"));
    view_menu.append(Some("Details Pane"), Some("app.settings_show_detail_pane"));
    view_menu.append(Some("Status Pane"), Some("app.settings_show_status_bar"));

    let settings_menu = gio::Menu::new();
    settings_menu.append_submenu(Some("View"), &view_menu);
    settings_menu.append(Some("Preferences"), Some("app.settings_preferences"));

    let root = gio::Menu::new();
    root.append_submenu(Some("File"), &file_menu);
    root.append_submenu(Some("Settings"), &settings_menu);

    gtk::PopoverMenuBar::from_model(Some(&root))
}

fn set_scan_control_button_sensitivity(
    start_btn: &Button,
    stop_btn: &Button,
    wifi_running: bool,
    bluetooth_running: bool,
    scan_transition_in_progress: bool,
) {
    if scan_transition_in_progress {
        start_btn.set_sensitive(false);
        stop_btn.set_sensitive(false);
        return;
    }
    let any_running = wifi_running || bluetooth_running;
    let all_running = wifi_running && bluetooth_running;
    start_btn.set_sensitive(!all_running);
    stop_btn.set_sensitive(any_running);
}

fn describe_channel_mode(mode: &ChannelSelectionMode) -> String {
    match mode {
        ChannelSelectionMode::Locked { channel, ht_mode } => {
            format!("Locked {} ({})", channel, ht_mode)
        }
        ChannelSelectionMode::HopBand {
            band,
            channels,
            dwell_ms,
        } => format!(
            "Hop {:?} [{} channels @ {} ms]",
            band,
            channels.len(),
            dwell_ms
        ),
        ChannelSelectionMode::HopAll { channels, dwell_ms } => {
            format!(
                "Hop Specific [{} channels @ {} ms]",
                channels.len(),
                dwell_ms
            )
        }
    }
}

fn apply_view_visibility(
    settings: &AppSettings,
    content_paned: &Paned,
    status_container: &GtkBox,
    widgets: &UiWidgets,
) {
    content_paned.set_position(DEFAULT_CONTENT_PANE_POSITION);
    status_container.set_visible(settings.show_status_bar);

    let show_ap_bottom = settings.show_detail_pane || settings.show_device_pane;
    widgets.ap_root.set_position(DEFAULT_AP_ROOT_POSITION);
    widgets.ap_root.set_resize_end_child(show_ap_bottom);
    widgets.ap_bottom.set_visible(show_ap_bottom);
    widgets
        .ap_detail_notebook
        .set_visible(settings.show_detail_pane);
    widgets.ap_assoc_box.set_visible(settings.show_device_pane);
    widgets.ap_bottom.set_position(DEFAULT_AP_BOTTOM_POSITION);

    widgets
        .client_root
        .set_position(DEFAULT_CLIENT_ROOT_POSITION);
    widgets
        .client_root
        .set_resize_end_child(settings.show_detail_pane);
    widgets
        .client_detail_notebook
        .set_visible(settings.show_detail_pane);

    widgets
        .bluetooth_root
        .set_position(DEFAULT_BLUETOOTH_ROOT_POSITION);
    widgets
        .bluetooth_root
        .set_resize_end_child(settings.show_detail_pane);
    widgets
        .bluetooth_bottom
        .set_visible(settings.show_detail_pane);
    widgets
        .bluetooth_bottom
        .set_position(DEFAULT_BLUETOOTH_BOTTOM_POSITION);
}

fn apply_view_preferences(
    state: &Rc<RefCell<AppState>>,
    content_paned: &Paned,
    status_container: &GtkBox,
    widgets: &UiWidgets,
    show_status_bar: Option<bool>,
    show_detail_pane: Option<bool>,
    show_device_pane: Option<bool>,
) {
    let mut status_messages = Vec::new();
    {
        let mut s = state.borrow_mut();
        let previous_status_bar = s.settings.show_status_bar;
        let previous_detail_pane = s.settings.show_detail_pane;
        let previous_device_pane = s.settings.show_device_pane;
        let mut changed = false;

        if let Some(value) = show_status_bar {
            if s.settings.show_status_bar != value {
                s.settings.show_status_bar = value;
                changed = true;
            }
        }
        if let Some(value) = show_detail_pane {
            if s.settings.show_detail_pane != value {
                s.settings.show_detail_pane = value;
                changed = true;
            }
        }
        if let Some(value) = show_device_pane {
            if s.settings.show_device_pane != value {
                s.settings.show_device_pane = value;
                changed = true;
            }
        }

        if changed {
            apply_view_visibility(&s.settings, content_paned, status_container, widgets);
        }

        if s.settings.show_status_bar != previous_status_bar {
            status_messages.push(format!(
                "status pane {}",
                if s.settings.show_status_bar {
                    "enabled"
                } else {
                    "disabled"
                }
            ));
        }
        if s.settings.show_detail_pane != previous_detail_pane {
            status_messages.push(format!(
                "details pane {}",
                if s.settings.show_detail_pane {
                    "enabled"
                } else {
                    "disabled"
                }
            ));
        }
        if s.settings.show_device_pane != previous_device_pane {
            status_messages.push(format!(
                "device pane {}",
                if s.settings.show_device_pane {
                    "enabled"
                } else {
                    "disabled"
                }
            ));
        }

        for message in status_messages.drain(..) {
            s.push_status(message);
        }
        if changed {
            s.save_settings_to_disk();
        }
    }
}

fn sync_view_menu_action_state(app: &Application, action_name: &str, value: bool) {
    if let Some(action) = app.lookup_action(action_name) {
        if let Ok(action) = action.downcast::<gio::SimpleAction>() {
            action.set_state(&glib::Variant::from(value));
        }
    }
}

fn migrate_legacy_bluetooth_table_layout(layout: &mut TableLayout) {
    let visible_ids = layout
        .columns
        .iter()
        .filter(|column| column.visible)
        .map(|column| column.id.as_str())
        .collect::<Vec<_>>();
    let legacy_default = ["transport", "mac", "oui", "type", "first_seen", "last_seen"];
    let legacy_after_name_append = [
        "transport",
        "mac",
        "oui",
        "type",
        "first_seen",
        "last_seen",
        "name",
        "mfgr_ids",
    ];

    if visible_ids == legacy_default || visible_ids == legacy_after_name_append {
        *layout = default_bluetooth_table_layout();
    }
}

fn migrate_assoc_client_table_layout(layout: &mut TableLayout) {
    let has_current_ap = layout
        .columns
        .iter()
        .any(|column| column.id == "current_ap");
    let has_current_ssid = layout
        .columns
        .iter()
        .any(|column| column.id == "current_ssid");
    if !has_current_ap || !has_current_ssid {
        return;
    }

    if let Some(status_col) = layout
        .columns
        .iter_mut()
        .find(|column| column.id == "status")
    {
        status_col.visible = false;
    }
    if let Some(current_ap_col) = layout
        .columns
        .iter_mut()
        .find(|column| column.id == "current_ap")
    {
        current_ap_col.visible = true;
    }
    if let Some(current_ssid_col) = layout
        .columns
        .iter_mut()
        .find(|column| column.id == "current_ssid")
    {
        current_ssid_col.visible = true;
    }
}

fn prepare_and_start_wifi_capture(
    mut interfaces: Vec<InterfaceSettings>,
    session_capture_path: PathBuf,
    wifi_packet_header_mode: WifiPacketHeaderMode,
    gps_enabled: bool,
    capture_sender: Sender<CaptureEvent>,
) -> WifiStartResult {
    let mut status_lines = Vec::new();
    let mut privilege_alert = None;
    let mut wifi_interface_restore_types = HashMap::new();
    status_lines.push(format!(
        "Wi-Fi packet headers set to {}",
        match wifi_packet_header_mode {
            WifiPacketHeaderMode::Radiotap => "Radiotap",
            WifiPacketHeaderMode::Ppi => "PPI",
        }
    ));

    for iface in interfaces.iter_mut().filter(|i| i.enabled) {
        match capture::prepare_interface_for_capture(iface.clone(), true) {
            Ok(prepared) => {
                wifi_interface_restore_types.insert(
                    prepared.interface.interface_name.clone(),
                    prepared
                        .original_type
                        .unwrap_or_else(|| "managed".to_string()),
                );
                *iface = prepared.interface;
                status_lines.extend(prepared.status_lines);
            }
            Err(err) => {
                status_lines.push(format!(
                    "failed to prepare Wi-Fi interface {}: {}",
                    iface.interface_name, err
                ));
                privilege_alert = Some(format_wifi_start_failure_text(
                    &iface.interface_name,
                    &err.to_string(),
                ));
                return WifiStartResult {
                    interfaces,
                    wifi_interface_restore_types,
                    runtime: None,
                    status_lines,
                    privilege_alert,
                    started: false,
                    failed: true,
                };
            }
        }
    }

    let runtime = capture::start_capture(
        CaptureConfig {
            interfaces: interfaces.clone(),
            session_pcap_path: Some(session_capture_path),
            wifi_packet_header_mode,
            gps_enabled,
            passive_only: true,
        },
        capture_sender,
    );

    WifiStartResult {
        interfaces,
        wifi_interface_restore_types,
        runtime: Some(runtime),
        status_lines,
        privilege_alert,
        started: true,
        failed: false,
    }
}

fn format_wifi_start_failure_text(interface: &str, error_text: &str) -> String {
    let helper_hint = capture::helper_binary_hint();
    if capture::running_as_root() {
        format!(
            "WirelessExplorer could not prepare {} for Wi-Fi capture.\n\n{}\n\nWi-Fi capture was not started.\n\nWirelessExplorer is already running as root, so no additional privilege prompt was used. Verify the interface name, driver support, and that `ip`/`iw` succeeded under this root session.",
            interface, error_text
        )
    } else {
        format!(
            "WirelessExplorer could not prepare {} for Wi-Fi capture.\n\n{}\n\nWi-Fi capture was not started.\n\nRun the GUI as your normal user. One of these privilege paths must work:\n1. `pkexec` with a working polkit agent\n2. passwordless `sudo -n` for `{}`\n3. helper capabilities:\n   sudo setcap cap_net_admin,cap_net_raw=eip {}",
            interface, error_text, helper_hint, helper_hint
        )
    }
}

fn restore_wifi_interfaces(
    interfaces: &[InterfaceSettings],
    restore_types: &HashMap<String, String>,
) -> Vec<String> {
    let mut status_lines = Vec::new();

    for iface in interfaces.iter().filter(|iface| iface.enabled) {
        let restore_type = restore_types
            .get(&iface.interface_name)
            .cloned()
            .unwrap_or_else(|| "managed".to_string());
        let current_type = capture::current_interface_type(&iface.interface_name)
            .unwrap_or_else(|| "unknown".to_string());

        if current_type == restore_type {
            status_lines.push(format!(
                "{} restored to {}",
                iface.interface_name, restore_type
            ));
            continue;
        }

        match capture::set_interface_type(&iface.interface_name, &restore_type) {
            Ok(()) => status_lines.push(format!(
                "{} restored to {}",
                iface.interface_name, restore_type
            )),
            Err(err) => status_lines.push(format!(
                "failed to restore {} to {}: {}",
                iface.interface_name, restore_type, err
            )),
        }
    }

    status_lines
}

fn clear_runtime_interface_state(interfaces: &[InterfaceSettings]) -> Vec<InterfaceSettings> {
    interfaces
        .iter()
        .cloned()
        .map(|mut iface| {
            iface.monitor_interface_name = None;
            iface
        })
        .collect()
}

fn open_privilege_failure_dialog(window: &ApplicationWindow, message: &str) {
    let dialog = Dialog::builder()
        .transient_for(window)
        .modal(true)
        .title("Privilege Setup Failed")
        .default_width(760)
        .default_height(420)
        .build();
    dialog.add_button("Close", ResponseType::Close);

    let area = dialog.content_area();
    let wrapper = GtkBox::new(Orientation::Vertical, 8);

    let intro = Label::new(Some(if capture::running_as_root() {
        "WirelessExplorer could not start Wi-Fi capture because a root-level Wi-Fi command failed."
    } else {
        "WirelessExplorer could not start Wi-Fi capture because privilege escalation failed."
    }));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    wrapper.append(&intro);

    let detail = TextView::new();
    detail.set_editable(false);
    detail.set_cursor_visible(false);
    detail.set_monospace(true);
    detail.set_wrap_mode(gtk::WrapMode::WordChar);
    detail.buffer().set_text(message);

    let scrolled = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&detail)
        .build();
    wrapper.append(&scrolled);
    area.append(&wrapper);

    dialog.connect_response(|d, _| d.close());
    dialog.present();
}

fn build_capture_controls(
    window: &ApplicationWindow,
    state: Rc<RefCell<AppState>>,
) -> (GtkBox, Button, Button) {
    let bar = GtkBox::new(Orientation::Horizontal, 8);
    bar.append(&Label::new(Some("Scanning")));

    let start_btn = Button::with_label("Start");
    let stop_btn = Button::with_label("Stop");

    {
        let s = state.borrow();
        set_scan_control_button_sensitivity(
            &start_btn,
            &stop_btn,
            s.capture_runtime.is_some(),
            s.bluetooth_runtime.is_some(),
            s.scan_start_in_progress || s.scan_stop_in_progress,
        );
    }

    {
        let window = window.clone();
        let state = state.clone();
        let start_btn_handle = start_btn.clone();
        let stop_btn_handle = stop_btn.clone();
        start_btn.connect_clicked(move |_| {
            open_interface_settings_dialog_for_start(
                &window,
                state.clone(),
                start_btn_handle.clone(),
                stop_btn_handle.clone(),
            );
        });
    }

    {
        let state = state.clone();
        let start_btn_handle = start_btn.clone();
        let stop_btn_handle = stop_btn.clone();
        stop_btn.connect_clicked(move |_| {
            let mut s = state.borrow_mut();
            s.stop_scanning();
            set_scan_control_button_sensitivity(
                &start_btn_handle,
                &stop_btn_handle,
                s.capture_runtime.is_some(),
                s.bluetooth_runtime.is_some(),
                s.scan_start_in_progress || s.scan_stop_in_progress,
            );
        });
    }

    bar.append(&start_btn);
    bar.append(&stop_btn);
    (bar, start_btn, stop_btn)
}

fn detail_section_label() -> Label {
    let label = Label::new(None);
    label.set_xalign(0.0);
    label.set_yalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_selectable(true);
    label.set_hexpand(true);
    label
}

fn append_detail_section(
    parent: &GtkBox,
    title: &str,
    content: &Label,
    expanded: bool,
) -> Expander {
    let expander = Expander::builder().label(title).expanded(expanded).build();
    expander.set_hexpand(true);
    expander.set_child(Some(content));
    parent.append(&expander);
    expander
}

fn build_tabs(window: &ApplicationWindow, state: Rc<RefCell<AppState>>) -> (Notebook, UiWidgets) {
    let notebook = Notebook::new();
    let (
        ap_layout,
        client_layout,
        assoc_layout,
        bluetooth_layout,
        ap_sort,
        client_sort,
        assoc_sort,
        bluetooth_sort,
        default_rows_per_page,
    ) = {
        let s = state.borrow();
        (
            s.settings.ap_table_layout.clone(),
            s.settings.client_table_layout.clone(),
            s.settings.assoc_client_table_layout.clone(),
            s.settings.bluetooth_table_layout.clone(),
            s.ap_sort.clone(),
            s.client_sort.clone(),
            s.assoc_sort.clone(),
            s.bluetooth_sort.clone(),
            s.settings.default_rows_per_page.max(1),
        )
    };

    let ap_list = ListBox::new();
    ap_list.set_selection_mode(gtk::SelectionMode::Single);
    attach_listbox_click_selection(&ap_list);
    let ap_scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&ap_list)
        .build();
    let (ap_pagination_row, ap_pagination) = build_table_pagination_controls(
        default_rows_per_page,
        table_filter_columns(&ap_layout, ap_column_label),
    );

    let ap_header_holder = GtkBox::new(Orientation::Vertical, 0);
    ap_header_holder.append(&ap_table_header(&ap_layout, &ap_sort, state.clone()));
    ap_header_holder.append(&ap_pagination.filter_bar);
    let ap_top = GtkBox::new(Orientation::Vertical, 4);
    ap_top.append(&ap_header_holder);
    ap_top.append(&ap_scrolled);
    ap_top.append(&ap_pagination_row);

    let ap_detail_label = Label::new(None);
    ap_detail_label.set_xalign(0.0);
    ap_detail_label.set_yalign(0.0);
    ap_detail_label.set_selectable(true);
    ap_detail_label.set_wrap(true);

    let ap_notes_view = TextView::new();
    ap_notes_view.set_vexpand(true);

    let ap_packet_draw = DrawingArea::new();
    ap_packet_draw.set_content_width(300);
    ap_packet_draw.set_content_height(220);

    let save_notes_btn = Button::with_label("Save Notes");

    let ap_export_box = GtkBox::new(Orientation::Horizontal, 6);
    let ap_export_csv = Button::with_label("Export AP CSV");
    let ap_export_pcap = Button::with_label("Export AP PCAP");
    let ap_export_hs = Button::with_label("Export Handshakes PCAP");

    for b in [&ap_export_csv, &ap_export_pcap, &ap_export_hs] {
        ap_export_box.append(b);
    }

    let ap_detail_scroll = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .min_content_height(250)
        .child(&ap_detail_label)
        .build();

    let ap_packet_box = GtkBox::new(Orientation::Vertical, 6);
    ap_packet_box.append(&Label::new(Some("Packet Type Pie")));
    ap_packet_box.append(&ap_packet_draw);

    let ap_summary_row = Paned::new(Orientation::Horizontal);
    ap_summary_row.set_wide_handle(true);
    ap_summary_row.set_position(DEFAULT_AP_SUMMARY_ROW_POSITION);
    ap_summary_row.set_resize_start_child(true);
    ap_summary_row.set_resize_end_child(true);
    ap_summary_row.set_shrink_start_child(false);
    ap_summary_row.set_shrink_end_child(false);
    ap_summary_row.set_start_child(Some(&ap_detail_scroll));
    ap_summary_row.set_end_child(Some(&ap_packet_box));

    let ap_notes_scrolled = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .min_content_height(180)
        .child(&ap_notes_view)
        .build();

    let ap_notes_box = GtkBox::new(Orientation::Vertical, 6);
    ap_notes_box.append(&Label::new(Some("Notes")));
    ap_notes_box.append(&ap_notes_scrolled);
    ap_notes_box.append(&save_notes_btn);
    ap_notes_box.append(&ap_export_box);

    let ap_detail_sections = Paned::new(Orientation::Vertical);
    ap_detail_sections.set_wide_handle(true);
    ap_detail_sections.set_position(DEFAULT_AP_DETAIL_SECTIONS_POSITION);
    ap_detail_sections.set_resize_start_child(true);
    ap_detail_sections.set_resize_end_child(true);
    ap_detail_sections.set_shrink_start_child(false);
    ap_detail_sections.set_shrink_end_child(false);
    ap_detail_sections.set_start_child(Some(&ap_summary_row));
    ap_detail_sections.set_end_child(Some(&ap_notes_box));

    let ap_detail_box = GtkBox::new(Orientation::Vertical, 6);
    ap_detail_box.append(&Label::new(Some("Network Details and Packet Graphs")));
    ap_detail_box.append(&ap_detail_sections);

    let ap_selection_suppressed = Rc::new(RefCell::new(false));
    let ap_selected_key = Rc::new(RefCell::new(None::<String>));
    let ap_assoc_header_holder = GtkBox::new(Orientation::Vertical, 0);
    ap_assoc_header_holder.append(&ap_assoc_clients_header(
        &assoc_layout,
        &assoc_sort,
        state.clone(),
    ));
    let ap_assoc_list = ListBox::new();
    attach_listbox_click_selection(&ap_assoc_list);
    let ap_assoc_scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&ap_assoc_list)
        .build();
    let (ap_assoc_pagination_row, ap_assoc_pagination) = build_table_pagination_controls(
        default_rows_per_page,
        table_filter_columns(&assoc_layout, assoc_client_column_label),
    );
    ap_assoc_header_holder.append(&ap_assoc_pagination.filter_bar);

    let ap_assoc_box = GtkBox::new(Orientation::Vertical, 4);
    ap_assoc_box.append(&ap_assoc_header_holder);
    ap_assoc_box.append(&ap_assoc_scrolled);
    ap_assoc_box.append(&ap_assoc_pagination_row);

    let ap_bottom = Paned::new(Orientation::Horizontal);
    ap_bottom.set_wide_handle(true);
    ap_bottom.set_position(DEFAULT_AP_BOTTOM_POSITION);
    ap_bottom.set_resize_start_child(true);
    ap_bottom.set_resize_end_child(true);
    ap_bottom.set_shrink_start_child(false);
    ap_bottom.set_shrink_end_child(false);
    ap_bottom.set_end_child(Some(&ap_assoc_box));

    let ap_root = Paned::new(Orientation::Vertical);
    ap_root.set_wide_handle(true);
    ap_root.set_position(DEFAULT_AP_ROOT_POSITION);
    ap_root.set_resize_start_child(true);
    ap_root.set_resize_end_child(true);
    ap_root.set_shrink_start_child(false);
    ap_root.set_shrink_end_child(false);
    ap_root.set_start_child(Some(&ap_top));
    ap_root.set_end_child(Some(&ap_bottom));

    notebook.append_page(&ap_root, Some(&Label::new(Some("Access Points"))));

    let client_list = ListBox::new();
    client_list.set_selection_mode(gtk::SelectionMode::Single);
    attach_listbox_click_selection(&client_list);
    let client_selection_suppressed = Rc::new(RefCell::new(false));
    let client_selected_key = Rc::new(RefCell::new(None::<String>));
    let client_scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&client_list)
        .build();
    let (client_pagination_row, client_pagination) = build_table_pagination_controls(
        default_rows_per_page,
        table_filter_columns(&client_layout, client_column_label),
    );

    let client_header_holder = GtkBox::new(Orientation::Vertical, 0);
    client_header_holder.append(&client_table_header(
        &client_layout,
        &client_sort,
        state.clone(),
    ));
    client_header_holder.append(&client_pagination.filter_bar);
    let client_top = GtkBox::new(Orientation::Vertical, 4);
    client_top.append(&client_header_holder);
    client_top.append(&client_scrolled);
    client_top.append(&client_pagination_row);

    let client_detail_label = Label::new(None);
    client_detail_label.set_xalign(0.0);
    client_detail_label.set_yalign(0.0);
    client_detail_label.set_wrap(true);
    client_detail_label.set_selectable(true);

    let client_export_box = GtkBox::new(Orientation::Horizontal, 6);
    let client_export_csv = Button::with_label("Export Client CSV");
    let client_export_pcap = Button::with_label("Export Client PCAP");
    for b in [&client_export_csv, &client_export_pcap] {
        client_export_box.append(b);
    }

    let client_detail_box = GtkBox::new(Orientation::Vertical, 6);
    client_detail_box.append(&Label::new(Some("Client Details")));
    client_detail_box.append(&client_detail_label);
    client_detail_box.append(&client_export_box);
    let client_detail_scrolled = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .min_content_height(260)
        .child(&client_detail_box)
        .build();
    let wifi_geiger_state = Rc::new(RefCell::new(WifiGeigerUiState::default()));

    let ap_wifi_geiger_target_label = Label::new(Some(
        "Target: none selected. Select an AP to view its last RSSI or start locating it.",
    ));
    ap_wifi_geiger_target_label.set_xalign(0.0);
    ap_wifi_geiger_target_label.set_wrap(true);
    let ap_wifi_geiger_lock_label = Label::new(Some("Wi-Fi Lock: Unlocked"));
    ap_wifi_geiger_lock_label.set_xalign(0.0);
    ap_wifi_geiger_lock_label.set_wrap(true);
    let ap_wifi_geiger_rssi = Label::new(Some("RSSI: -- dBm"));
    ap_wifi_geiger_rssi.add_css_class("heading");
    ap_wifi_geiger_rssi.set_xalign(0.0);
    let ap_wifi_geiger_tone = Label::new(Some("Tone: -- Hz"));
    ap_wifi_geiger_tone.set_xalign(0.0);
    let ap_wifi_geiger_meter = DrawingArea::new();
    ap_wifi_geiger_meter.set_width_request(520);
    ap_wifi_geiger_meter.set_height_request(300);
    ap_wifi_geiger_meter.set_hexpand(true);
    ap_wifi_geiger_meter.set_vexpand(true);
    {
        let wifi_geiger_state = wifi_geiger_state.clone();
        ap_wifi_geiger_meter.set_draw_func(move |_, ctx, width, height| {
            draw_wifi_geiger_meter(
                ctx,
                width as f64,
                height as f64,
                &wifi_geiger_state.borrow(),
            );
        });
    }

    let ap_geiger_track_btn = Button::with_label("Track This AP");
    let ap_geiger_lock_btn = Button::with_label("Lock to AP");
    let ap_geiger_unlock_btn = Button::with_label("Unlock WiFi Card");
    let ap_geiger_stop_btn = Button::with_label("Stop Locate");
    let ap_geiger_button_row = GtkBox::new(Orientation::Horizontal, 8);
    for button in [
        &ap_geiger_track_btn,
        &ap_geiger_lock_btn,
        &ap_geiger_unlock_btn,
        &ap_geiger_stop_btn,
    ] {
        ap_geiger_button_row.append(button);
    }
    let ap_geiger_box = GtkBox::new(Orientation::Vertical, 8);
    ap_geiger_box.append(&ap_wifi_geiger_target_label);
    ap_geiger_box.append(&ap_wifi_geiger_lock_label);
    ap_geiger_box.append(&ap_wifi_geiger_meter);
    ap_geiger_box.append(&ap_wifi_geiger_rssi);
    ap_geiger_box.append(&ap_wifi_geiger_tone);
    ap_geiger_box.append(&ap_geiger_button_row);
    let ap_geiger_scrolled = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .min_content_height(260)
        .child(&ap_geiger_box)
        .build();

    let ap_detail_notebook = Notebook::new();
    ap_detail_notebook.append_page(&ap_detail_box, Some(&Label::new(Some("Details"))));
    ap_detail_notebook.append_page(
        &ap_geiger_scrolled,
        Some(&Label::new(Some("RSSI Geiger Counter"))),
    );
    ap_bottom.set_start_child(Some(&ap_detail_notebook));

    let client_wifi_geiger_target_label = Label::new(Some(
        "Target: none selected. Select a client to view its last RSSI or start locating it.",
    ));
    client_wifi_geiger_target_label.set_xalign(0.0);
    client_wifi_geiger_target_label.set_wrap(true);
    let client_wifi_geiger_lock_label = Label::new(Some("Wi-Fi Lock: Unlocked"));
    client_wifi_geiger_lock_label.set_xalign(0.0);
    client_wifi_geiger_lock_label.set_wrap(true);
    let client_wifi_geiger_rssi = Label::new(Some("RSSI: -- dBm"));
    client_wifi_geiger_rssi.add_css_class("heading");
    client_wifi_geiger_rssi.set_xalign(0.0);
    let client_wifi_geiger_tone = Label::new(Some("Tone: -- Hz"));
    client_wifi_geiger_tone.set_xalign(0.0);
    let client_wifi_geiger_meter = DrawingArea::new();
    client_wifi_geiger_meter.set_width_request(520);
    client_wifi_geiger_meter.set_height_request(300);
    client_wifi_geiger_meter.set_hexpand(true);
    client_wifi_geiger_meter.set_vexpand(true);
    {
        let wifi_geiger_state = wifi_geiger_state.clone();
        client_wifi_geiger_meter.set_draw_func(move |_, ctx, width, height| {
            draw_wifi_geiger_meter(
                ctx,
                width as f64,
                height as f64,
                &wifi_geiger_state.borrow(),
            );
        });
    }

    {
        let wifi_geiger_state = wifi_geiger_state.clone();
        let ap_wifi_geiger_meter = ap_wifi_geiger_meter.clone();
        let client_wifi_geiger_meter = client_wifi_geiger_meter.clone();
        glib::timeout_add_local(Duration::from_millis(33), move || {
            let mut geiger = wifi_geiger_state.borrow_mut();
            let now = Instant::now();
            let dt = geiger
                .last_animation_at
                .map(|last| now.saturating_duration_since(last).as_secs_f64())
                .unwrap_or(1.0 / 30.0);
            geiger.last_animation_at = Some(now);

            let delta = geiger.target_fraction - geiger.needle_fraction;
            let max_step = (dt * 1.8).max(0.02);
            let moving = delta.abs() > 0.001;
            if moving {
                if delta.abs() <= max_step {
                    geiger.needle_fraction = geiger.target_fraction;
                } else {
                    geiger.needle_fraction += max_step * delta.signum();
                }
            }

            let pulse_active = geiger
                .last_update_at
                .map(|last| now.saturating_duration_since(last).as_secs_f64() < 0.55)
                .unwrap_or(false);

            if geiger.target.is_some() && (moving || pulse_active) {
                ap_wifi_geiger_meter.queue_draw();
                client_wifi_geiger_meter.queue_draw();
            }

            glib::ControlFlow::Continue
        });
    }

    let client_geiger_track_btn = Button::with_label("Track This Client");
    let client_geiger_lock_btn = Button::with_label("Lock to AP");
    let client_geiger_unlock_btn = Button::with_label("Unlock WiFi Card");
    let client_geiger_stop_btn = Button::with_label("Stop Locate");
    let client_geiger_button_row = GtkBox::new(Orientation::Horizontal, 8);
    for button in [
        &client_geiger_track_btn,
        &client_geiger_lock_btn,
        &client_geiger_unlock_btn,
        &client_geiger_stop_btn,
    ] {
        client_geiger_button_row.append(button);
    }
    let client_geiger_box = GtkBox::new(Orientation::Vertical, 8);
    client_geiger_box.append(&client_wifi_geiger_target_label);
    client_geiger_box.append(&client_wifi_geiger_lock_label);
    client_geiger_box.append(&client_wifi_geiger_meter);
    client_geiger_box.append(&client_wifi_geiger_rssi);
    client_geiger_box.append(&client_wifi_geiger_tone);
    client_geiger_box.append(&client_geiger_button_row);
    let client_geiger_scrolled = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .min_content_height(260)
        .child(&client_geiger_box)
        .build();

    let client_detail_notebook = Notebook::new();
    client_detail_notebook.append_page(&client_detail_scrolled, Some(&Label::new(Some("Details"))));
    client_detail_notebook.append_page(
        &client_geiger_scrolled,
        Some(&Label::new(Some("RSSI Geiger Counter"))),
    );

    let client_root = Paned::new(Orientation::Vertical);
    client_root.set_wide_handle(true);
    client_root.set_position(DEFAULT_CLIENT_ROOT_POSITION);
    client_root.set_resize_start_child(true);
    client_root.set_resize_end_child(true);
    client_root.set_shrink_start_child(false);
    client_root.set_shrink_end_child(false);
    client_root.set_start_child(Some(&client_top));
    client_root.set_end_child(Some(&client_detail_notebook));

    notebook.append_page(&client_root, Some(&Label::new(Some("Clients"))));

    let bluetooth_geiger_state = Rc::new(RefCell::new(BluetoothGeigerUiState::default()));

    let bluetooth_list = ListBox::new();
    bluetooth_list.set_selection_mode(gtk::SelectionMode::Single);
    attach_listbox_click_selection(&bluetooth_list);
    let bluetooth_selection_suppressed = Rc::new(RefCell::new(false));
    let bluetooth_selected_key = Rc::new(RefCell::new(None::<String>));
    let bluetooth_header_holder = GtkBox::new(Orientation::Vertical, 0);
    bluetooth_header_holder.append(&bluetooth_table_header(
        &bluetooth_layout,
        &bluetooth_sort,
        state.clone(),
    ));
    let bluetooth_scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&bluetooth_list)
        .build();
    let (bluetooth_pagination_row, bluetooth_pagination) = build_table_pagination_controls(
        default_rows_per_page,
        table_filter_columns(&bluetooth_layout, bluetooth_column_label),
    );
    bluetooth_header_holder.append(&bluetooth_pagination.filter_bar);
    let bluetooth_top = GtkBox::new(Orientation::Vertical, 4);
    bluetooth_top.append(&bluetooth_header_holder);
    bluetooth_top.append(&bluetooth_scrolled);
    bluetooth_top.append(&bluetooth_pagination_row);

    let bluetooth_identity_label = detail_section_label();
    let bluetooth_passive_label = detail_section_label();
    let bluetooth_active_summary_label = detail_section_label();
    let bluetooth_readable_label = detail_section_label();
    let bluetooth_services_label = detail_section_label();
    let bluetooth_characteristics_label = detail_section_label();
    let bluetooth_descriptors_label = detail_section_label();
    let bluetooth_detail_view = state.borrow().settings.bluetooth_detail_view.clone();
    let bluetooth_enumerate_btn = Button::with_label("Connect & Enumerate");
    let bluetooth_disconnect_btn = Button::with_label("Disconnect");

    let bluetooth_geiger_rssi = Label::new(Some("RSSI: -- dBm"));
    bluetooth_geiger_rssi.set_xalign(0.0);
    let bluetooth_geiger_tone = Label::new(Some("Tone: -- Hz"));
    bluetooth_geiger_tone.set_xalign(0.0);
    let bluetooth_geiger_progress = ProgressBar::new();
    bluetooth_geiger_progress.set_show_text(true);
    bluetooth_geiger_progress.set_text(Some("No target"));
    let bluetooth_geiger_track = Button::with_label("Locate Device");
    let bluetooth_geiger_stop = Button::with_label("Stop Locate");

    let bluetooth_geiger_box = GtkBox::new(Orientation::Vertical, 6);
    bluetooth_geiger_box.append(&Label::new(Some("Geiger Tracker")));
    bluetooth_geiger_box.append(&bluetooth_geiger_rssi);
    bluetooth_geiger_box.append(&bluetooth_geiger_tone);
    bluetooth_geiger_box.append(&bluetooth_geiger_progress);
    bluetooth_geiger_box.append(&bluetooth_geiger_track);
    bluetooth_geiger_box.append(&bluetooth_geiger_stop);
    let bluetooth_geiger_scrolled = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .min_content_height(220)
        .child(&bluetooth_geiger_box)
        .build();

    let bluetooth_detail_box = GtkBox::new(Orientation::Vertical, 6);
    bluetooth_detail_box.append(&Label::new(Some("Bluetooth Details")));
    let bluetooth_detail_actions = GtkBox::new(Orientation::Horizontal, 6);
    bluetooth_detail_actions.append(&bluetooth_enumerate_btn);
    bluetooth_detail_actions.append(&bluetooth_disconnect_btn);
    bluetooth_detail_box.append(&bluetooth_detail_actions);
    let bluetooth_identity_expander = append_detail_section(
        &bluetooth_detail_box,
        "Identity",
        &bluetooth_identity_label,
        bluetooth_detail_view.identity_expanded,
    );
    let bluetooth_passive_expander = append_detail_section(
        &bluetooth_detail_box,
        "Passive Broadcast Data",
        &bluetooth_passive_label,
        bluetooth_detail_view.passive_expanded,
    );
    let bluetooth_active_summary_expander = append_detail_section(
        &bluetooth_detail_box,
        "Active Enumeration Summary",
        &bluetooth_active_summary_label,
        bluetooth_detail_view.active_summary_expanded,
    );
    let bluetooth_readable_expander = append_detail_section(
        &bluetooth_detail_box,
        "Readable Attributes",
        &bluetooth_readable_label,
        bluetooth_detail_view.readable_expanded,
    );
    let bluetooth_services_expander = append_detail_section(
        &bluetooth_detail_box,
        "Services",
        &bluetooth_services_label,
        bluetooth_detail_view.services_expanded,
    );
    let bluetooth_characteristics_expander = append_detail_section(
        &bluetooth_detail_box,
        "Characteristics",
        &bluetooth_characteristics_label,
        bluetooth_detail_view.characteristics_expanded,
    );
    let bluetooth_descriptors_expander = append_detail_section(
        &bluetooth_detail_box,
        "Descriptors",
        &bluetooth_descriptors_label,
        bluetooth_detail_view.descriptors_expanded,
    );
    let bluetooth_detail_scrolled = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .min_content_height(220)
        .child(&bluetooth_detail_box)
        .build();

    let bluetooth_bottom = Paned::new(Orientation::Horizontal);
    bluetooth_bottom.set_wide_handle(true);
    bluetooth_bottom.set_position(DEFAULT_BLUETOOTH_BOTTOM_POSITION);
    bluetooth_bottom.set_resize_start_child(true);
    bluetooth_bottom.set_resize_end_child(true);
    bluetooth_bottom.set_shrink_start_child(false);
    bluetooth_bottom.set_shrink_end_child(false);
    bluetooth_bottom.set_start_child(Some(&bluetooth_geiger_scrolled));
    bluetooth_bottom.set_end_child(Some(&bluetooth_detail_scrolled));

    let bluetooth_root = Paned::new(Orientation::Vertical);
    bluetooth_root.set_wide_handle(true);
    bluetooth_root.set_position(DEFAULT_BLUETOOTH_ROOT_POSITION);
    bluetooth_root.set_resize_start_child(true);
    bluetooth_root.set_resize_end_child(true);
    bluetooth_root.set_shrink_start_child(false);
    bluetooth_root.set_shrink_end_child(false);
    bluetooth_root.set_start_child(Some(&bluetooth_top));
    bluetooth_root.set_end_child(Some(&bluetooth_bottom));

    {
        let state = state.clone();
        bluetooth_identity_expander.connect_expanded_notify(move |expander| {
            let mut s = state.borrow_mut();
            s.settings.bluetooth_detail_view.identity_expanded = expander.is_expanded();
            s.save_settings_to_disk();
        });
    }
    {
        let state = state.clone();
        bluetooth_passive_expander.connect_expanded_notify(move |expander| {
            let mut s = state.borrow_mut();
            s.settings.bluetooth_detail_view.passive_expanded = expander.is_expanded();
            s.save_settings_to_disk();
        });
    }
    {
        let state = state.clone();
        bluetooth_active_summary_expander.connect_expanded_notify(move |expander| {
            let mut s = state.borrow_mut();
            s.settings.bluetooth_detail_view.active_summary_expanded = expander.is_expanded();
            s.save_settings_to_disk();
        });
    }
    {
        let state = state.clone();
        bluetooth_readable_expander.connect_expanded_notify(move |expander| {
            let mut s = state.borrow_mut();
            s.settings.bluetooth_detail_view.readable_expanded = expander.is_expanded();
            s.save_settings_to_disk();
        });
    }
    {
        let state = state.clone();
        bluetooth_services_expander.connect_expanded_notify(move |expander| {
            let mut s = state.borrow_mut();
            s.settings.bluetooth_detail_view.services_expanded = expander.is_expanded();
            s.save_settings_to_disk();
        });
    }
    {
        let state = state.clone();
        bluetooth_characteristics_expander.connect_expanded_notify(move |expander| {
            let mut s = state.borrow_mut();
            s.settings.bluetooth_detail_view.characteristics_expanded = expander.is_expanded();
            s.save_settings_to_disk();
        });
    }
    {
        let state = state.clone();
        bluetooth_descriptors_expander.connect_expanded_notify(move |expander| {
            let mut s = state.borrow_mut();
            s.settings.bluetooth_detail_view.descriptors_expanded = expander.is_expanded();
            s.save_settings_to_disk();
        });
    }
    notebook.append_page(&bluetooth_root, Some(&Label::new(Some("Bluetooth"))));

    let channel_band_combo = ComboBoxText::new();
    channel_band_combo.append(Some("all"), "All Bands");
    channel_band_combo.append(Some("2.4"), "2.4 GHz");
    channel_band_combo.append(Some("5"), "5 GHz");
    channel_band_combo.append(Some("6"), "6 GHz");
    channel_band_combo.set_active_id(Some("all"));

    let channel_draw = DrawingArea::new();
    channel_draw.set_content_width(1200);
    channel_draw.set_content_height(380);
    channel_draw.set_hexpand(true);
    channel_draw.set_vexpand(true);

    let channel_controls = GtkBox::new(Orientation::Vertical, 6);
    channel_controls.append(&Label::new(Some("Spectrum Filter")));
    channel_controls.append(&channel_band_combo);
    channel_controls.append(&channel_draw);

    let status_label = Label::new(Some("starting"));
    status_label.set_xalign(0.0);
    status_label.set_wrap(true);
    status_label.set_selectable(true);

    let gps_status_label = Label::new(Some("GPS status initializing"));
    gps_status_label.set_xalign(0.0);
    gps_status_label.set_wrap(true);
    gps_status_label.set_selectable(true);

    let channel_status_box = GtkBox::new(Orientation::Vertical, 6);
    channel_status_box.append(&Label::new(Some("Status")));
    channel_status_box.append(&status_label);
    channel_status_box.append(&Label::new(Some("GPS Status")));
    channel_status_box.append(&gps_status_label);
    let channel_status_scrolled = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .min_content_height(170)
        .child(&channel_status_box)
        .build();

    let channel_root = Paned::new(Orientation::Vertical);
    channel_root.set_wide_handle(true);
    channel_root.set_position(DEFAULT_CHANNEL_ROOT_POSITION);
    channel_root.set_resize_start_child(true);
    channel_root.set_resize_end_child(true);
    channel_root.set_shrink_start_child(false);
    channel_root.set_shrink_end_child(false);
    channel_root.set_start_child(Some(&channel_controls));
    channel_root.set_end_child(Some(&channel_status_scrolled));
    notebook.append_page(&channel_root, Some(&Label::new(Some("Channel Usage"))));

    let sdr_model = Rc::new(RefCell::new(SdrUiModel {
        current_freq_hz: SdrConfig::default().center_freq_hz,
        sample_rate_hz: SdrConfig::default().sample_rate_hz,
        squelch_dbm: SdrConfig::default().squelch_dbm,
        ..SdrUiModel::default()
    }));
    let sdr_frequency_label = Label::new(Some(
        "Center: 433920000 Hz | Sample Rate: 2400000 Hz | Sweep: active",
    ));
    sdr_frequency_label.set_xalign(0.0);
    let sdr_decoder_label = Label::new(Some("Decoder: idle"));
    sdr_decoder_label.set_xalign(0.0);
    let sdr_dependency_label = Label::new(Some("Dependencies: not checked"));
    sdr_dependency_label.set_xalign(0.0);
    sdr_dependency_label.set_wrap(true);

    let sdr_hardware_combo = ComboBoxText::new();
    for hardware in [
        SdrHardware::RtlSdr,
        SdrHardware::HackRf,
        SdrHardware::BladeRf,
        SdrHardware::EttusB210,
    ] {
        sdr_hardware_combo.append(Some(hardware.id()), hardware.label());
    }
    sdr_hardware_combo.set_active_id(Some(SdrHardware::default().id()));

    let sdr_center_freq_entry = Entry::new();
    sdr_center_freq_entry.set_width_chars(14);
    sdr_center_freq_entry.set_text(&SdrConfig::default().center_freq_hz.to_string());
    let sdr_sample_rate_entry = Entry::new();
    sdr_sample_rate_entry.set_width_chars(10);
    sdr_sample_rate_entry.set_text(&SdrConfig::default().sample_rate_hz.to_string());
    let sdr_set_frequency_btn = Button::with_label("Set Center");
    let sdr_start_btn = Button::with_label("Start SDR");
    let sdr_stop_btn = Button::with_label("Stop SDR");
    let sdr_pause_check = CheckButton::with_label("Pause Sweep");

    let sdr_decoder_combo = ComboBoxText::new();
    let sdr_builtin_decoders = sdr::builtin_decoders_in_priority_order();
    let plugin_path = sdr::default_plugin_config_path();
    let plugin_decoders = sdr::load_plugin_definitions(plugin_path.as_deref())
        .into_iter()
        .map(|plugin| SdrDecoderKind::Plugin {
            id: plugin.id,
            label: plugin.label,
            command_template: plugin.command_template,
            protocol: plugin.protocol,
        })
        .collect::<Vec<_>>();
    let mut sdr_decoders = Vec::with_capacity(sdr_builtin_decoders.len() + plugin_decoders.len());
    sdr_decoders.extend(sdr_builtin_decoders);
    sdr_decoders.extend(plugin_decoders);
    let sdr_decoder_lookup: Rc<RefCell<HashMap<String, SdrDecoderKind>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let sdr_decoder_order: Rc<Vec<String>> =
        Rc::new(sdr_decoders.iter().map(|decoder| decoder.id()).collect());
    for decoder in &sdr_decoders {
        let id = decoder.id();
        let label = decoder.label();
        sdr_decoder_combo.append(Some(&id), &label);
        sdr_decoder_lookup
            .borrow_mut()
            .insert(id.to_string(), decoder.clone());
    }
    sdr_decoder_combo.set_active(Some(0));

    let sdr_decode_start_btn = Button::with_label("Start Decode");
    let sdr_decode_stop_btn = Button::with_label("Stop Decode");
    let sdr_dep_refresh_btn = Button::with_label("Refresh Dependencies");
    let sdr_dep_install_btn = Button::with_label("Install Missing Dependencies");

    let sdr_log_enable_check = CheckButton::with_label("Log decoder output");
    let sdr_log_dir_entry = Entry::new();
    sdr_log_dir_entry.set_width_chars(42);
    sdr_log_dir_entry.set_text(
        SdrConfig::default()
            .log_output_dir
            .to_string_lossy()
            .as_ref(),
    );
    let sdr_scan_enable_check = CheckButton::with_label("Scan Range");
    sdr_scan_enable_check.set_active(SdrConfig::default().scan_range_enabled);
    let sdr_scan_start_entry = Entry::new();
    sdr_scan_start_entry.set_width_chars(12);
    sdr_scan_start_entry.set_text(&SdrConfig::default().scan_start_hz.to_string());
    let sdr_scan_end_entry = Entry::new();
    sdr_scan_end_entry.set_width_chars(12);
    sdr_scan_end_entry.set_text(&SdrConfig::default().scan_end_hz.to_string());
    let sdr_scan_step_entry = Entry::new();
    sdr_scan_step_entry.set_width_chars(10);
    sdr_scan_step_entry.set_text(&SdrConfig::default().scan_step_hz.to_string());
    let sdr_scan_speed_entry = Entry::new();
    sdr_scan_speed_entry.set_width_chars(6);
    sdr_scan_speed_entry.set_text(&format!("{:.2}", SdrConfig::default().scan_steps_per_sec));
    let sdr_squelch_scale = gtk::Scale::with_range(Orientation::Horizontal, -130.0, -10.0, 1.0);
    sdr_squelch_scale.set_hexpand(true);
    sdr_squelch_scale.set_value(SdrConfig::default().squelch_dbm as f64);
    let sdr_squelch_value_label = Label::new(Some(&format!(
        "{:.0} dBm",
        SdrConfig::default().squelch_dbm
    )));
    sdr_squelch_value_label.set_xalign(0.0);
    let sdr_autotune_check = CheckButton::with_label("Auto-tune decoders");
    sdr_autotune_check.set_active(SdrConfig::default().auto_tune_decoders);
    let sdr_bias_tee_check = CheckButton::with_label("Bias-Tee / Antenna Power");
    sdr_bias_tee_check.set_active(SdrConfig::default().bias_tee_enabled);
    let sdr_no_payload_satcom_check = CheckButton::with_label("No payload for satcom decoders");
    sdr_no_payload_satcom_check.set_active(SdrConfig::default().no_payload_satcom);
    let sdr_sample_duration_spin = SpinButton::with_range(1.0, 600.0, 1.0);
    sdr_sample_duration_spin.set_value(10.0);
    let sdr_sample_dir_entry = Entry::new();
    sdr_sample_dir_entry.set_width_chars(42);
    sdr_sample_dir_entry.set_text(
        SdrConfig::default()
            .log_output_dir
            .join("iq_samples")
            .to_string_lossy()
            .as_ref(),
    );
    let sdr_capture_sample_btn = Button::with_label("Capture IQ Sample");
    let sdr_export_map_btn = Button::with_label("Export Map JSON");
    let sdr_export_satcom_btn = Button::with_label("Export Satcom JSON");

    let mut initial_sdr_bookmarks = vec![
        ("ADS-B (1090 MHz)".to_string(), 1_090_000_000),
        ("ACARS (131.550 MHz)".to_string(), 131_550_000),
        ("AIS Ch A (161.975 MHz)".to_string(), 161_975_000),
        ("AIS Ch B (162.025 MHz)".to_string(), 162_025_000),
        ("POCSAG (929.6125 MHz)".to_string(), 929_612_500),
        ("NOAA WX 162.550".to_string(), 162_550_000),
        ("APRS 144.390".to_string(), 144_390_000),
        ("Iridium 1626 MHz".to_string(), 1_626_000_000),
        ("Inmarsat Aero 1545.000".to_string(), 1_545_000_000),
        ("DAB Block 11D 222.064".to_string(), 222_064_000),
        ("VOR 113.000".to_string(), 113_000_000),
    ];
    initial_sdr_bookmarks.extend(load_gqrx_bookmarks());
    initial_sdr_bookmarks.sort_by_key(|(_, freq)| *freq);
    initial_sdr_bookmarks.dedup_by(|left, right| left.1 == right.1);
    let sdr_bookmarks: Rc<RefCell<Vec<(String, u64)>>> =
        Rc::new(RefCell::new(initial_sdr_bookmarks));
    let sdr_bookmark_combo = ComboBoxText::new();
    for (label, freq) in sdr_bookmarks.borrow().iter() {
        sdr_bookmark_combo.append(Some(&freq.to_string()), label);
    }
    sdr_bookmark_combo.set_active(Some(0));
    let sdr_bookmark_add_btn = Button::with_label("Add Bookmark");
    let sdr_bookmark_jump_btn = Button::with_label("Jump");

    let sdr_controls = Grid::new();
    sdr_controls.set_column_spacing(10);
    sdr_controls.set_row_spacing(6);
    sdr_controls.attach(&Label::new(Some("Hardware")), 0, 0, 1, 1);
    sdr_controls.attach(&sdr_hardware_combo, 1, 0, 1, 1);
    sdr_controls.attach(&Label::new(Some("Center (Hz)")), 2, 0, 1, 1);
    sdr_controls.attach(&sdr_center_freq_entry, 3, 0, 1, 1);
    sdr_controls.attach(&Label::new(Some("Sample Rate")), 4, 0, 1, 1);
    sdr_controls.attach(&sdr_sample_rate_entry, 5, 0, 1, 1);
    sdr_controls.attach(&sdr_set_frequency_btn, 6, 0, 1, 1);
    sdr_controls.attach(&sdr_start_btn, 7, 0, 1, 1);
    sdr_controls.attach(&sdr_stop_btn, 8, 0, 1, 1);
    sdr_controls.attach(&sdr_pause_check, 9, 0, 1, 1);
    sdr_controls.attach(&Label::new(Some("Decode")), 0, 1, 1, 1);
    sdr_controls.attach(&sdr_decoder_combo, 1, 1, 2, 1);
    sdr_controls.attach(&sdr_decode_start_btn, 3, 1, 1, 1);
    sdr_controls.attach(&sdr_decode_stop_btn, 4, 1, 1, 1);
    sdr_controls.attach(&sdr_dep_refresh_btn, 5, 1, 2, 1);
    sdr_controls.attach(&sdr_dep_install_btn, 7, 1, 3, 1);
    sdr_controls.attach(&sdr_log_enable_check, 0, 2, 2, 1);
    sdr_controls.attach(&Label::new(Some("Log Dir")), 2, 2, 1, 1);
    sdr_controls.attach(&sdr_log_dir_entry, 3, 2, 7, 1);
    sdr_controls.attach(&sdr_scan_enable_check, 0, 3, 2, 1);
    sdr_controls.attach(&Label::new(Some("Scan Start")), 2, 3, 1, 1);
    sdr_controls.attach(&sdr_scan_start_entry, 3, 3, 1, 1);
    sdr_controls.attach(&Label::new(Some("Scan End")), 4, 3, 1, 1);
    sdr_controls.attach(&sdr_scan_end_entry, 5, 3, 1, 1);
    sdr_controls.attach(&Label::new(Some("Step (Hz)")), 6, 3, 1, 1);
    sdr_controls.attach(&sdr_scan_step_entry, 7, 3, 1, 1);
    sdr_controls.attach(&Label::new(Some("Steps/s")), 8, 3, 1, 1);
    sdr_controls.attach(&sdr_scan_speed_entry, 9, 3, 1, 1);
    sdr_controls.attach(&Label::new(Some("Squelch")), 0, 4, 1, 1);
    sdr_controls.attach(&sdr_squelch_scale, 1, 4, 7, 1);
    sdr_controls.attach(&sdr_squelch_value_label, 8, 4, 2, 1);
    sdr_controls.attach(&sdr_autotune_check, 0, 5, 2, 1);
    sdr_controls.attach(&sdr_bias_tee_check, 2, 5, 3, 1);
    sdr_controls.attach(&sdr_no_payload_satcom_check, 5, 5, 3, 1);
    sdr_controls.attach(&Label::new(Some("Bookmarks")), 0, 6, 1, 1);
    sdr_controls.attach(&sdr_bookmark_combo, 1, 6, 2, 1);
    sdr_controls.attach(&sdr_bookmark_jump_btn, 3, 6, 1, 1);
    sdr_controls.attach(&sdr_bookmark_add_btn, 4, 6, 1, 1);
    sdr_controls.attach(&Label::new(Some("Sample (s)")), 5, 6, 1, 1);
    sdr_controls.attach(&sdr_sample_duration_spin, 6, 6, 1, 1);
    sdr_controls.attach(&Label::new(Some("IQ Dir")), 7, 6, 1, 1);
    sdr_controls.attach(&sdr_sample_dir_entry, 8, 6, 2, 1);
    sdr_controls.attach(&sdr_capture_sample_btn, 10, 6, 1, 1);
    sdr_controls.attach(&sdr_export_map_btn, 9, 7, 1, 1);
    sdr_controls.attach(&sdr_export_satcom_btn, 10, 7, 1, 1);

    let sdr_spectrogram_draw = DrawingArea::new();
    sdr_spectrogram_draw.set_content_width(1200);
    sdr_spectrogram_draw.set_content_height(230);
    sdr_spectrogram_draw.set_hexpand(true);
    sdr_spectrogram_draw.set_vexpand(true);
    {
        let sdr_model = sdr_model.clone();
        sdr_spectrogram_draw.set_draw_func(move |_, ctx, width, height| {
            draw_sdr_spectrogram(ctx, width as f64, height as f64, &sdr_model.borrow());
        });
    }

    let sdr_fft_draw = DrawingArea::new();
    sdr_fft_draw.set_content_width(1200);
    sdr_fft_draw.set_content_height(220);
    sdr_fft_draw.set_hexpand(true);
    sdr_fft_draw.set_vexpand(true);
    {
        let sdr_model = sdr_model.clone();
        sdr_fft_draw.set_draw_func(move |_, ctx, width, height| {
            draw_sdr_fft(ctx, width as f64, height as f64, &sdr_model.borrow());
        });
    }

    let sdr_map_draw = DrawingArea::new();
    sdr_map_draw.set_content_width(1200);
    sdr_map_draw.set_content_height(200);
    sdr_map_draw.set_hexpand(true);
    sdr_map_draw.set_vexpand(false);
    {
        let sdr_model = sdr_model.clone();
        sdr_map_draw.set_draw_func(move |_, ctx, width, height| {
            draw_sdr_map(ctx, width as f64, height as f64, &sdr_model.borrow());
        });
    }

    let sdr_decode_list = ListBox::new();
    sdr_decode_list.set_selection_mode(gtk::SelectionMode::None);
    let sdr_decode_scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&sdr_decode_list)
        .build();

    let (sdr_decode_pagination_row, sdr_decode_pagination) = build_table_pagination_controls(
        default_rows_per_page,
        vec![
            ("time".to_string(), "Time".to_string(), 20),
            ("decoder".to_string(), "Decoder".to_string(), 14),
            ("freq".to_string(), "Freq".to_string(), 13),
            ("protocol".to_string(), "Protocol".to_string(), 14),
            ("message".to_string(), "Message".to_string(), 50),
            ("raw".to_string(), "Raw".to_string(), 50),
        ],
    );
    let sdr_decode_header_holder = GtkBox::new(Orientation::Vertical, 0);
    sdr_decode_header_holder.append(&sdr_decode_table_header());
    sdr_decode_header_holder.append(&sdr_decode_pagination.filter_bar);

    let sdr_decode_box = GtkBox::new(Orientation::Vertical, 4);
    sdr_decode_box.append(&sdr_decode_header_holder);
    sdr_decode_box.append(&sdr_decode_scrolled);
    sdr_decode_box.append(&sdr_decode_pagination_row);

    let sdr_satcom_list = ListBox::new();
    sdr_satcom_list.set_selection_mode(gtk::SelectionMode::None);
    let sdr_satcom_scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&sdr_satcom_list)
        .build();
    let (sdr_satcom_pagination_row, sdr_satcom_pagination) = build_table_pagination_controls(
        default_rows_per_page,
        vec![
            ("time".to_string(), "Time".to_string(), 20),
            ("decoder".to_string(), "Decoder".to_string(), 14),
            ("protocol".to_string(), "Protocol".to_string(), 14),
            ("freq".to_string(), "Freq".to_string(), 13),
            ("band".to_string(), "Band".to_string(), 14),
            ("posture".to_string(), "Encryption".to_string(), 12),
            ("coords".to_string(), "Coords".to_string(), 8),
            ("identifiers".to_string(), "Identifiers".to_string(), 20),
            ("summary".to_string(), "Summary".to_string(), 40),
        ],
    );
    let sdr_satcom_header_holder = GtkBox::new(Orientation::Vertical, 0);
    sdr_satcom_header_holder.append(&sdr_satcom_table_header());
    sdr_satcom_header_holder.append(&sdr_satcom_pagination.filter_bar);
    let sdr_satcom_box = GtkBox::new(Orientation::Vertical, 4);
    sdr_satcom_box.append(&sdr_satcom_header_holder);
    sdr_satcom_box.append(&sdr_satcom_scrolled);
    sdr_satcom_box.append(&sdr_satcom_pagination_row);

    let sdr_output_notebook = Notebook::new();
    sdr_output_notebook.append_page(&sdr_decode_box, Some(&Label::new(Some("Decode Output"))));
    sdr_output_notebook.append_page(&sdr_satcom_box, Some(&Label::new(Some("Satcom Audit"))));

    let sdr_top = GtkBox::new(Orientation::Vertical, 6);
    sdr_top.append(&sdr_controls);
    sdr_top.append(&sdr_frequency_label);
    sdr_top.append(&sdr_decoder_label);
    sdr_top.append(&sdr_dependency_label);
    sdr_top.append(&Label::new(Some("Spectrogram")));
    sdr_top.append(&sdr_spectrogram_draw);
    sdr_top.append(&Label::new(Some("FFT")));
    sdr_top.append(&sdr_fft_draw);
    sdr_top.append(&Label::new(Some("Map (decoded coordinates)")));
    sdr_top.append(&sdr_map_draw);

    let sdr_root = Paned::new(Orientation::Vertical);
    sdr_root.set_wide_handle(true);
    sdr_root.set_position(DEFAULT_SDR_ROOT_POSITION);
    sdr_root.set_resize_start_child(true);
    sdr_root.set_resize_end_child(true);
    sdr_root.set_shrink_start_child(false);
    sdr_root.set_shrink_end_child(false);
    sdr_root.set_start_child(Some(&sdr_top));
    sdr_root.set_end_child(Some(&sdr_output_notebook));
    notebook.append_page(&sdr_root, Some(&Label::new(Some("SDR"))));

    let selected_packet_mix: Rc<RefCell<PacketTypeBreakdown>> =
        Rc::new(RefCell::new(PacketTypeBreakdown::default()));
    {
        let mix = selected_packet_mix.clone();
        ap_packet_draw.set_draw_func(move |_, ctx, width, height| {
            draw_packet_pie(ctx, width as f64, height as f64, &mix.borrow());
        });
    }

    {
        let ap_selection_suppressed = ap_selection_suppressed.clone();
        let ap_selected_key = ap_selected_key.clone();
        ap_list.connect_row_selected(move |_, row| {
            if *ap_selection_suppressed.borrow() {
                return;
            }
            *ap_selected_key.borrow_mut() = row.map(|entry| entry.widget_name().to_string());
        });
    }

    {
        let state = state.clone();
        let ap_list = ap_list.clone();
        let ap_notes_view = ap_notes_view.clone();
        save_notes_btn.connect_clicked(move |_| {
            let Some(row) = ap_list.selected_row() else {
                return;
            };
            let key = row.widget_name().to_string();

            let mut state = state.borrow_mut();
            let Some(ap) = state.access_points.iter_mut().find(|ap| ap.bssid == key) else {
                return;
            };

            let buffer = ap_notes_view.buffer();
            let start = buffer.start_iter();
            let end = buffer.end_iter();
            let notes = buffer.text(&start, &end, true).to_string();
            ap.notes = if notes.trim().is_empty() {
                None
            } else {
                Some(notes)
            };
            let ap_clone = ap.clone();
            let _ = state.storage.upsert_access_point(&ap_clone);
            state.push_status("saved AP notes".to_string());
        });
    }

    {
        let client_selection_suppressed = client_selection_suppressed.clone();
        let client_selected_key = client_selected_key.clone();
        client_list.connect_row_selected(move |_, row| {
            if *client_selection_suppressed.borrow() {
                return;
            }
            *client_selected_key.borrow_mut() = row.map(|entry| entry.widget_name().to_string());
        });
    }

    {
        let state = state.clone();
        let ap_list = ap_list.clone();
        let wifi_geiger_state = wifi_geiger_state.clone();
        let ap_detail_notebook = ap_detail_notebook.clone();
        ap_geiger_track_btn.connect_clicked(move |_| {
            if let Some(ap) = selected_ap(&state, &ap_list) {
                start_wifi_geiger_tracking_for_ap(
                    &state,
                    &wifi_geiger_state,
                    &ap_detail_notebook,
                    &ap,
                );
            }
        });
    }

    {
        let state = state.clone();
        let client_list = client_list.clone();
        let wifi_geiger_state = wifi_geiger_state.clone();
        let client_detail_notebook = client_detail_notebook.clone();
        client_geiger_track_btn.connect_clicked(move |_| {
            if let Some(client) = selected_client(&state, &client_list) {
                start_wifi_geiger_tracking_for_client(
                    &state,
                    &wifi_geiger_state,
                    &client_detail_notebook,
                    &client,
                );
            }
        });
    }

    {
        let state = state.clone();
        let ap_list = ap_list.clone();
        ap_geiger_lock_btn.connect_clicked(move |_| {
            let Some(ap) = selected_ap(&state, &ap_list) else {
                state
                    .borrow_mut()
                    .push_status("no AP selected for Wi-Fi lock".to_string());
                return;
            };
            let Some(channel) = ap.channel else {
                state
                    .borrow_mut()
                    .push_status("selected AP has no known channel to lock".to_string());
                return;
            };
            let label = ap.ssid.clone().unwrap_or_else(|| ap.bssid.clone());
            let _ = state
                .borrow_mut()
                .lock_wifi_to_channel(channel, "HT20", label);
        });
    }

    {
        let state = state.clone();
        ap_geiger_unlock_btn.connect_clicked(move |_| {
            let _ = state.borrow_mut().unlock_wifi_card();
        });
    }

    {
        let wifi_geiger_state = wifi_geiger_state.clone();
        let state = state.clone();
        ap_geiger_stop_btn.connect_clicked(move |_| {
            stop_wifi_geiger_tracking(&state, &wifi_geiger_state);
        });
    }

    {
        let state = state.clone();
        let client_list = client_list.clone();
        client_geiger_lock_btn.connect_clicked(move |_| {
            let Some(client) = selected_client(&state, &client_list) else {
                state
                    .borrow_mut()
                    .push_status("no client selected for AP lock".to_string());
                return;
            };
            let (channel, label) = {
                let s = state.borrow();
                let Some(ap_bssid) = client.associated_ap.as_ref() else {
                    drop(s);
                    state
                        .borrow_mut()
                        .push_status("selected client is not associated to an AP".to_string());
                    return;
                };
                let Some(ap) = s.access_points.iter().find(|ap| &ap.bssid == ap_bssid) else {
                    drop(s);
                    state
                        .borrow_mut()
                        .push_status(format!("associated AP {} is not in the AP table", ap_bssid));
                    return;
                };
                let Some(channel) = ap.channel else {
                    drop(s);
                    state
                        .borrow_mut()
                        .push_status("associated AP has no known channel to lock".to_string());
                    return;
                };
                (channel, ap.ssid.clone().unwrap_or_else(|| ap.bssid.clone()))
            };
            let _ = state
                .borrow_mut()
                .lock_wifi_to_channel(channel, "HT20", label);
        });
    }

    {
        let state = state.clone();
        client_geiger_unlock_btn.connect_clicked(move |_| {
            let _ = state.borrow_mut().unlock_wifi_card();
        });
    }

    {
        let wifi_geiger_state = wifi_geiger_state.clone();
        let state = state.clone();
        client_geiger_stop_btn.connect_clicked(move |_| {
            stop_wifi_geiger_tracking(&state, &wifi_geiger_state);
        });
    }

    {
        let bluetooth_selection_suppressed = bluetooth_selection_suppressed.clone();
        let bluetooth_selected_key = bluetooth_selected_key.clone();
        bluetooth_list.connect_row_selected(move |_, row| {
            if *bluetooth_selection_suppressed.borrow() {
                return;
            }
            *bluetooth_selected_key.borrow_mut() = row.map(|entry| entry.widget_name().to_string());
        });
    }

    {
        let state = state.clone();
        let bluetooth_list = bluetooth_list.clone();
        let bluetooth_geiger_state = bluetooth_geiger_state.clone();
        bluetooth_geiger_track.connect_clicked(move |_| {
            let Some(device) = selected_bluetooth(&state, &bluetooth_list) else {
                state
                    .borrow_mut()
                    .push_status("no bluetooth device selected for locate".to_string());
                return;
            };
            start_bluetooth_geiger_tracking(&state, &bluetooth_geiger_state, &device.mac);
        });
    }

    {
        let bluetooth_geiger_state = bluetooth_geiger_state.clone();
        let bluetooth_geiger_rssi = bluetooth_geiger_rssi.clone();
        let bluetooth_geiger_tone = bluetooth_geiger_tone.clone();
        let bluetooth_geiger_progress = bluetooth_geiger_progress.clone();
        bluetooth_geiger_stop.connect_clicked(move |_| {
            if let Some(stop) = bluetooth_geiger_state.borrow_mut().stop.take() {
                stop.store(true, Ordering::Relaxed);
            }
            bluetooth_geiger_state.borrow_mut().receiver = None;
            bluetooth_geiger_state.borrow_mut().target_mac = None;
            bluetooth_geiger_rssi.set_text("RSSI: -- dBm");
            bluetooth_geiger_tone.set_text("Tone: -- Hz");
            bluetooth_geiger_progress.set_fraction(0.0);
            bluetooth_geiger_progress.set_text(Some("Stopped"));
        });
    }

    {
        let state = state.clone();
        let bluetooth_list = bluetooth_list.clone();
        bluetooth_enumerate_btn.connect_clicked(move |_| {
            let Some(device) = selected_bluetooth(&state, &bluetooth_list) else {
                state
                    .borrow_mut()
                    .push_status("no bluetooth device selected for enumeration".to_string());
                return;
            };

            let (controller, sender) = {
                let s = state.borrow();
                (
                    s.settings.bluetooth_controller.clone(),
                    s.bluetooth_sender.clone(),
                )
            };
            state.borrow_mut().push_status(format!(
                "starting active bluetooth enumeration for {}",
                device.mac
            ));

            thread::spawn(move || {
                match bluetooth::connect_and_enumerate_device(controller.as_deref(), &device.mac) {
                    Ok(record) => {
                        let note = record
                            .active_enumeration
                            .as_ref()
                            .and_then(|active| active.last_error.clone())
                            .map(|error| {
                                format!(
                                    "active bluetooth enumeration completed with warning: {error}"
                                )
                            })
                            .unwrap_or_else(|| {
                                format!("active bluetooth enumeration completed for {}", record.mac)
                            });
                        let _ = sender.send(BluetoothEvent::DeviceSeen(record));
                        let _ = sender.send(BluetoothEvent::Log(note));
                    }
                    Err(err) => {
                        let _ = sender.send(BluetoothEvent::Log(format!(
                            "active bluetooth enumeration failed for {}: {err}",
                            device.mac
                        )));
                    }
                }
            });
        });
    }

    {
        let state = state.clone();
        let bluetooth_list = bluetooth_list.clone();
        bluetooth_disconnect_btn.connect_clicked(move |_| {
            let Some(device) = selected_bluetooth(&state, &bluetooth_list) else {
                state
                    .borrow_mut()
                    .push_status("no bluetooth device selected for disconnect".to_string());
                return;
            };

            let (controller, sender) = {
                let s = state.borrow();
                (
                    s.settings.bluetooth_controller.clone(),
                    s.bluetooth_sender.clone(),
                )
            };
            state
                .borrow_mut()
                .push_status(format!("disconnecting bluetooth device {}", device.mac));
            thread::spawn(move || {
                match bluetooth::disconnect_device(controller.as_deref(), &device.mac) {
                    Ok(()) => {
                        if let Ok(record) =
                            bluetooth::read_device_state(controller.as_deref(), &device.mac)
                        {
                            let _ = sender.send(BluetoothEvent::DeviceSeen(record));
                        }
                        let _ = sender.send(BluetoothEvent::Log(format!(
                            "bluetooth device disconnected: {}",
                            device.mac
                        )));
                    }
                    Err(err) => {
                        let _ = sender.send(BluetoothEvent::Log(format!(
                            "bluetooth disconnect failed for {}: {err}",
                            device.mac
                        )));
                    }
                }
            });
        });
    }

    attach_ap_context_menu(
        window,
        &ap_detail_notebook,
        &ap_list,
        state.clone(),
        wifi_geiger_state.clone(),
    );
    attach_client_context_menu(
        window,
        &client_detail_notebook,
        &client_list,
        state.clone(),
        wifi_geiger_state.clone(),
    );
    attach_bluetooth_context_menu(
        &bluetooth_list,
        state.clone(),
        bluetooth_geiger_state.clone(),
    );

    {
        let state = state.clone();
        let ap_list = ap_list.clone();
        ap_export_csv.connect_clicked(move |_| {
            export_selected_ap_csv(&state, &ap_list);
        });
    }
    {
        let state = state.clone();
        let ap_list = ap_list.clone();
        ap_export_pcap.connect_clicked(move |_| {
            if let Some(ap) = selected_ap(&state, &ap_list) {
                let mut s = state.borrow_mut();
                let gps_track = s.gps_track_for_export();
                let out = s.exporter.export_filtered_pcap(
                    &s.session_capture_path,
                    &format!("ap_{}.pcapng", sanitize_name(&ap.bssid)),
                    &format!("wlan.bssid == {}", ap.bssid),
                    &gps_track,
                );
                if out.is_ok() {
                    s.push_status("exported AP filtered PCAPNG with GPS".to_string());
                } else if let Err(err) = out {
                    s.push_status(format!("AP PCAP export failed: {err}"));
                }
            }
        });
    }
    {
        let state = state.clone();
        let ap_list = ap_list.clone();
        ap_export_hs.connect_clicked(move |_| {
            if let Some(ap) = selected_ap(&state, &ap_list) {
                let mut s = state.borrow_mut();
                let gps_track = s.gps_track_for_export();
                let out = s.exporter.export_handshake_pcap(
                    &s.session_capture_path,
                    &ap.bssid,
                    &gps_track,
                );
                if out.is_ok() {
                    s.push_status("exported handshake-only PCAPNG with GPS".to_string());
                } else if let Err(err) = out {
                    s.push_status(format!("Handshake PCAP export failed: {err}"));
                }
            }
        });
    }

    {
        let state = state.clone();
        let client_list = client_list.clone();
        client_export_csv.connect_clicked(move |_| {
            export_selected_client_csv(&state, &client_list);
        });
    }
    {
        let state = state.clone();
        let client_list = client_list.clone();
        client_export_pcap.connect_clicked(move |_| {
            if let Some(client) = selected_client(&state, &client_list) {
                let mut s = state.borrow_mut();
                let gps_track = s.gps_track_for_export();
                let out = s.exporter.export_filtered_pcap(
                    &s.session_capture_path,
                    &format!("client_{}.pcapng", sanitize_name(&client.mac)),
                    &format!("wlan.sa == {} || wlan.da == {}", client.mac, client.mac),
                    &gps_track,
                );
                if out.is_ok() {
                    s.push_status("exported client filtered PCAPNG with GPS".to_string());
                } else if let Err(err) = out {
                    s.push_status(format!("Client PCAP export failed: {err}"));
                }
            }
        });
    }

    {
        let state = state.clone();
        let channel_draw = channel_draw.clone();
        channel_band_combo.connect_changed(move |_| {
            let _ = state.borrow();
            channel_draw.queue_draw();
        });
    }

    {
        let state = state.clone();
        let channel_band_combo = channel_band_combo.clone();
        channel_draw.set_draw_func(move |_, ctx, width, height| {
            draw_channel_usage_chart(
                ctx,
                width as f64,
                height as f64,
                &state.borrow().channel_usage,
                channel_band_combo.active_id().as_deref(),
            );
        });
    }

    {
        let state = state.clone();
        let sdr_hardware_combo = sdr_hardware_combo.clone();
        let sdr_center_freq_entry = sdr_center_freq_entry.clone();
        let sdr_sample_rate_entry = sdr_sample_rate_entry.clone();
        let sdr_log_enable_check = sdr_log_enable_check.clone();
        let sdr_log_dir_entry = sdr_log_dir_entry.clone();
        let sdr_scan_enable_check = sdr_scan_enable_check.clone();
        let sdr_scan_start_entry = sdr_scan_start_entry.clone();
        let sdr_scan_end_entry = sdr_scan_end_entry.clone();
        let sdr_scan_step_entry = sdr_scan_step_entry.clone();
        let sdr_scan_speed_entry = sdr_scan_speed_entry.clone();
        let sdr_squelch_scale = sdr_squelch_scale.clone();
        let sdr_autotune_check = sdr_autotune_check.clone();
        let sdr_bias_tee_check = sdr_bias_tee_check.clone();
        let sdr_no_payload_satcom_check = sdr_no_payload_satcom_check.clone();
        sdr_start_btn.connect_clicked(move |_| {
            let config = sdr_config_from_inputs(
                &sdr_hardware_combo,
                &sdr_center_freq_entry,
                &sdr_sample_rate_entry,
                &sdr_log_enable_check,
                &sdr_log_dir_entry,
                &sdr_scan_enable_check,
                &sdr_scan_start_entry,
                &sdr_scan_end_entry,
                &sdr_scan_step_entry,
                &sdr_scan_speed_entry,
                &sdr_squelch_scale,
                &sdr_autotune_check,
                &sdr_bias_tee_check,
                &sdr_no_payload_satcom_check,
            );
            let mut s = state.borrow_mut();
            s.start_sdr_runtime(config.clone());
            if let Some(runtime) = s.sdr_runtime.as_ref() {
                apply_sdr_runtime_controls(runtime, &config);
                runtime.refresh_dependencies();
            }
        });
    }

    {
        let state = state.clone();
        sdr_stop_btn.connect_clicked(move |_| {
            state.borrow_mut().stop_sdr_runtime();
        });
    }

    {
        let state = state.clone();
        let sdr_hardware_combo = sdr_hardware_combo.clone();
        let sdr_center_freq_entry = sdr_center_freq_entry.clone();
        let sdr_sample_rate_entry = sdr_sample_rate_entry.clone();
        let sdr_log_enable_check = sdr_log_enable_check.clone();
        let sdr_log_dir_entry = sdr_log_dir_entry.clone();
        let sdr_scan_enable_check = sdr_scan_enable_check.clone();
        let sdr_scan_start_entry = sdr_scan_start_entry.clone();
        let sdr_scan_end_entry = sdr_scan_end_entry.clone();
        let sdr_scan_step_entry = sdr_scan_step_entry.clone();
        let sdr_scan_speed_entry = sdr_scan_speed_entry.clone();
        let sdr_squelch_scale = sdr_squelch_scale.clone();
        let sdr_autotune_check = sdr_autotune_check.clone();
        let sdr_bias_tee_check = sdr_bias_tee_check.clone();
        let sdr_no_payload_satcom_check = sdr_no_payload_satcom_check.clone();
        sdr_set_frequency_btn.connect_clicked(move |_| {
            let config = sdr_config_from_inputs(
                &sdr_hardware_combo,
                &sdr_center_freq_entry,
                &sdr_sample_rate_entry,
                &sdr_log_enable_check,
                &sdr_log_dir_entry,
                &sdr_scan_enable_check,
                &sdr_scan_start_entry,
                &sdr_scan_end_entry,
                &sdr_scan_step_entry,
                &sdr_scan_speed_entry,
                &sdr_squelch_scale,
                &sdr_autotune_check,
                &sdr_bias_tee_check,
                &sdr_no_payload_satcom_check,
            );
            let mut s = state.borrow_mut();
            if s.sdr_runtime.is_none() {
                s.start_sdr_runtime(config.clone());
            }
            if let Some(runtime) = s.sdr_runtime.as_ref() {
                runtime.set_center_freq(config.center_freq_hz);
                apply_sdr_runtime_controls(runtime, &config);
            }
        });
    }

    {
        let state = state.clone();
        sdr_pause_check.connect_toggled(move |check| {
            if let Some(runtime) = state.borrow().sdr_runtime.as_ref() {
                runtime.set_sweep_paused(check.is_active());
            }
        });
    }

    {
        let state = state.clone();
        let sdr_hardware_combo = sdr_hardware_combo.clone();
        let sdr_center_freq_entry = sdr_center_freq_entry.clone();
        let sdr_sample_rate_entry = sdr_sample_rate_entry.clone();
        let sdr_log_enable_check = sdr_log_enable_check.clone();
        let sdr_log_dir_entry = sdr_log_dir_entry.clone();
        let sdr_scan_enable_check = sdr_scan_enable_check.clone();
        let sdr_scan_start_entry = sdr_scan_start_entry.clone();
        let sdr_scan_end_entry = sdr_scan_end_entry.clone();
        let sdr_scan_step_entry = sdr_scan_step_entry.clone();
        let sdr_scan_speed_entry = sdr_scan_speed_entry.clone();
        let sdr_squelch_scale = sdr_squelch_scale.clone();
        let sdr_autotune_check = sdr_autotune_check.clone();
        let sdr_bias_tee_check = sdr_bias_tee_check.clone();
        let sdr_scan_enable_check_for_apply = sdr_scan_enable_check.clone();
        let sdr_scan_start_entry_for_apply = sdr_scan_start_entry.clone();
        let sdr_scan_end_entry_for_apply = sdr_scan_end_entry.clone();
        let sdr_scan_step_entry_for_apply = sdr_scan_step_entry.clone();
        let sdr_scan_speed_entry_for_apply = sdr_scan_speed_entry.clone();
        let sdr_squelch_scale_for_apply = sdr_squelch_scale.clone();
        let sdr_autotune_check_for_apply = sdr_autotune_check.clone();
        let sdr_bias_tee_check_for_apply = sdr_bias_tee_check.clone();
        let sdr_no_payload_satcom_check_for_apply = sdr_no_payload_satcom_check.clone();

        let apply_scan: Rc<dyn Fn()> = Rc::new(move || {
            if let Some(runtime) = state.borrow().sdr_runtime.as_ref() {
                let config = sdr_config_from_inputs(
                    &sdr_hardware_combo,
                    &sdr_center_freq_entry,
                    &sdr_sample_rate_entry,
                    &sdr_log_enable_check,
                    &sdr_log_dir_entry,
                    &sdr_scan_enable_check_for_apply,
                    &sdr_scan_start_entry_for_apply,
                    &sdr_scan_end_entry_for_apply,
                    &sdr_scan_step_entry_for_apply,
                    &sdr_scan_speed_entry_for_apply,
                    &sdr_squelch_scale_for_apply,
                    &sdr_autotune_check_for_apply,
                    &sdr_bias_tee_check_for_apply,
                    &sdr_no_payload_satcom_check_for_apply,
                );
                runtime.set_scan_range(
                    config.scan_range_enabled,
                    config.scan_start_hz,
                    config.scan_end_hz,
                    config.scan_step_hz,
                    config.scan_steps_per_sec,
                );
            }
        });

        {
            let apply_scan = apply_scan.clone();
            sdr_scan_enable_check.connect_toggled(move |_| (apply_scan)());
        }
        {
            let apply_scan = apply_scan.clone();
            sdr_scan_start_entry.connect_activate(move |_| (apply_scan)());
        }
        {
            let apply_scan = apply_scan.clone();
            sdr_scan_end_entry.connect_activate(move |_| (apply_scan)());
        }
        {
            let apply_scan = apply_scan.clone();
            sdr_scan_step_entry.connect_activate(move |_| (apply_scan)());
        }
        {
            let apply_scan = apply_scan.clone();
            sdr_scan_speed_entry.connect_activate(move |_| (apply_scan)());
        }
    }

    {
        let state = state.clone();
        let sdr_squelch_value_label = sdr_squelch_value_label.clone();
        sdr_squelch_scale.connect_value_changed(move |scale| {
            let squelch = scale.value() as f32;
            sdr_squelch_value_label.set_text(&format!("{squelch:.0} dBm"));
            if let Some(runtime) = state.borrow().sdr_runtime.as_ref() {
                runtime.set_squelch(squelch);
            }
        });
    }

    {
        let sdr_squelch_scale = sdr_squelch_scale.clone();
        let sdr_spectrogram_draw_for_click = sdr_spectrogram_draw.clone();
        let click = GestureClick::new();
        click.set_button(1);
        click.connect_pressed(move |_, _, _x, y| {
            let height = sdr_spectrogram_draw_for_click.allocated_height().max(1) as f64;
            let normalized = (1.0 - (y / height)).clamp(0.0, 1.0);
            let squelch = -130.0 + (normalized * 120.0);
            sdr_squelch_scale.set_value(squelch);
        });
        sdr_spectrogram_draw.add_controller(click);
    }

    {
        let state = state.clone();
        sdr_autotune_check.connect_toggled(move |check| {
            if let Some(runtime) = state.borrow().sdr_runtime.as_ref() {
                runtime.set_auto_tune(check.is_active());
            }
        });
    }

    {
        let state = state.clone();
        sdr_bias_tee_check.connect_toggled(move |check| {
            if let Some(runtime) = state.borrow().sdr_runtime.as_ref() {
                runtime.set_bias_tee(check.is_active());
            }
        });
    }

    {
        let state = state.clone();
        let sdr_center_freq_entry = sdr_center_freq_entry.clone();
        let sdr_bookmark_combo = sdr_bookmark_combo.clone();
        sdr_bookmark_jump_btn.connect_clicked(move |_| {
            let Some(active_id) = sdr_bookmark_combo.active_id() else {
                return;
            };
            let Ok(freq_hz) = active_id.as_str().parse::<u64>() else {
                return;
            };
            sdr_center_freq_entry.set_text(&freq_hz.to_string());
            if let Some(runtime) = state.borrow().sdr_runtime.as_ref() {
                runtime.set_center_freq(freq_hz);
            }
        });
    }

    {
        let sdr_bookmarks = sdr_bookmarks.clone();
        let sdr_bookmark_combo = sdr_bookmark_combo.clone();
        let sdr_center_freq_entry = sdr_center_freq_entry.clone();
        sdr_bookmark_add_btn.connect_clicked(move |_| {
            let Ok(freq_hz) = sdr_center_freq_entry.text().trim().parse::<u64>() else {
                return;
            };
            if freq_hz < 100_000 {
                return;
            }
            let key = freq_hz.to_string();
            let label = format!("{:.6} MHz", freq_hz as f64 / 1_000_000.0);
            if !sdr_bookmarks
                .borrow()
                .iter()
                .any(|(_, freq)| *freq == freq_hz)
            {
                sdr_bookmarks.borrow_mut().push((label.clone(), freq_hz));
                sdr_bookmark_combo.append(Some(&key), &label);
            }
            sdr_bookmark_combo.set_active_id(Some(&key));
        });
    }

    {
        let state = state.clone();
        let sdr_hardware_combo = sdr_hardware_combo.clone();
        let sdr_center_freq_entry = sdr_center_freq_entry.clone();
        let sdr_sample_rate_entry = sdr_sample_rate_entry.clone();
        let sdr_log_enable_check = sdr_log_enable_check.clone();
        let sdr_log_dir_entry = sdr_log_dir_entry.clone();
        let sdr_scan_enable_check = sdr_scan_enable_check.clone();
        let sdr_scan_start_entry = sdr_scan_start_entry.clone();
        let sdr_scan_end_entry = sdr_scan_end_entry.clone();
        let sdr_scan_step_entry = sdr_scan_step_entry.clone();
        let sdr_scan_speed_entry = sdr_scan_speed_entry.clone();
        let sdr_squelch_scale = sdr_squelch_scale.clone();
        let sdr_autotune_check = sdr_autotune_check.clone();
        let sdr_bias_tee_check = sdr_bias_tee_check.clone();
        let sdr_no_payload_satcom_check = sdr_no_payload_satcom_check.clone();
        let sdr_sample_duration_spin = sdr_sample_duration_spin.clone();
        let sdr_sample_dir_entry = sdr_sample_dir_entry.clone();
        sdr_capture_sample_btn.connect_clicked(move |_| {
            let config = sdr_config_from_inputs(
                &sdr_hardware_combo,
                &sdr_center_freq_entry,
                &sdr_sample_rate_entry,
                &sdr_log_enable_check,
                &sdr_log_dir_entry,
                &sdr_scan_enable_check,
                &sdr_scan_start_entry,
                &sdr_scan_end_entry,
                &sdr_scan_step_entry,
                &sdr_scan_speed_entry,
                &sdr_squelch_scale,
                &sdr_autotune_check,
                &sdr_bias_tee_check,
                &sdr_no_payload_satcom_check,
            );
            let duration_secs = sdr_sample_duration_spin.value_as_int().max(1) as u32;
            let output_dir_text = sdr_sample_dir_entry.text().trim().to_string();
            let output_dir = if output_dir_text.is_empty() {
                config.log_output_dir.join("iq_samples")
            } else {
                PathBuf::from(output_dir_text)
            };

            let mut s = state.borrow_mut();
            if s.sdr_runtime.is_none() {
                s.start_sdr_runtime(config.clone());
            }
            if let Some(runtime) = s.sdr_runtime.as_ref() {
                runtime.set_center_freq(config.center_freq_hz);
                apply_sdr_runtime_controls(runtime, &config);
                runtime.capture_sample(duration_secs, output_dir.clone());
                s.push_status(format!(
                    "capturing IQ sample at {} Hz for {}s into {}",
                    config.center_freq_hz,
                    duration_secs,
                    output_dir.display()
                ));
            }
        });
    }

    {
        let state = state.clone();
        let sdr_model = sdr_model.clone();
        let sdr_log_dir_entry = sdr_log_dir_entry.clone();
        sdr_export_map_btn.connect_clicked(move |_| {
            let points = sdr_model.borrow().map_points.clone();
            if points.is_empty() {
                state
                    .borrow_mut()
                    .push_status("SDR map export skipped: no coordinate points yet".to_string());
                return;
            }

            let output_dir_text = sdr_log_dir_entry.text().trim().to_string();
            let output_dir = if output_dir_text.is_empty() {
                SdrConfig::default().log_output_dir
            } else {
                PathBuf::from(output_dir_text)
            };

            if let Err(err) = fs::create_dir_all(&output_dir) {
                state.borrow_mut().push_status(format!(
                    "SDR map export failed (create dir {}): {err}",
                    output_dir.display()
                ));
                return;
            }

            let file_path = output_dir.join(format!(
                "sdr_map_points_{}.json",
                Utc::now().format("%Y%m%dT%H%M%SZ")
            ));
            match serde_json::to_vec_pretty(&points) {
                Ok(data) => {
                    if let Err(err) = fs::write(&file_path, data) {
                        state.borrow_mut().push_status(format!(
                            "SDR map export failed (write {}): {err}",
                            file_path.display()
                        ));
                    } else {
                        state.borrow_mut().push_status(format!(
                            "exported SDR map points: {}",
                            file_path.display()
                        ));
                    }
                }
                Err(err) => {
                    state
                        .borrow_mut()
                        .push_status(format!("SDR map export serialization failed: {err}"));
                }
            }
        });
    }

    {
        let state = state.clone();
        let sdr_model = sdr_model.clone();
        let sdr_log_dir_entry = sdr_log_dir_entry.clone();
        sdr_export_satcom_btn.connect_clicked(move |_| {
            let observations = sdr_model.borrow().satcom_observations.clone();
            if observations.is_empty() {
                state.borrow_mut().push_status(
                    "SDR satcom export skipped: no satcom observations yet".to_string(),
                );
                return;
            }

            let output_dir_text = sdr_log_dir_entry.text().trim().to_string();
            let output_dir = if output_dir_text.is_empty() {
                SdrConfig::default().log_output_dir
            } else {
                PathBuf::from(output_dir_text)
            };

            if let Err(err) = fs::create_dir_all(&output_dir) {
                state.borrow_mut().push_status(format!(
                    "SDR satcom export failed (create dir {}): {err}",
                    output_dir.display()
                ));
                return;
            }

            let file_path = output_dir.join(format!(
                "sdr_satcom_audit_{}.json",
                Utc::now().format("%Y%m%dT%H%M%SZ")
            ));
            match serde_json::to_vec_pretty(&observations) {
                Ok(data) => {
                    if let Err(err) = fs::write(&file_path, data) {
                        state.borrow_mut().push_status(format!(
                            "SDR satcom export failed (write {}): {err}",
                            file_path.display()
                        ));
                    } else {
                        state.borrow_mut().push_status(format!(
                            "exported SDR satcom audit: {}",
                            file_path.display()
                        ));
                    }
                }
                Err(err) => {
                    state
                        .borrow_mut()
                        .push_status(format!("SDR satcom export serialization failed: {err}"));
                }
            }
        });
    }

    {
        let state = state.clone();
        let sdr_hardware_combo = sdr_hardware_combo.clone();
        let sdr_center_freq_entry = sdr_center_freq_entry.clone();
        let sdr_sample_rate_entry = sdr_sample_rate_entry.clone();
        let sdr_log_enable_check = sdr_log_enable_check.clone();
        let sdr_log_dir_entry = sdr_log_dir_entry.clone();
        let sdr_scan_enable_check = sdr_scan_enable_check.clone();
        let sdr_scan_start_entry = sdr_scan_start_entry.clone();
        let sdr_scan_end_entry = sdr_scan_end_entry.clone();
        let sdr_scan_step_entry = sdr_scan_step_entry.clone();
        let sdr_scan_speed_entry = sdr_scan_speed_entry.clone();
        let sdr_squelch_scale = sdr_squelch_scale.clone();
        let sdr_autotune_check = sdr_autotune_check.clone();
        let sdr_bias_tee_check = sdr_bias_tee_check.clone();
        let sdr_no_payload_satcom_check = sdr_no_payload_satcom_check.clone();
        sdr_dep_refresh_btn.connect_clicked(move |_| {
            let config = sdr_config_from_inputs(
                &sdr_hardware_combo,
                &sdr_center_freq_entry,
                &sdr_sample_rate_entry,
                &sdr_log_enable_check,
                &sdr_log_dir_entry,
                &sdr_scan_enable_check,
                &sdr_scan_start_entry,
                &sdr_scan_end_entry,
                &sdr_scan_step_entry,
                &sdr_scan_speed_entry,
                &sdr_squelch_scale,
                &sdr_autotune_check,
                &sdr_bias_tee_check,
                &sdr_no_payload_satcom_check,
            );
            let mut s = state.borrow_mut();
            if s.sdr_runtime.is_none() {
                s.start_sdr_runtime(config.clone());
            }
            if let Some(runtime) = s.sdr_runtime.as_ref() {
                apply_sdr_runtime_controls(runtime, &config);
                runtime.refresh_dependencies();
            }
        });
    }

    {
        let state = state.clone();
        let sdr_hardware_combo = sdr_hardware_combo.clone();
        let sdr_center_freq_entry = sdr_center_freq_entry.clone();
        let sdr_sample_rate_entry = sdr_sample_rate_entry.clone();
        let sdr_log_enable_check = sdr_log_enable_check.clone();
        let sdr_log_dir_entry = sdr_log_dir_entry.clone();
        let sdr_scan_enable_check = sdr_scan_enable_check.clone();
        let sdr_scan_start_entry = sdr_scan_start_entry.clone();
        let sdr_scan_end_entry = sdr_scan_end_entry.clone();
        let sdr_scan_step_entry = sdr_scan_step_entry.clone();
        let sdr_scan_speed_entry = sdr_scan_speed_entry.clone();
        let sdr_squelch_scale = sdr_squelch_scale.clone();
        let sdr_autotune_check = sdr_autotune_check.clone();
        let sdr_bias_tee_check = sdr_bias_tee_check.clone();
        let sdr_no_payload_satcom_check = sdr_no_payload_satcom_check.clone();
        sdr_dep_install_btn.connect_clicked(move |_| {
            let config = sdr_config_from_inputs(
                &sdr_hardware_combo,
                &sdr_center_freq_entry,
                &sdr_sample_rate_entry,
                &sdr_log_enable_check,
                &sdr_log_dir_entry,
                &sdr_scan_enable_check,
                &sdr_scan_start_entry,
                &sdr_scan_end_entry,
                &sdr_scan_step_entry,
                &sdr_scan_speed_entry,
                &sdr_squelch_scale,
                &sdr_autotune_check,
                &sdr_bias_tee_check,
                &sdr_no_payload_satcom_check,
            );
            let mut s = state.borrow_mut();
            if s.sdr_runtime.is_none() {
                s.start_sdr_runtime(config.clone());
            }
            if let Some(runtime) = s.sdr_runtime.as_ref() {
                apply_sdr_runtime_controls(runtime, &config);
                runtime.install_missing_dependencies();
            }
        });
    }

    {
        let state = state.clone();
        let sdr_hardware_combo = sdr_hardware_combo.clone();
        let sdr_center_freq_entry = sdr_center_freq_entry.clone();
        let sdr_sample_rate_entry = sdr_sample_rate_entry.clone();
        let sdr_log_enable_check = sdr_log_enable_check.clone();
        let sdr_log_dir_entry = sdr_log_dir_entry.clone();
        let sdr_scan_enable_check = sdr_scan_enable_check.clone();
        let sdr_scan_start_entry = sdr_scan_start_entry.clone();
        let sdr_scan_end_entry = sdr_scan_end_entry.clone();
        let sdr_scan_step_entry = sdr_scan_step_entry.clone();
        let sdr_scan_speed_entry = sdr_scan_speed_entry.clone();
        let sdr_squelch_scale = sdr_squelch_scale.clone();
        let sdr_autotune_check = sdr_autotune_check.clone();
        let sdr_bias_tee_check = sdr_bias_tee_check.clone();
        let sdr_no_payload_satcom_check = sdr_no_payload_satcom_check.clone();
        let sdr_decoder_combo = sdr_decoder_combo.clone();
        let sdr_decoder_lookup = sdr_decoder_lookup.clone();
        sdr_decode_start_btn.connect_clicked(move |_| {
            let config = sdr_config_from_inputs(
                &sdr_hardware_combo,
                &sdr_center_freq_entry,
                &sdr_sample_rate_entry,
                &sdr_log_enable_check,
                &sdr_log_dir_entry,
                &sdr_scan_enable_check,
                &sdr_scan_start_entry,
                &sdr_scan_end_entry,
                &sdr_scan_step_entry,
                &sdr_scan_speed_entry,
                &sdr_squelch_scale,
                &sdr_autotune_check,
                &sdr_bias_tee_check,
                &sdr_no_payload_satcom_check,
            );
            let Some(decoder_id) = sdr_decoder_combo.active_id() else {
                return;
            };
            let Some(decoder) = sdr_decoder_lookup
                .borrow()
                .get(decoder_id.as_str())
                .cloned()
            else {
                return;
            };

            let mut s = state.borrow_mut();
            if s.sdr_runtime.is_none() {
                s.start_sdr_runtime(config.clone());
            }
            if let Some(runtime) = s.sdr_runtime.as_ref() {
                runtime.set_center_freq(config.center_freq_hz);
                apply_sdr_runtime_controls(runtime, &config);
                runtime.start_decode(decoder);
            }
        });
    }

    {
        let state = state.clone();
        sdr_decode_stop_btn.connect_clicked(move |_| {
            if let Some(runtime) = state.borrow().sdr_runtime.as_ref() {
                runtime.stop_decode();
            }
        });
    }

    {
        let state = state.clone();
        let sdr_fft_draw = sdr_fft_draw.clone();
        let sdr_fft_draw_for_click = sdr_fft_draw.clone();
        let sdr_model = sdr_model.clone();
        let sdr_decoder_lookup = sdr_decoder_lookup.clone();
        let sdr_decoder_order = sdr_decoder_order.clone();
        let sdr_hardware_combo = sdr_hardware_combo.clone();
        let sdr_center_freq_entry = sdr_center_freq_entry.clone();
        let sdr_sample_rate_entry = sdr_sample_rate_entry.clone();
        let sdr_log_enable_check = sdr_log_enable_check.clone();
        let sdr_log_dir_entry = sdr_log_dir_entry.clone();
        let sdr_scan_enable_check = sdr_scan_enable_check.clone();
        let sdr_scan_start_entry = sdr_scan_start_entry.clone();
        let sdr_scan_end_entry = sdr_scan_end_entry.clone();
        let sdr_scan_step_entry = sdr_scan_step_entry.clone();
        let sdr_scan_speed_entry = sdr_scan_speed_entry.clone();
        let sdr_squelch_scale = sdr_squelch_scale.clone();
        let sdr_autotune_check = sdr_autotune_check.clone();
        let sdr_bias_tee_check = sdr_bias_tee_check.clone();
        let sdr_no_payload_satcom_check = sdr_no_payload_satcom_check.clone();
        let right_click = GestureClick::new();
        right_click.set_button(3);
        right_click.connect_pressed(move |_, _, x, y| {
            let width = sdr_fft_draw_for_click.allocated_width().max(1) as f64;
            let model = sdr_model.borrow();
            let sample_rate = model.sample_rate_hz.max(1) as f64;
            let left_hz = model.current_freq_hz as f64 - sample_rate / 2.0;
            let click_ratio = (x / width).clamp(0.0, 1.0);
            let clicked_freq_hz = (left_hz + click_ratio * sample_rate).max(100_000.0) as u64;
            drop(model);

            sdr_center_freq_entry.set_text(&clicked_freq_hz.to_string());

            let popover = Popover::new();
            popover.set_has_arrow(true);
            popover.set_parent(&sdr_fft_draw_for_click);
            let rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover.set_pointing_to(Some(&rect));
            let popover_box = GtkBox::new(Orientation::Vertical, 4);
            let title = Label::new(Some(&format!("Decode {} Hz", clicked_freq_hz)));
            title.set_xalign(0.0);
            popover_box.append(&title);
            for decoder_id in sdr_decoder_order.iter() {
                let Some(decoder) = sdr_decoder_lookup.borrow().get(decoder_id).cloned() else {
                    continue;
                };
                let button = Button::with_label(&format!("Decode -> {}", decoder.label()));
                let state = state.clone();
                let decoder = decoder.clone();
                let sdr_hardware_combo = sdr_hardware_combo.clone();
                let sdr_center_freq_entry = sdr_center_freq_entry.clone();
                let sdr_sample_rate_entry = sdr_sample_rate_entry.clone();
                let sdr_log_enable_check = sdr_log_enable_check.clone();
                let sdr_log_dir_entry = sdr_log_dir_entry.clone();
                let sdr_scan_enable_check = sdr_scan_enable_check.clone();
                let sdr_scan_start_entry = sdr_scan_start_entry.clone();
                let sdr_scan_end_entry = sdr_scan_end_entry.clone();
                let sdr_scan_step_entry = sdr_scan_step_entry.clone();
                let sdr_scan_speed_entry = sdr_scan_speed_entry.clone();
                let sdr_squelch_scale = sdr_squelch_scale.clone();
                let sdr_autotune_check = sdr_autotune_check.clone();
                let sdr_bias_tee_check = sdr_bias_tee_check.clone();
                let sdr_no_payload_satcom_check = sdr_no_payload_satcom_check.clone();
                let popover = popover.clone();
                button.connect_clicked(move |_| {
                    let config = sdr_config_from_inputs(
                        &sdr_hardware_combo,
                        &sdr_center_freq_entry,
                        &sdr_sample_rate_entry,
                        &sdr_log_enable_check,
                        &sdr_log_dir_entry,
                        &sdr_scan_enable_check,
                        &sdr_scan_start_entry,
                        &sdr_scan_end_entry,
                        &sdr_scan_step_entry,
                        &sdr_scan_speed_entry,
                        &sdr_squelch_scale,
                        &sdr_autotune_check,
                        &sdr_bias_tee_check,
                        &sdr_no_payload_satcom_check,
                    );
                    let mut s = state.borrow_mut();
                    if s.sdr_runtime.is_none() {
                        s.start_sdr_runtime(config.clone());
                    }
                    if let Some(runtime) = s.sdr_runtime.as_ref() {
                        runtime.set_center_freq(clicked_freq_hz);
                        apply_sdr_runtime_controls(runtime, &config);
                        runtime.start_decode(decoder.clone());
                    }
                    popover.popdown();
                });
                popover_box.append(&button);
            }
            popover.set_child(Some(&popover_box));
            popover.popup();
        });
        sdr_fft_draw.add_controller(right_click);
    }

    (
        notebook,
        UiWidgets {
            ap_root,
            ap_bottom,
            ap_detail_notebook,
            ap_assoc_box,
            ap_header_holder,
            ap_list,
            ap_pagination,
            ap_selection_suppressed,
            ap_selected_key,
            ap_detail_label,
            ap_notes_view,
            ap_assoc_header_holder,
            ap_assoc_list,
            ap_assoc_pagination,
            ap_packet_draw,
            ap_selected_packet_mix: selected_packet_mix,
            client_header_holder,
            client_list,
            client_pagination,
            client_selection_suppressed,
            client_selected_key,
            client_detail_label,
            client_root,
            client_detail_notebook,
            ap_wifi_geiger_target_label,
            ap_wifi_geiger_lock_label,
            ap_wifi_geiger_rssi,
            ap_wifi_geiger_tone,
            ap_wifi_geiger_meter,
            client_wifi_geiger_target_label,
            client_wifi_geiger_lock_label,
            client_wifi_geiger_rssi,
            client_wifi_geiger_tone,
            client_wifi_geiger_meter,
            wifi_geiger_state,
            bluetooth_list,
            bluetooth_header_holder,
            bluetooth_pagination,
            bluetooth_selection_suppressed,
            bluetooth_selected_key,
            bluetooth_detail_box,
            bluetooth_identity_label,
            bluetooth_passive_label,
            bluetooth_active_summary_label,
            bluetooth_readable_label,
            bluetooth_services_label,
            bluetooth_characteristics_label,
            bluetooth_descriptors_label,
            bluetooth_root,
            bluetooth_bottom,
            bluetooth_geiger_rssi,
            bluetooth_geiger_tone,
            bluetooth_geiger_progress,
            bluetooth_geiger_state,
            channel_draw,
            sdr_frequency_label,
            sdr_decoder_label,
            sdr_dependency_label,
            sdr_fft_draw,
            sdr_spectrogram_draw,
            sdr_map_draw,
            sdr_decode_header_holder,
            sdr_decode_list,
            sdr_decode_pagination,
            sdr_satcom_header_holder,
            sdr_satcom_list,
            sdr_satcom_pagination,
            sdr_model,
            status_label,
            gps_status_label,
        },
    )
}

fn bind_poll_loop(
    receiver: Receiver<CaptureEvent>,
    bluetooth_receiver: Receiver<BluetoothEvent>,
    sdr_receiver: Receiver<SdrEvent>,
    state: Rc<RefCell<AppState>>,
    widgets: UiWidgets,
    capture_start_btn: Button,
    capture_stop_btn: Button,
    global_status_label: Label,
    global_gps_status_label: Label,
    notebook: Notebook,
    window: &ApplicationWindow,
) {
    let UiWidgets {
        ap_root: _ap_root,
        ap_bottom: _ap_bottom,
        ap_detail_notebook: _ap_detail_notebook,
        ap_assoc_box: _ap_assoc_box,
        ap_header_holder,
        ap_list,
        ap_pagination,
        ap_selection_suppressed,
        ap_selected_key,
        ap_detail_label,
        ap_notes_view,
        ap_assoc_header_holder,
        ap_assoc_list,
        ap_assoc_pagination,
        ap_packet_draw,
        ap_selected_packet_mix,
        client_header_holder,
        client_list,
        client_pagination,
        client_selection_suppressed,
        client_selected_key,
        client_detail_label,
        client_root: _client_root,
        client_detail_notebook: _client_detail_notebook,
        ap_wifi_geiger_target_label,
        ap_wifi_geiger_lock_label,
        ap_wifi_geiger_rssi,
        ap_wifi_geiger_tone,
        ap_wifi_geiger_meter,
        client_wifi_geiger_target_label,
        client_wifi_geiger_lock_label,
        client_wifi_geiger_rssi,
        client_wifi_geiger_tone,
        client_wifi_geiger_meter,
        wifi_geiger_state,
        bluetooth_list,
        bluetooth_header_holder,
        bluetooth_pagination,
        bluetooth_selection_suppressed,
        bluetooth_selected_key,
        bluetooth_detail_box,
        bluetooth_identity_label,
        bluetooth_passive_label,
        bluetooth_active_summary_label,
        bluetooth_readable_label,
        bluetooth_services_label,
        bluetooth_characteristics_label,
        bluetooth_descriptors_label,
        bluetooth_root: _bluetooth_root,
        bluetooth_bottom: _bluetooth_bottom,
        bluetooth_geiger_rssi,
        bluetooth_geiger_tone,
        bluetooth_geiger_progress,
        bluetooth_geiger_state,
        channel_draw,
        sdr_frequency_label,
        sdr_decoder_label,
        sdr_dependency_label,
        sdr_fft_draw,
        sdr_spectrogram_draw,
        sdr_map_draw,
        sdr_decode_header_holder: _sdr_decode_header_holder,
        sdr_decode_list,
        sdr_decode_pagination,
        sdr_satcom_header_holder: _sdr_satcom_header_holder,
        sdr_satcom_list,
        sdr_satcom_pagination,
        sdr_model,
        status_label,
        gps_status_label,
    } = widgets;
    let window = window.clone();
    let last_ap_list_refresh = Rc::new(RefCell::new(None::<Instant>));
    let last_client_list_refresh = Rc::new(RefCell::new(None::<Instant>));
    let last_bluetooth_list_refresh = Rc::new(RefCell::new(None::<Instant>));
    let last_ap_list_signature = Rc::new(RefCell::new(None::<String>));
    let last_client_list_signature = Rc::new(RefCell::new(None::<String>));
    let last_bluetooth_list_signature = Rc::new(RefCell::new(None::<String>));
    let last_ap_selected_key = Rc::new(RefCell::new(None::<String>));
    let last_ap_detail_signature = Rc::new(RefCell::new(None::<String>));
    let last_ap_assoc_signature = Rc::new(RefCell::new(None::<String>));
    let last_client_selected_key = Rc::new(RefCell::new(None::<String>));
    let last_client_detail_signature = Rc::new(RefCell::new(None::<String>));
    let last_bluetooth_selected_key = Rc::new(RefCell::new(None::<String>));
    let last_bluetooth_detail_signature = Rc::new(RefCell::new(None::<String>));
    let last_sdr_decode_signature = Rc::new(RefCell::new(None::<String>));
    let last_sdr_satcom_signature = Rc::new(RefCell::new(None::<String>));
    let last_ap_pagination_generation = Cell::new(ap_pagination.generation.get());
    let last_ap_assoc_pagination_generation = Cell::new(ap_assoc_pagination.generation.get());
    let last_client_pagination_generation = Cell::new(client_pagination.generation.get());
    let last_bluetooth_pagination_generation = Cell::new(bluetooth_pagination.generation.get());
    let last_sdr_pagination_generation = Cell::new(sdr_decode_pagination.generation.get());
    let last_sdr_satcom_pagination_generation = Cell::new(sdr_satcom_pagination.generation.get());
    let pending_ap_refresh = Cell::new(true);
    let pending_client_refresh = Cell::new(true);
    let pending_bluetooth_refresh = Cell::new(true);
    let pending_channel_refresh = Cell::new(true);
    let pending_sdr_refresh = Cell::new(true);

    glib::timeout_add_local(Duration::from_millis(UI_POLL_INTERVAL_MS), move || {
        let mut refresh = UiRefreshHint::none();
        let mut layout_changed = false;
        {
            let mut s = state.borrow_mut();
            s.maybe_record_gps_track_point();
            if s.layout_dirty {
                s.layout_dirty = false;
                layout_changed = true;
            }
        }

        for event in drain_capture_events_batch(&receiver, MAX_CAPTURE_EVENTS_PER_TICK) {
            let hint = {
                let mut s = state.borrow_mut();
                s.apply_capture_event(event).unwrap_or_default()
            };
            refresh.merge(hint);
        }

        for event in
            drain_bluetooth_events_batch(&bluetooth_receiver, MAX_BLUETOOTH_EVENTS_PER_TICK)
        {
            let hint = {
                let mut s = state.borrow_mut();
                s.apply_bluetooth_event(event).unwrap_or_default()
            };
            refresh.merge(hint);
        }

        for event in drain_sdr_events_batch(&sdr_receiver, MAX_SDR_EVENTS_PER_TICK) {
            match event {
                SdrEvent::Log(text) => {
                    state.borrow_mut().push_status(format!("SDR: {text}"));
                    refresh.status = true;
                }
                SdrEvent::FrequencyChanged(freq_hz) => {
                    let mut model = sdr_model.borrow_mut();
                    model.current_freq_hz = freq_hz;
                    pending_sdr_refresh.set(true);
                }
                SdrEvent::SpectrumFrame(frame) => {
                    let mut model = sdr_model.borrow_mut();
                    model.current_freq_hz = frame.center_freq_hz;
                    model.sample_rate_hz = frame.sample_rate_hz;
                    model.spectrum_bins = frame.bins_db.clone();
                    model.spectrogram_rows.push(frame.bins_db);
                    if model.spectrogram_rows.len() > 160 {
                        let keep_from = model.spectrogram_rows.len() - 160;
                        model.spectrogram_rows = model.spectrogram_rows.split_off(keep_from);
                    }
                    pending_sdr_refresh.set(true);
                }
                SdrEvent::DecodeRow(row) => {
                    let mut model = sdr_model.borrow_mut();
                    model.decode_rows.push(row);
                    if model.decode_rows.len() > 5000 {
                        let keep_from = model.decode_rows.len() - 5000;
                        model.decode_rows = model.decode_rows.split_off(keep_from);
                    }
                    pending_sdr_refresh.set(true);
                }
                SdrEvent::DecoderState { running, decoder } => {
                    let mut model = sdr_model.borrow_mut();
                    model.decoder_running = if running { decoder } else { None };
                    model.sweep_paused = running;
                    pending_sdr_refresh.set(true);
                }
                SdrEvent::DependencyStatus(status) => {
                    let mut model = sdr_model.borrow_mut();
                    model.dependency_status = status;
                    pending_sdr_refresh.set(true);
                }
                SdrEvent::MapPoint(point) => {
                    let mut model = sdr_model.borrow_mut();
                    model.map_points.push(point);
                    if model.map_points.len() > 20_000 {
                        let keep_from = model.map_points.len() - 20_000;
                        model.map_points = model.map_points.split_off(keep_from);
                    }
                    pending_sdr_refresh.set(true);
                }
                SdrEvent::SatcomObservation(observation) => {
                    let mut model = sdr_model.borrow_mut();
                    model.satcom_observations.push(observation);
                    if model.satcom_observations.len() > 20_000 {
                        let keep_from = model.satcom_observations.len() - 20_000;
                        model.satcom_observations = model.satcom_observations.split_off(keep_from);
                    }
                    pending_sdr_refresh.set(true);
                }
                SdrEvent::SquelchChanged(squelch_dbm) => {
                    let mut model = sdr_model.borrow_mut();
                    model.squelch_dbm = squelch_dbm;
                    pending_sdr_refresh.set(true);
                }
            }
        }

        let privilege_alert = {
            let mut s = state.borrow_mut();
            s.pending_privilege_alert.take()
        };
        if let Some(message) = privilege_alert {
            open_privilege_failure_dialog(&window, &message);
        }

        let stop_completion = {
            let s = state.borrow();
            s.pending_stop_completion
                .as_ref()
                .and_then(|rx| rx.try_recv().ok())
        };
        if let Some(completion) = stop_completion {
            let mut s = state.borrow_mut();
            s.scan_stop_in_progress = false;
            s.pending_stop_completion = None;
            let restart_message = s.pending_scan_restart_message.take();
            s.wifi_interface_restore_types.clear();
            if let Some(interfaces) = completion.cleared_interfaces {
                s.settings.interfaces = interfaces;
            }
            for line in completion.status_lines {
                s.push_status(line);
            }
            if let Some(message) = restart_message {
                s.push_status(message);
                s.start_scanning();
            } else {
                s.push_status("scanning stopped".to_string());
            }
            refresh.status = true;
        }

        let start_completion = {
            let s = state.borrow();
            s.pending_start_completion
                .as_ref()
                .and_then(|rx| rx.try_recv().ok())
        };
        if let Some(completion) = start_completion {
            let mut s = state.borrow_mut();
            s.scan_start_in_progress = false;
            s.pending_start_completion = None;
            if let Some(interfaces) = completion.updated_interfaces {
                s.settings.interfaces = interfaces;
            }
            s.wifi_interface_restore_types = completion.wifi_interface_restore_types;
            if let Some(runtime) = completion.capture_runtime {
                s.capture_runtime = Some(runtime);
            }
            if let Some(runtime) = completion.bluetooth_runtime {
                s.bluetooth_runtime = Some(runtime);
            }
            for line in completion.status_lines {
                s.push_status(line);
            }
            if let Some(alert) = completion.privilege_alert {
                s.pending_privilege_alert = Some(alert);
            }
            if completion.wifi_started && completion.bluetooth_started {
                s.push_status("Wi-Fi and Bluetooth scanning started".to_string());
            } else if completion.wifi_started {
                s.push_status("Wi-Fi scanning started".to_string());
            } else if completion.bluetooth_started && completion.wifi_failed {
                s.push_status(
                    "Bluetooth scanning started; Wi-Fi capture failed to start".to_string(),
                );
            } else if completion.bluetooth_started {
                s.push_status("Bluetooth scanning started".to_string());
            } else if completion.wifi_failed {
                s.push_status("Wi-Fi capture failed to start".to_string());
            } else {
                s.push_status("scan start completed".to_string());
            }
            refresh.status = true;
        }

        if layout_changed {
            let s = state.borrow();
            rebuild_header_container(
                &ap_header_holder,
                &ap_table_header(&s.settings.ap_table_layout, &s.ap_sort, state.clone()),
                Some(&ap_pagination.filter_bar),
            );
            rebuild_header_container(
                &client_header_holder,
                &client_table_header(
                    &s.settings.client_table_layout,
                    &s.client_sort,
                    state.clone(),
                ),
                Some(&client_pagination.filter_bar),
            );
            rebuild_header_container(
                &ap_assoc_header_holder,
                &ap_assoc_clients_header(
                    &s.settings.assoc_client_table_layout,
                    &s.assoc_sort,
                    state.clone(),
                ),
                Some(&ap_assoc_pagination.filter_bar),
            );
            rebuild_header_container(
                &bluetooth_header_holder,
                &bluetooth_table_header(
                    &s.settings.bluetooth_table_layout,
                    &s.bluetooth_sort,
                    state.clone(),
                ),
                Some(&bluetooth_pagination.filter_bar),
            );
            *last_ap_list_signature.borrow_mut() = None;
            *last_client_list_signature.borrow_mut() = None;
            *last_bluetooth_list_signature.borrow_mut() = None;
            pending_ap_refresh.set(true);
            pending_client_refresh.set(true);
            pending_bluetooth_refresh.set(true);
            refresh.status = true;
        }

        if refresh.ap_list {
            pending_ap_refresh.set(true);
        }
        if refresh.client_list {
            pending_client_refresh.set(true);
        }
        if refresh.bluetooth_list {
            pending_bluetooth_refresh.set(true);
        }
        if refresh.channel_chart {
            pending_channel_refresh.set(true);
        }
        if last_ap_pagination_generation.get() != ap_pagination.generation.get() {
            last_ap_pagination_generation.set(ap_pagination.generation.get());
            *last_ap_list_signature.borrow_mut() = None;
            pending_ap_refresh.set(true);
        }
        if last_client_pagination_generation.get() != client_pagination.generation.get() {
            last_client_pagination_generation.set(client_pagination.generation.get());
            *last_client_list_signature.borrow_mut() = None;
            pending_client_refresh.set(true);
        }
        if last_ap_assoc_pagination_generation.get() != ap_assoc_pagination.generation.get() {
            last_ap_assoc_pagination_generation.set(ap_assoc_pagination.generation.get());
            *last_ap_assoc_signature.borrow_mut() = None;
        }
        if last_bluetooth_pagination_generation.get() != bluetooth_pagination.generation.get() {
            last_bluetooth_pagination_generation.set(bluetooth_pagination.generation.get());
            *last_bluetooth_list_signature.borrow_mut() = None;
            pending_bluetooth_refresh.set(true);
        }
        if last_sdr_pagination_generation.get() != sdr_decode_pagination.generation.get() {
            last_sdr_pagination_generation.set(sdr_decode_pagination.generation.get());
            *last_sdr_decode_signature.borrow_mut() = None;
            pending_sdr_refresh.set(true);
        }
        if last_sdr_satcom_pagination_generation.get() != sdr_satcom_pagination.generation.get() {
            last_sdr_satcom_pagination_generation.set(sdr_satcom_pagination.generation.get());
            *last_sdr_satcom_signature.borrow_mut() = None;
            pending_sdr_refresh.set(true);
        }

        let active_tab = notebook.current_page().unwrap_or(ACCESS_POINTS_TAB_INDEX);
        let ap_tab_active = active_tab == ACCESS_POINTS_TAB_INDEX;
        let client_tab_active = active_tab == CLIENTS_TAB_INDEX;
        let bluetooth_tab_active = active_tab == BLUETOOTH_TAB_INDEX;
        let channel_tab_active = active_tab == CHANNEL_USAGE_TAB_INDEX;
        let sdr_tab_active = active_tab == SDR_TAB_INDEX;

        let ap_selected_key_now = ap_selected_key.borrow().clone();
        let client_selected_key_now = client_selected_key.borrow().clone();
        let bluetooth_selected_key_now = bluetooth_selected_key.borrow().clone();

        let ap_selection_changed = {
            let mut last = last_ap_selected_key.borrow_mut();
            if *last != ap_selected_key_now {
                *last = ap_selected_key_now.clone();
                true
            } else {
                false
            }
        };
        let client_selection_changed = {
            let mut last = last_client_selected_key.borrow_mut();
            if *last != client_selected_key_now {
                *last = client_selected_key_now.clone();
                true
            } else {
                false
            }
        };
        let bluetooth_selection_changed = {
            let mut last = last_bluetooth_selected_key.borrow_mut();
            if *last != bluetooth_selected_key_now {
                *last = bluetooth_selected_key_now.clone();
                true
            } else {
                false
            }
        };

        if ap_tab_active && pending_ap_refresh.get() {
            let now = Instant::now();
            let s = state.borrow();
            let should_rebuild = last_ap_list_refresh
                .borrow()
                .map(|last| {
                    now.saturating_duration_since(last).as_millis() as u64
                        >= MIN_LIST_REFRESH_INTERVAL_MS
                })
                .unwrap_or(true);
            if should_rebuild {
                let signature = ap_list_signature(
                    &s.access_points,
                    &s.settings,
                    &s.ap_sort,
                    ap_pagination.current_page.get(),
                    ap_pagination.page_size.get(),
                    &pagination_filter_terms(&ap_pagination),
                );
                let changed =
                    last_ap_list_signature.borrow().as_deref() != Some(signature.as_str());
                if changed {
                    *ap_selection_suppressed.borrow_mut() = true;
                    refresh_ap_list(
                        &ap_list,
                        &s.access_points,
                        &s.clients,
                        &s.settings,
                        &s.ap_sort,
                        &ap_pagination,
                        ap_selected_key.borrow().as_deref(),
                        Some(ap_selected_key.clone()),
                    );
                    *ap_selection_suppressed.borrow_mut() = false;
                    *last_ap_list_signature.borrow_mut() = Some(signature);
                }
                *last_ap_list_refresh.borrow_mut() = Some(now);
                pending_ap_refresh.set(false);
            }
        }

        if ap_tab_active {
            let s = state.borrow();
            if let Some(key) = ap_selected_key_now.as_deref() {
                if let Some(ap) = s.access_points.iter().find(|ap| ap.bssid == key) {
                    let detail_signature = ap_detail_signature(ap);
                    let detail_changed = ap_selection_changed
                        || last_ap_detail_signature.borrow().as_deref()
                            != Some(detail_signature.as_str());
                    if ap_selection_changed || detail_changed {
                        sync_wifi_geiger_preview_for_ap_if_idle(&wifi_geiger_state, ap);
                    }
                    if detail_changed {
                        ap_detail_label.set_text(&format_ap_detail_text(ap));
                        set_detail_watchlist_highlight(
                            &ap_detail_label,
                            ap_watchlist_match(ap, &s.settings.watchlists).is_some(),
                        );
                        *ap_selected_packet_mix.borrow_mut() = ap.packet_mix.clone();
                        ap_packet_draw.queue_draw();
                        let notes_text = ap.notes.as_deref().unwrap_or("");
                        let buffer = ap_notes_view.buffer();
                        let current_notes = buffer
                            .text(&buffer.start_iter(), &buffer.end_iter(), true)
                            .to_string();
                        if current_notes != notes_text {
                            buffer.set_text(notes_text);
                        }
                        *last_ap_detail_signature.borrow_mut() = Some(detail_signature);
                    }

                    let assoc_clients = clients_currently_on_ap(&s.clients, &ap.bssid);
                    let assoc_signature = assoc_clients_signature(
                        &assoc_clients,
                        &ap.bssid,
                        &s.access_points,
                        &s.settings.assoc_client_table_layout,
                        &s.assoc_sort,
                        ap_assoc_pagination.current_page.get(),
                        ap_assoc_pagination.page_size.get(),
                        &pagination_filter_terms(&ap_assoc_pagination),
                        &s.settings.watchlists,
                    );
                    let assoc_changed = ap_selection_changed
                        || last_ap_assoc_signature.borrow().as_deref()
                            != Some(assoc_signature.as_str());
                    if assoc_changed {
                        refresh_assoc_client_list(
                            &ap_assoc_list,
                            &ap.bssid,
                            &s.access_points,
                            &assoc_clients,
                            &s.settings.watchlists,
                            &s.settings.assoc_client_table_layout,
                            &s.assoc_sort,
                            &ap_assoc_pagination,
                        );
                        *last_ap_assoc_signature.borrow_mut() = Some(assoc_signature);
                    }
                } else if ap_selection_changed || last_ap_detail_signature.borrow().is_some() {
                    ap_detail_label.set_text("");
                    set_detail_watchlist_highlight(&ap_detail_label, false);
                    ap_notes_view.buffer().set_text("");
                    *ap_selected_packet_mix.borrow_mut() = PacketTypeBreakdown::default();
                    ap_packet_draw.queue_draw();
                    clear_listbox(&ap_assoc_list);
                    clear_wifi_geiger_preview(&wifi_geiger_state);
                    *last_ap_detail_signature.borrow_mut() = None;
                    *last_ap_assoc_signature.borrow_mut() = None;
                }
            } else if ap_selection_changed || last_ap_detail_signature.borrow().is_some() {
                ap_detail_label.set_text("");
                set_detail_watchlist_highlight(&ap_detail_label, false);
                ap_notes_view.buffer().set_text("");
                *ap_selected_packet_mix.borrow_mut() = PacketTypeBreakdown::default();
                ap_packet_draw.queue_draw();
                clear_listbox(&ap_assoc_list);
                clear_wifi_geiger_preview(&wifi_geiger_state);
                *last_ap_detail_signature.borrow_mut() = None;
                *last_ap_assoc_signature.borrow_mut() = None;
            }
        }

        if client_tab_active && pending_client_refresh.get() {
            let now = Instant::now();
            let s = state.borrow();
            let should_rebuild = last_client_list_refresh
                .borrow()
                .map(|last| {
                    now.saturating_duration_since(last).as_millis() as u64
                        >= MIN_LIST_REFRESH_INTERVAL_MS
                })
                .unwrap_or(true);
            if should_rebuild {
                let signature = client_list_signature(
                    &s.clients,
                    &s.access_points,
                    &s.settings,
                    &s.client_sort,
                    client_pagination.current_page.get(),
                    client_pagination.page_size.get(),
                    &pagination_filter_terms(&client_pagination),
                );
                let changed =
                    last_client_list_signature.borrow().as_deref() != Some(signature.as_str());
                if changed {
                    *client_selection_suppressed.borrow_mut() = true;
                    refresh_client_list(
                        &client_list,
                        &s.clients,
                        &s.access_points,
                        &s.settings,
                        &s.client_sort,
                        &client_pagination,
                        client_selected_key.borrow().as_deref(),
                        Some(client_selected_key.clone()),
                    );
                    *client_selection_suppressed.borrow_mut() = false;
                    *last_client_list_signature.borrow_mut() = Some(signature);
                }
                *last_client_list_refresh.borrow_mut() = Some(now);
                pending_client_refresh.set(false);
            }
        }

        if client_tab_active {
            let s = state.borrow();
            if let Some(key) = client_selected_key_now.as_deref() {
                if let Some(client) = s.clients.iter().find(|c| c.mac == key) {
                    let detail_signature = client_detail_signature(client);
                    let detail_changed = client_selection_changed
                        || last_client_detail_signature.borrow().as_deref()
                            != Some(detail_signature.as_str());
                    if client_selection_changed || detail_changed {
                        sync_wifi_geiger_preview_for_client_if_idle(&s, &wifi_geiger_state, client);
                    }
                    if detail_changed {
                        client_detail_label
                            .set_text(&format_client_detail_text(client, &s.access_points));
                        set_detail_watchlist_highlight(
                            &client_detail_label,
                            client_watchlist_match(
                                client,
                                &s.access_points,
                                &s.settings.watchlists,
                            )
                            .is_some(),
                        );
                        *last_client_detail_signature.borrow_mut() = Some(detail_signature);
                    }
                } else if client_selection_changed
                    || last_client_detail_signature.borrow().is_some()
                {
                    client_detail_label.set_text("");
                    set_detail_watchlist_highlight(&client_detail_label, false);
                    clear_wifi_geiger_preview(&wifi_geiger_state);
                    *last_client_detail_signature.borrow_mut() = None;
                }
            } else if client_selection_changed || last_client_detail_signature.borrow().is_some() {
                client_detail_label.set_text("");
                set_detail_watchlist_highlight(&client_detail_label, false);
                clear_wifi_geiger_preview(&wifi_geiger_state);
                *last_client_detail_signature.borrow_mut() = None;
            }
        }

        if bluetooth_tab_active && pending_bluetooth_refresh.get() {
            let now = Instant::now();
            let s = state.borrow();
            let should_rebuild = last_bluetooth_list_refresh
                .borrow()
                .map(|last| {
                    now.saturating_duration_since(last).as_millis() as u64
                        >= MIN_LIST_REFRESH_INTERVAL_MS
                })
                .unwrap_or(true);
            if should_rebuild {
                let signature = bluetooth_list_signature(
                    &s.bluetooth_devices,
                    &s.settings,
                    &s.settings.watchlists,
                    &s.bluetooth_sort,
                    bluetooth_pagination.current_page.get(),
                    bluetooth_pagination.page_size.get(),
                    &pagination_filter_terms(&bluetooth_pagination),
                );
                let changed =
                    last_bluetooth_list_signature.borrow().as_deref() != Some(signature.as_str());
                if changed {
                    *bluetooth_selection_suppressed.borrow_mut() = true;
                    refresh_bluetooth_list(
                        &bluetooth_list,
                        &s.bluetooth_devices,
                        &s.settings,
                        &s.settings.watchlists,
                        &s.bluetooth_sort,
                        &bluetooth_pagination,
                        bluetooth_selected_key.borrow().as_deref(),
                        Some(bluetooth_selected_key.clone()),
                    );
                    *bluetooth_selection_suppressed.borrow_mut() = false;
                    *last_bluetooth_list_signature.borrow_mut() = Some(signature);
                }
                *last_bluetooth_list_refresh.borrow_mut() = Some(now);
                pending_bluetooth_refresh.set(false);
            }
        }

        if bluetooth_tab_active {
            let s = state.borrow();
            if let Some(key) = bluetooth_selected_key_now.as_deref() {
                if let Some(device) = s.bluetooth_devices.iter().find(|d| d.mac == key) {
                    let detail_signature = bluetooth_detail_signature(device);
                    let detail_changed = bluetooth_selection_changed
                        || last_bluetooth_detail_signature.borrow().as_deref()
                            != Some(detail_signature.as_str());
                    if detail_changed {
                        set_bluetooth_detail_sections(
                            device,
                            &bluetooth_identity_label,
                            &bluetooth_passive_label,
                            &bluetooth_active_summary_label,
                            &bluetooth_readable_label,
                            &bluetooth_services_label,
                            &bluetooth_characteristics_label,
                            &bluetooth_descriptors_label,
                        );
                        set_detail_watchlist_highlight(
                            &bluetooth_detail_box,
                            bluetooth_watchlist_match(device, &s.settings.watchlists).is_some(),
                        );
                        *last_bluetooth_detail_signature.borrow_mut() = Some(detail_signature);
                    }
                } else if bluetooth_selection_changed
                    || last_bluetooth_detail_signature.borrow().is_some()
                {
                    clear_bluetooth_detail_sections(
                        &bluetooth_identity_label,
                        &bluetooth_passive_label,
                        &bluetooth_active_summary_label,
                        &bluetooth_readable_label,
                        &bluetooth_services_label,
                        &bluetooth_characteristics_label,
                        &bluetooth_descriptors_label,
                    );
                    set_detail_watchlist_highlight(&bluetooth_detail_box, false);
                    *last_bluetooth_detail_signature.borrow_mut() = None;
                }
            } else if bluetooth_selection_changed
                || last_bluetooth_detail_signature.borrow().is_some()
            {
                clear_bluetooth_detail_sections(
                    &bluetooth_identity_label,
                    &bluetooth_passive_label,
                    &bluetooth_active_summary_label,
                    &bluetooth_readable_label,
                    &bluetooth_services_label,
                    &bluetooth_characteristics_label,
                    &bluetooth_descriptors_label,
                );
                set_detail_watchlist_highlight(&bluetooth_detail_box, false);
                *last_bluetooth_detail_signature.borrow_mut() = None;
            }
        }

        {
            let mut geiger = wifi_geiger_state.borrow_mut();
            let receiver = geiger.receiver.clone();
            if let Some(rx) = receiver {
                for _ in 0..MAX_WIFI_GEIGER_UPDATES_PER_TICK {
                    let Ok(update) = rx.try_recv() else {
                        break;
                    };
                    geiger.latest_update = Some(update.clone());
                    geiger.last_update_at = Some(Instant::now());
                    geiger.target_fraction = normalize_rssi_fraction(update.rssi_dbm);
                    ap_wifi_geiger_meter.queue_draw();
                    client_wifi_geiger_meter.queue_draw();
                    let _ = std::process::Command::new("beep")
                        .arg("-f")
                        .arg(update.tone_hz.to_string())
                        .arg("-l")
                        .arg("35")
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                }
            }
            let should_clear = geiger
                .stop
                .as_ref()
                .map(|stop| stop.load(Ordering::Relaxed))
                .unwrap_or(false);
            if should_clear {
                geiger.receiver = None;
                geiger.stop = None;
            }
            let target_text = geiger
                .target
                .as_ref()
                .map(|target| {
                    format!(
                        "Target: {} | Monitor Channel: {}",
                        target.display_name, target.channel
                    )
                })
                .unwrap_or_else(|| "Target: none selected".to_string());
            let rssi_text = geiger
                .latest_update
                .as_ref()
                .map(|update| format!("RSSI: {} dBm", update.rssi_dbm))
                .unwrap_or_else(|| "RSSI: -- dBm".to_string());
            let tone_text = geiger
                .latest_update
                .as_ref()
                .map(|update| format!("Tone: {} Hz", update.tone_hz))
                .unwrap_or_else(|| "Tone: -- Hz".to_string());
            ap_wifi_geiger_target_label.set_text(&target_text);
            client_wifi_geiger_target_label.set_text(&target_text);
            ap_wifi_geiger_rssi.set_text(&rssi_text);
            client_wifi_geiger_rssi.set_text(&rssi_text);
            ap_wifi_geiger_tone.set_text(&tone_text);
            client_wifi_geiger_tone.set_text(&tone_text);
            ap_wifi_geiger_meter.queue_draw();
            client_wifi_geiger_meter.queue_draw();
        }

        {
            let mut geiger = bluetooth_geiger_state.borrow_mut();
            if let Some(rx) = &geiger.receiver {
                while let Ok(update) = rx.try_recv() {
                    bluetooth_geiger_rssi.set_text(&format!("RSSI: {} dBm", update.rssi_dbm));
                    bluetooth_geiger_tone.set_text(&format!("Tone: {} Hz", update.tone_hz));
                    let fraction = ((update.rssi_dbm + 100) as f64 / 70.0).clamp(0.0, 1.0);
                    bluetooth_geiger_progress.set_fraction(fraction);
                    bluetooth_geiger_progress.set_text(Some(&format!("{:.0}%", fraction * 100.0)));
                    let _ = std::process::Command::new("beep")
                        .arg("-f")
                        .arg(update.tone_hz.to_string())
                        .arg("-l")
                        .arg("35")
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                }
            }
            let should_clear = geiger
                .stop
                .as_ref()
                .map(|stop| stop.load(Ordering::Relaxed))
                .unwrap_or(false);
            if should_clear {
                geiger.receiver = None;
            }
        }

        if channel_tab_active && pending_channel_refresh.get() {
            channel_draw.queue_draw();
            pending_channel_refresh.set(false);
        }

        if sdr_tab_active && pending_sdr_refresh.get() {
            let model = sdr_model.borrow();
            let decode_signature = sdr_decode_signature(
                &model.decode_rows,
                sdr_decode_pagination.current_page.get(),
                sdr_decode_pagination.page_size.get(),
                &pagination_filter_terms(&sdr_decode_pagination),
            );
            let decode_changed =
                last_sdr_decode_signature.borrow().as_deref() != Some(decode_signature.as_str());
            if decode_changed {
                refresh_sdr_decode_list(
                    &sdr_decode_list,
                    &model.decode_rows,
                    &sdr_decode_pagination,
                );
                *last_sdr_decode_signature.borrow_mut() = Some(decode_signature);
            }

            let satcom_signature = sdr_satcom_signature(
                &model.satcom_observations,
                sdr_satcom_pagination.current_page.get(),
                sdr_satcom_pagination.page_size.get(),
                &pagination_filter_terms(&sdr_satcom_pagination),
            );
            let satcom_changed =
                last_sdr_satcom_signature.borrow().as_deref() != Some(satcom_signature.as_str());
            if satcom_changed {
                refresh_sdr_satcom_list(
                    &sdr_satcom_list,
                    &model.satcom_observations,
                    &sdr_satcom_pagination,
                );
                *last_sdr_satcom_signature.borrow_mut() = Some(satcom_signature);
            }
            sdr_fft_draw.queue_draw();
            sdr_spectrogram_draw.queue_draw();
            sdr_map_draw.queue_draw();
            pending_sdr_refresh.set(false);
        }

        {
            let model = sdr_model.borrow();
            let sweep_state = if model.sweep_paused {
                "paused"
            } else {
                "active"
            };
            sdr_frequency_label.set_text(&format!(
                "Center: {} Hz | Sample Rate: {} Hz | Sweep: {} | Satcom Audit Rows: {}",
                model.current_freq_hz,
                model.sample_rate_hz,
                sweep_state,
                model.satcom_observations.len()
            ));
            sdr_decoder_label.set_text(
                &model
                    .decoder_running
                    .as_ref()
                    .map(|decoder| format!("Decoder: running {decoder}"))
                    .unwrap_or_else(|| "Decoder: idle".to_string()),
            );
            sdr_dependency_label.set_text(&format_sdr_dependency_status(&model.dependency_status));
        }

        let (status_text, gps_text, wifi_running, bluetooth_running, scan_transition_in_progress) = {
            let s = state.borrow();
            (
                s.status_text(),
                s.gps_status_text(),
                s.capture_runtime.is_some(),
                s.bluetooth_runtime.is_some(),
                s.scan_start_in_progress || s.scan_stop_in_progress,
            )
        };
        let wifi_lock_text = state.borrow().wifi_lock_status_text();
        ap_wifi_geiger_lock_label.set_text(&format!("Wi-Fi Lock: {}", wifi_lock_text));
        client_wifi_geiger_lock_label.set_text(&format!("Wi-Fi Lock: {}", wifi_lock_text));
        set_scan_control_button_sensitivity(
            &capture_start_btn,
            &capture_stop_btn,
            wifi_running,
            bluetooth_running,
            scan_transition_in_progress,
        );
        let text = status_text;
        status_label.set_text(&text);
        global_status_label.set_text(&text);

        gps_status_label.set_text(&gps_text);
        global_gps_status_label.set_text(&gps_text);

        glib::ControlFlow::Continue
    });
}

fn default_sort_descending(table: SortableTable, column_id: &str) -> bool {
    match table {
        SortableTable::AccessPoints => matches!(
            column_id,
            "channel"
                | "frequency"
                | "rssi"
                | "clients"
                | "first_seen"
                | "last_seen"
                | "handshakes"
                | "uptime"
                | "observation_count"
                | "avg_rssi"
                | "min_rssi"
                | "max_rssi"
                | "packet_total"
        ),
        SortableTable::Clients | SortableTable::AssocClients => matches!(
            column_id,
            "rssi"
                | "first_heard"
                | "last_heard"
                | "data_transferred"
                | "probe_count"
                | "seen_ap_count"
                | "handshake_network_count"
                | "observation_count"
                | "avg_rssi"
                | "min_rssi"
                | "max_rssi"
        ),
        SortableTable::Bluetooth => matches!(column_id, "first_seen" | "last_seen" | "rssi"),
    }
}

fn ap_detail_signature(ap: &AccessPointRecord) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}",
        ap.bssid,
        ap.last_seen.timestamp_millis(),
        ap.rssi_dbm.unwrap_or(i32::MIN),
        ap.number_of_clients,
        ap.handshake_count,
        ap.packet_mix.total(),
        ap.observations.len(),
        ap.notes.as_deref().unwrap_or(""),
        ap.wps.is_some()
    )
}

fn client_detail_signature(client: &ClientRecord) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        client.mac,
        client.last_seen.timestamp_millis(),
        client.rssi_dbm.unwrap_or(i32::MIN),
        client.data_transferred_bytes,
        client.probes.len(),
        client.seen_access_points.len(),
        client.handshake_networks.len(),
        client.observations.len(),
        client.wps.is_some(),
        client.network_intel.retry_frame_count,
        client_network_intel_signature(&client.network_intel),
    )
}

fn client_network_intel_signature(intel: &ClientNetworkIntel) -> String {
    let qos_priorities = intel
        .qos_priorities
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",");

    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        intel.packet_mix.management,
        intel.packet_mix.control,
        intel.packet_mix.data,
        intel.packet_mix.other,
        intel.uplink_bytes,
        intel.downlink_bytes,
        intel.retry_frame_count,
        intel.power_save_observed,
        qos_priorities,
        intel.eapol_frame_count,
        intel.pmkid_count,
        intel.last_reason_code.unwrap_or_default(),
        intel.last_status_code.unwrap_or_default(),
    )
}

fn bluetooth_detail_signature(device: &BluetoothDeviceRecord) -> String {
    let active = device.active_enumeration.as_ref();
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        device.mac,
        device.last_seen.timestamp_millis(),
        device.rssi_dbm.unwrap_or(i32::MIN),
        device.mfgr_ids.len(),
        device.uuids.len(),
        device.observations.len(),
        device.device_type.as_deref().unwrap_or(""),
        active
            .and_then(|entry| entry.last_enumerated)
            .map(|value| value.timestamp_millis())
            .unwrap_or_default(),
        active.map(|entry| entry.services.len()).unwrap_or_default(),
        active
            .map(|entry| entry.characteristics.len())
            .unwrap_or_default(),
        active
            .map(|entry| entry.descriptors.len())
            .unwrap_or_default(),
        active
            .map(|entry| entry.readable_attributes.len())
            .unwrap_or_default(),
        active
            .and_then(|entry| entry.last_error.as_deref())
            .unwrap_or(""),
    )
}

fn assoc_clients_signature(
    clients: &[ClientRecord],
    ap_bssid: &str,
    aps: &[AccessPointRecord],
    layout: &TableLayout,
    sort: &TableSortState,
    current_page: usize,
    page_size: usize,
    filters: &[(String, String)],
    watchlists: &WatchlistSettings,
) -> String {
    let mut sorted = clients_currently_on_ap(clients, ap_bssid);
    sort_assoc_clients(&mut sorted, ap_bssid, aps, sort, watchlists);
    let visible_columns = layout
        .columns
        .iter()
        .filter(|c| c.visible)
        .map(|c| c.id.as_str())
        .collect::<Vec<_>>();
    let filtered = sorted
        .into_iter()
        .filter(|client| {
            row_matches_column_filters(filters, |column_id| {
                assoc_client_column_value_with_watchlist(
                    client, ap_bssid, aps, column_id, watchlists,
                )
            })
        })
        .collect::<Vec<_>>();
    let newest = filtered
        .iter()
        .map(|client| client.last_seen.timestamp_millis())
        .max()
        .unwrap_or_default();
    let bytes = filtered
        .iter()
        .map(|client| client.data_transferred_bytes)
        .sum::<u64>();
    let probe_total = filtered
        .iter()
        .map(|client| client.probes.len())
        .sum::<usize>();
    let current_count = filtered.len();
    let total_items = filtered.len();
    let (current_page, _, start, end) = paged_indices(total_items, current_page, page_size);
    let status_signature = filtered[start..end]
        .iter()
        .map(|client| {
            let row_values = visible_columns
                .iter()
                .filter_map(|column_id| {
                    assoc_client_column_value_with_watchlist(
                        client, ap_bssid, aps, column_id, watchlists,
                    )
                })
                .collect::<Vec<_>>()
                .join("\u{1f}");
            let status = if client.associated_ap.as_deref() == Some(ap_bssid) {
                "current"
            } else {
                "historical"
            };
            format!("{}:{}:{}", client.mac, status, row_values)
        })
        .collect::<Vec<_>>()
        .join("\u{1e}");
    format!(
        "{}|{}|{}|{}|{}|page={}|size={}|search={}",
        total_items,
        newest,
        bytes + probe_total as u64,
        current_count,
        status_signature,
        current_page,
        page_size,
        pagination_filter_signature(filters)
    )
}

fn cmp_option_string(a: Option<&str>, b: Option<&str>) -> std::cmp::Ordering {
    a.unwrap_or("")
        .to_ascii_lowercase()
        .cmp(&b.unwrap_or("").to_ascii_lowercase())
}

fn cmp_option_ord<T: Ord>(a: Option<T>, b: Option<T>) -> std::cmp::Ordering {
    a.cmp(&b)
}

fn rssi_stats(
    observations: &[GeoObservation],
    fallback: Option<i32>,
) -> (Option<i32>, Option<i32>, Option<i32>, usize) {
    let mut values = observations
        .iter()
        .filter_map(|obs| obs.rssi_dbm)
        .collect::<Vec<_>>();
    if values.is_empty() {
        if let Some(value) = fallback {
            values.push(value);
        }
    }
    if values.is_empty() {
        return (None, None, None, 0);
    }
    let sum = values.iter().copied().map(i64::from).sum::<i64>();
    let avg = (sum as f64 / values.len() as f64).round() as i32;
    let min = values.iter().copied().min();
    let max = values.iter().copied().max();
    (Some(avg), min, max, values.len())
}

fn sort_access_points(
    aps: &mut [AccessPointRecord],
    sort: &TableSortState,
    watchlists: &WatchlistSettings,
) {
    aps.sort_by(|a, b| {
        let order = match sort.column_id.as_str() {
            "watchlist_entry" => ap_watchlist_entry_value(a, watchlists)
                .cmp(&ap_watchlist_entry_value(b, watchlists)),
            "ssid" => cmp_option_string(a.ssid.as_deref(), b.ssid.as_deref()),
            "bssid" => a.bssid.cmp(&b.bssid),
            "oui" => {
                cmp_option_string(a.oui_manufacturer.as_deref(), b.oui_manufacturer.as_deref())
            }
            "channel" => cmp_option_ord(a.channel, b.channel),
            "encryption" => a.encryption_short.cmp(&b.encryption_short),
            "rssi" => cmp_option_ord(a.rssi_dbm, b.rssi_dbm),
            "wps" => a.wps.is_some().cmp(&b.wps.is_some()),
            "clients" => a.number_of_clients.cmp(&b.number_of_clients),
            "first_seen" => a.first_seen.cmp(&b.first_seen),
            "last_seen" => a.last_seen.cmp(&b.last_seen),
            "handshakes" => a.handshake_count.cmp(&b.handshake_count),
            "band" => a.band.label().cmp(b.band.label()),
            "frequency" => cmp_option_ord(a.frequency_mhz, b.frequency_mhz),
            "country" => cmp_option_string(
                a.country_code_80211d.as_deref(),
                b.country_code_80211d.as_deref(),
            ),
            "full_encryption" => a.encryption_full.cmp(&b.encryption_full),
            "hidden_ssid" => ap_hidden_ssid(a).cmp(&ap_hidden_ssid(b)),
            "uptime" => cmp_option_ord(a.uptime_beacons, b.uptime_beacons),
            "observation_count" => a.observations.len().cmp(&b.observations.len()),
            "avg_rssi" => cmp_option_ord(
                rssi_stats(&a.observations, a.rssi_dbm).0,
                rssi_stats(&b.observations, b.rssi_dbm).0,
            ),
            "min_rssi" => cmp_option_ord(
                rssi_stats(&a.observations, a.rssi_dbm).1,
                rssi_stats(&b.observations, b.rssi_dbm).1,
            ),
            "max_rssi" => cmp_option_ord(
                rssi_stats(&a.observations, a.rssi_dbm).2,
                rssi_stats(&b.observations, b.rssi_dbm).2,
            ),
            "packet_total" => a.packet_mix.total().cmp(&b.packet_mix.total()),
            "notes" => cmp_option_string(a.notes.as_deref(), b.notes.as_deref()),
            "first_location" => cmp_option_ord(
                observation_highlights(&a.observations)
                    .first
                    .map(|obs| obs.timestamp),
                observation_highlights(&b.observations)
                    .first
                    .map(|obs| obs.timestamp),
            ),
            "last_location" => cmp_option_ord(
                observation_highlights(&a.observations)
                    .last
                    .map(|obs| obs.timestamp),
                observation_highlights(&b.observations)
                    .last
                    .map(|obs| obs.timestamp),
            ),
            "strongest_location" => cmp_option_ord(
                observation_highlights(&a.observations)
                    .strongest
                    .and_then(|obs| obs.rssi_dbm),
                observation_highlights(&b.observations)
                    .strongest
                    .and_then(|obs| obs.rssi_dbm),
            ),
            _ => a.last_seen.cmp(&b.last_seen),
        };
        let order = if sort.descending {
            order.reverse()
        } else {
            order
        };
        order
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.bssid.cmp(&b.bssid))
    });
}

fn sort_clients(
    clients: &mut [ClientRecord],
    aps: &[AccessPointRecord],
    sort: &TableSortState,
    watchlists: &WatchlistSettings,
) {
    clients.sort_by(|a, b| {
        let order = match sort.column_id.as_str() {
            "watchlist_entry" => client_watchlist_entry_value(a, aps, watchlists)
                .cmp(&client_watchlist_entry_value(b, aps, watchlists)),
            "mac" => a.mac.cmp(&b.mac),
            "oui" => {
                cmp_option_string(a.oui_manufacturer.as_deref(), b.oui_manufacturer.as_deref())
            }
            "associated_ap" => {
                cmp_option_string(a.associated_ap.as_deref(), b.associated_ap.as_deref())
            }
            "associated_ssid" => cmp_option_string(
                associated_ssid_for_client(aps, a).as_deref(),
                associated_ssid_for_client(aps, b).as_deref(),
            ),
            "rssi" => cmp_option_ord(a.rssi_dbm, b.rssi_dbm),
            "wps" => a.wps.is_some().cmp(&b.wps.is_some()),
            "probes" => a.probes.join(",").cmp(&b.probes.join(",")),
            "first_heard" | "first_seen" => a.first_seen.cmp(&b.first_seen),
            "last_heard" | "last_seen" => a.last_seen.cmp(&b.last_seen),
            "data_transferred" => a.data_transferred_bytes.cmp(&b.data_transferred_bytes),
            "probe_count" => a.probes.len().cmp(&b.probes.len()),
            "seen_ap_count" => a.seen_access_points.len().cmp(&b.seen_access_points.len()),
            "handshake_network_count" => {
                a.handshake_networks.len().cmp(&b.handshake_networks.len())
            }
            "observation_count" => a.observations.len().cmp(&b.observations.len()),
            "avg_rssi" => cmp_option_ord(
                rssi_stats(&a.observations, a.rssi_dbm).0,
                rssi_stats(&b.observations, b.rssi_dbm).0,
            ),
            "min_rssi" => cmp_option_ord(
                rssi_stats(&a.observations, a.rssi_dbm).1,
                rssi_stats(&b.observations, b.rssi_dbm).1,
            ),
            "max_rssi" => cmp_option_ord(
                rssi_stats(&a.observations, a.rssi_dbm).2,
                rssi_stats(&b.observations, b.rssi_dbm).2,
            ),
            "seen_aps" => a
                .seen_access_points
                .join(",")
                .cmp(&b.seen_access_points.join(",")),
            "handshake_networks" => a
                .handshake_networks
                .join(",")
                .cmp(&b.handshake_networks.join(",")),
            "band" => a
                .network_intel
                .band
                .label()
                .cmp(b.network_intel.band.label()),
            "channel" => cmp_option_ord(a.network_intel.last_channel, b.network_intel.last_channel),
            "frequency" => cmp_option_ord(
                a.network_intel.last_frequency_mhz,
                b.network_intel.last_frequency_mhz,
            ),
            "uplink_bytes" => a
                .network_intel
                .uplink_bytes
                .cmp(&b.network_intel.uplink_bytes),
            "downlink_bytes" => a
                .network_intel
                .downlink_bytes
                .cmp(&b.network_intel.downlink_bytes),
            "retry_count" => a
                .network_intel
                .retry_frame_count
                .cmp(&b.network_intel.retry_frame_count),
            "retry_rate" => retry_rate_text(a).cmp(&retry_rate_text(b)),
            "power_save" => a
                .network_intel
                .power_save_observed
                .cmp(&b.network_intel.power_save_observed),
            "eapol_frames" => a
                .network_intel
                .eapol_frame_count
                .cmp(&b.network_intel.eapol_frame_count),
            "pmkid_count" => a
                .network_intel
                .pmkid_count
                .cmp(&b.network_intel.pmkid_count),
            "first_location" => cmp_option_ord(
                observation_highlights(&a.observations)
                    .first
                    .map(|obs| obs.timestamp),
                observation_highlights(&b.observations)
                    .first
                    .map(|obs| obs.timestamp),
            ),
            "last_location" => cmp_option_ord(
                observation_highlights(&a.observations)
                    .last
                    .map(|obs| obs.timestamp),
                observation_highlights(&b.observations)
                    .last
                    .map(|obs| obs.timestamp),
            ),
            "strongest_location" => cmp_option_ord(
                observation_highlights(&a.observations)
                    .strongest
                    .and_then(|obs| obs.rssi_dbm),
                observation_highlights(&b.observations)
                    .strongest
                    .and_then(|obs| obs.rssi_dbm),
            ),
            _ => a.last_seen.cmp(&b.last_seen),
        };
        let order = if sort.descending {
            order.reverse()
        } else {
            order
        };
        order
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.mac.cmp(&b.mac))
    });
}

fn sort_assoc_clients(
    clients: &mut [ClientRecord],
    ap_bssid: &str,
    aps: &[AccessPointRecord],
    sort: &TableSortState,
    watchlists: &WatchlistSettings,
) {
    clients.sort_by(|a, b| {
        let order = match sort.column_id.as_str() {
            "status" => (a.associated_ap.as_deref() == Some(ap_bssid))
                .cmp(&(b.associated_ap.as_deref() == Some(ap_bssid))),
            "current_ap" => {
                cmp_option_string(a.associated_ap.as_deref(), b.associated_ap.as_deref())
            }
            "current_ssid" => cmp_option_string(
                associated_ssid_for_client(aps, a).as_deref(),
                associated_ssid_for_client(aps, b).as_deref(),
            ),
            _ => {
                let mut copy = vec![a.clone(), b.clone()];
                sort_clients(&mut copy, aps, sort, watchlists);
                if copy.first().map(|client| client.mac.as_str()) == Some(a.mac.as_str()) {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            }
        };
        let order = if sort.descending {
            order.reverse()
        } else {
            order
        };
        order
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.mac.cmp(&b.mac))
    });
}

fn sort_bluetooth_devices(
    devices: &mut [BluetoothDeviceRecord],
    sort: &TableSortState,
    watchlists: &WatchlistSettings,
) {
    devices.sort_by(|a, b| {
        let order = match sort.column_id.as_str() {
            "watchlist_entry" => bluetooth_watchlist_entry_value(a, watchlists)
                .cmp(&bluetooth_watchlist_entry_value(b, watchlists)),
            "transport" => a.transport.cmp(&b.transport),
            "mac" => a.mac.cmp(&b.mac),
            "oui" => {
                cmp_option_string(a.oui_manufacturer.as_deref(), b.oui_manufacturer.as_deref())
            }
            "name" => bluetooth_display_name(a)
                .to_ascii_lowercase()
                .cmp(&bluetooth_display_name(b).to_ascii_lowercase()),
            "type" => cmp_option_string(a.device_type.as_deref(), b.device_type.as_deref()),
            "rssi" => cmp_option_ord(a.rssi_dbm, b.rssi_dbm),
            "advertised_name" => {
                cmp_option_string(a.advertised_name.as_deref(), b.advertised_name.as_deref())
            }
            "alias" => cmp_option_string(a.alias.as_deref(), b.alias.as_deref()),
            "address_type" => {
                cmp_option_string(a.address_type.as_deref(), b.address_type.as_deref())
            }
            "class_of_device" => {
                cmp_option_string(a.class_of_device.as_deref(), b.class_of_device.as_deref())
            }
            "mfgr_ids" => a.mfgr_ids.join(",").cmp(&b.mfgr_ids.join(",")),
            "mfgr_names" => a.mfgr_names.join(",").cmp(&b.mfgr_names.join(",")),
            "uuids" => a.uuid_names.join(",").cmp(&b.uuid_names.join(",")),
            "first_seen" => a.first_seen.cmp(&b.first_seen),
            "last_seen" => a.last_seen.cmp(&b.last_seen),
            _ => a.last_seen.cmp(&b.last_seen),
        };
        let order = if sort.descending {
            order.reverse()
        } else {
            order
        };
        order
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.mac.cmp(&b.mac))
    });
}

fn ap_list_signature(
    aps: &[AccessPointRecord],
    settings: &AppSettings,
    sort: &TableSortState,
    current_page: usize,
    page_size: usize,
    filters: &[(String, String)],
) -> String {
    let mut sorted = aps.to_vec();
    sort_access_points(&mut sorted, sort, &settings.watchlists);
    let filtered = sorted
        .into_iter()
        .filter(|ap| {
            row_matches_column_filters(filters, |column_id| {
                ap_column_value_with_watchlist(ap, column_id, &settings.watchlists)
            })
        })
        .collect::<Vec<_>>();
    let total_items = filtered.len();
    let (current_page, _, start, end) = paged_indices(total_items, current_page, page_size);
    let visible_columns = settings
        .ap_table_layout
        .columns
        .iter()
        .filter(|c| c.visible)
        .map(|c| c.id.as_str())
        .collect::<Vec<_>>();

    std::iter::once(format!(
        "page={current_page}|size={page_size}|total={total_items}|filters={}",
        pagination_filter_signature(filters)
    ))
    .chain(filtered[start..end].iter().map(|ap| {
        let row_values = visible_columns
            .iter()
            .filter_map(|column_id| {
                ap_column_value_with_watchlist(ap, column_id, &settings.watchlists)
            })
            .collect::<Vec<_>>()
            .join("\u{1f}");
        format!(
            "{}|{}|{}|{}",
            ap.bssid,
            ap_watchlist_match(ap, &settings.watchlists).is_some(),
            ap.handshake_count > 0,
            row_values
        )
    }))
    .collect::<Vec<_>>()
    .join("\u{1e}")
}

fn client_list_signature(
    clients: &[ClientRecord],
    aps: &[AccessPointRecord],
    settings: &AppSettings,
    sort: &TableSortState,
    current_page: usize,
    page_size: usize,
    filters: &[(String, String)],
) -> String {
    let mut sorted = clients.to_vec();
    sort_clients(&mut sorted, aps, sort, &settings.watchlists);
    let filtered = sorted
        .into_iter()
        .filter(|client| {
            row_matches_column_filters(filters, |column_id| {
                client_column_value_with_watchlist(client, aps, column_id, &settings.watchlists)
            })
        })
        .collect::<Vec<_>>();
    let total_items = filtered.len();
    let (current_page, _, start, end) = paged_indices(total_items, current_page, page_size);
    let visible_columns = settings
        .client_table_layout
        .columns
        .iter()
        .filter(|c| c.visible)
        .map(|c| c.id.as_str())
        .collect::<Vec<_>>();

    std::iter::once(format!(
        "page={current_page}|size={page_size}|total={total_items}|filters={}",
        pagination_filter_signature(filters)
    ))
    .chain(filtered[start..end].iter().map(|client| {
        let row_values = visible_columns
            .iter()
            .filter_map(|column_id| {
                client_column_value_with_watchlist(client, aps, column_id, &settings.watchlists)
            })
            .collect::<Vec<_>>()
            .join("\u{1f}");
        format!(
            "{}|{}|{}",
            client.mac,
            client_watchlist_match(client, aps, &settings.watchlists).is_some(),
            row_values
        )
    }))
    .collect::<Vec<_>>()
    .join("\u{1e}")
}

fn bluetooth_list_signature(
    devices: &[BluetoothDeviceRecord],
    settings: &AppSettings,
    watchlists: &WatchlistSettings,
    sort: &TableSortState,
    current_page: usize,
    page_size: usize,
    filters: &[(String, String)],
) -> String {
    let mut sorted = devices.to_vec();
    sort_bluetooth_devices(&mut sorted, sort, watchlists);
    let visible_columns = settings
        .bluetooth_table_layout
        .columns
        .iter()
        .filter(|c| c.visible)
        .map(|c| c.id.as_str())
        .collect::<Vec<_>>();
    let filtered = sorted
        .into_iter()
        .filter(|device| {
            row_matches_column_filters(filters, |column_id| {
                bluetooth_column_value_with_watchlist(device, column_id, watchlists)
            })
        })
        .collect::<Vec<_>>();
    let total_items = filtered.len();
    let (current_page, _, start, end) = paged_indices(total_items, current_page, page_size);
    let page_window = format!(
        "page={current_page}|size={page_size}|total={total_items}|filters={}",
        pagination_filter_signature(filters)
    );
    std::iter::once(page_window)
        .chain(filtered[start..end].iter().map(|device| {
            let row_values = visible_columns
                .iter()
                .filter_map(|column_id| {
                    bluetooth_column_value_with_watchlist(device, column_id, watchlists)
                })
                .collect::<Vec<_>>()
                .join("\u{1f}");
            format!(
                "{}|{}|{}",
                device.mac,
                bluetooth_watchlist_match(device, watchlists).is_some(),
                row_values
            )
        }))
        .collect::<Vec<_>>()
        .join("\u{1e}")
}

fn paged_indices(
    total_items: usize,
    requested_page: usize,
    page_size: usize,
) -> (usize, usize, usize, usize) {
    let page_size = page_size.max(1);
    let total_pages = total_items.max(1).div_ceil(page_size);
    let current_page = requested_page.min(total_pages.saturating_sub(1));
    let start = current_page.saturating_mul(page_size).min(total_items);
    let end = (start + page_size).min(total_items);
    (current_page, total_pages, start, end)
}

fn update_table_pagination_summary(
    pagination: &TablePaginationUi,
    total_items: usize,
    current_page: usize,
    total_pages: usize,
    start: usize,
    end: usize,
) {
    pagination.current_page.set(current_page);
    pagination
        .prev_button
        .set_sensitive(current_page > 0 && total_items > 0);
    pagination
        .next_button
        .set_sensitive(current_page + 1 < total_pages && total_items > 0);
    pagination.page_go_button.set_sensitive(total_items > 0);
    let shown = if total_items == 0 {
        "Showing 0 of 0".to_string()
    } else {
        format!("Showing {}-{} of {}", start + 1, end, total_items)
    };
    let current_page_text = (current_page + 1).to_string();
    if pagination.page_entry.text().as_str() != current_page_text {
        pagination.page_entry.set_text(&current_page_text);
    }
    pagination.summary_label.set_text(&format!(
        "{shown} | Page {} of {}",
        current_page + 1,
        total_pages
    ));
}

fn pagination_filter_terms(pagination: &TablePaginationUi) -> Vec<(String, String)> {
    let entries = pagination.filter_entries.borrow();
    let mut filters = pagination
        .filter_order
        .iter()
        .filter_map(|column_id| {
            let value = entries.get(column_id)?.text().trim().to_string();
            if value.is_empty() {
                None
            } else {
                Some((column_id.clone(), value.to_ascii_lowercase()))
            }
        })
        .collect::<Vec<_>>();
    filters.sort_by(|a, b| a.0.cmp(&b.0));
    filters
}

fn pagination_filter_signature(filters: &[(String, String)]) -> String {
    filters
        .iter()
        .map(|(column, value)| format!("{column}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn row_matches_column_filters(
    filters: &[(String, String)],
    value_for: impl Fn(&str) -> Option<String>,
) -> bool {
    filters.iter().all(|(column_id, needle)| {
        let normalized_needle = needle.to_ascii_lowercase();
        value_for(column_id)
            .map(|value| value.to_ascii_lowercase().contains(&normalized_needle))
            .unwrap_or(false)
    })
}

fn focus_first_filter_entry(pagination: &TablePaginationUi) {
    let entries = pagination.filter_entries.borrow();
    for column_id in pagination.filter_order.iter() {
        if let Some(entry) = entries.get(column_id) {
            entry.grab_focus();
            entry.select_region(0, -1);
            break;
        }
    }
}

fn refresh_ap_list(
    list: &ListBox,
    aps: &[AccessPointRecord],
    _clients: &[ClientRecord],
    settings: &AppSettings,
    sort: &TableSortState,
    pagination: &TablePaginationUi,
    selected_key_override: Option<&str>,
    selected_key_state: Option<Rc<RefCell<Option<String>>>>,
) {
    let selected_key = selected_key_override
        .map(str::to_string)
        .or_else(|| selected_row_key(list));
    clear_listbox(list);

    let filters = pagination_filter_terms(pagination);
    let mut sorted = aps.to_vec();
    sort_access_points(&mut sorted, sort, &settings.watchlists);
    let watchlist_classes = watchlist_css_classes(&settings.watchlists);
    let filtered = sorted
        .into_iter()
        .filter(|ap| {
            row_matches_column_filters(&filters, |column_id| {
                ap_column_value_with_watchlist(ap, column_id, &settings.watchlists)
            })
        })
        .collect::<Vec<_>>();
    let total_items = filtered.len();
    let page_size = pagination.page_size.get();
    let (current_page, total_pages, start, end) =
        paged_indices(total_items, pagination.current_page.get(), page_size);

    for ap in filtered
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
    {
        let row = ListBoxRow::new();
        row.set_widget_name(&ap.bssid);
        attach_row_click_selection(&row, list, selected_key_state.clone());
        let line = GtkBox::new(Orientation::Horizontal, 14);
        line.set_hexpand(true);
        let watchlist_match = ap_watchlist_match(&ap, &settings.watchlists);
        set_row_alert_classes(
            &row,
            &line,
            watchlist_match
                .as_ref()
                .map(|matched| matched.css_class.as_str()),
            &watchlist_classes,
            ap.handshake_count > 0,
        );
        for column in settings
            .ap_table_layout
            .columns
            .iter()
            .filter(|c| c.visible)
        {
            if let Some(value) =
                ap_column_value_with_watchlist(&ap, &column.id, &settings.watchlists)
            {
                line.append(&label_cell(value, column.width_chars));
            }
        }
        row.set_child(Some(&line));
        list.append(&row);
    }

    update_table_pagination_summary(
        pagination,
        total_items,
        current_page,
        total_pages,
        start,
        end,
    );
    restore_listbox_selection(list, selected_key.as_deref());
}

fn drain_capture_events_batch(
    receiver: &Receiver<CaptureEvent>,
    limit: usize,
) -> Vec<CaptureEvent> {
    let mut latest_aps: HashMap<String, AccessPointRecord> = HashMap::new();
    let mut latest_clients: HashMap<String, ClientRecord> = HashMap::new();
    let mut latest_usage: HashMap<u16, ChannelUsagePoint> = HashMap::new();
    let mut observations: Vec<(String, String, GeoObservation)> = Vec::new();
    let mut handshakes = Vec::new();
    let mut logs = Vec::new();

    for _ in 0..limit {
        let Ok(event) = receiver.try_recv() else {
            break;
        };
        match event {
            CaptureEvent::AccessPointSeen(ap) => {
                latest_aps.insert(ap.bssid.clone(), ap);
            }
            CaptureEvent::ClientSeen(client) => {
                latest_clients.insert(client.mac.clone(), client);
            }
            CaptureEvent::Observation {
                device_type,
                device_id,
                observation,
            } => observations.push((device_type, device_id, observation)),
            CaptureEvent::HandshakeSeen(handshake) => handshakes.push(handshake),
            CaptureEvent::ChannelUsage(usage) => {
                latest_usage.insert(usage.channel, usage);
            }
            CaptureEvent::Log(text) => logs.push(text),
        }
    }

    let mut events = Vec::with_capacity(
        logs.len()
            + latest_aps.len()
            + latest_clients.len()
            + observations.len()
            + handshakes.len()
            + latest_usage.len(),
    );
    events.extend(logs.into_iter().map(CaptureEvent::Log));
    events.extend(latest_aps.into_values().map(CaptureEvent::AccessPointSeen));
    events.extend(latest_clients.into_values().map(CaptureEvent::ClientSeen));
    events.extend(
        observations
            .into_iter()
            .map(
                |(device_type, device_id, observation)| CaptureEvent::Observation {
                    device_type,
                    device_id,
                    observation,
                },
            ),
    );
    events.extend(handshakes.into_iter().map(CaptureEvent::HandshakeSeen));
    events.extend(latest_usage.into_values().map(CaptureEvent::ChannelUsage));
    events
}

fn drain_sdr_events_batch(receiver: &Receiver<SdrEvent>, limit: usize) -> Vec<SdrEvent> {
    let mut logs: Vec<String> = Vec::new();
    let mut latest_freq: Option<u64> = None;
    let mut latest_spectrum: Option<SdrSpectrumFrame> = None;
    let mut decode_rows: Vec<SdrDecodeRow> = Vec::new();
    let mut map_points: Vec<SdrMapPoint> = Vec::new();
    let mut satcom_observations: Vec<SdrSatcomObservation> = Vec::new();
    let mut latest_decoder_state: Option<(bool, Option<String>)> = None;
    let mut latest_dependencies: Option<Vec<SdrDependencyStatus>> = None;
    let mut latest_squelch: Option<f32> = None;

    for _ in 0..limit {
        let Ok(event) = receiver.try_recv() else {
            break;
        };
        match event {
            SdrEvent::Log(text) => logs.push(text),
            SdrEvent::FrequencyChanged(freq_hz) => latest_freq = Some(freq_hz),
            SdrEvent::SpectrumFrame(frame) => latest_spectrum = Some(frame),
            SdrEvent::DecodeRow(row) => decode_rows.push(row),
            SdrEvent::DecoderState { running, decoder } => {
                latest_decoder_state = Some((running, decoder));
            }
            SdrEvent::DependencyStatus(status) => latest_dependencies = Some(status),
            SdrEvent::MapPoint(point) => map_points.push(point),
            SdrEvent::SatcomObservation(observation) => satcom_observations.push(observation),
            SdrEvent::SquelchChanged(value) => latest_squelch = Some(value),
        }
    }

    let mut events = Vec::with_capacity(
        logs.len()
            + decode_rows.len()
            + map_points.len()
            + satcom_observations.len()
            + usize::from(latest_freq.is_some())
            + usize::from(latest_spectrum.is_some())
            + usize::from(latest_decoder_state.is_some())
            + usize::from(latest_dependencies.is_some())
            + usize::from(latest_squelch.is_some()),
    );

    events.extend(logs.into_iter().map(SdrEvent::Log));
    if let Some(freq_hz) = latest_freq {
        events.push(SdrEvent::FrequencyChanged(freq_hz));
    }
    if let Some(frame) = latest_spectrum {
        events.push(SdrEvent::SpectrumFrame(frame));
    }
    events.extend(decode_rows.into_iter().map(SdrEvent::DecodeRow));
    if let Some((running, decoder)) = latest_decoder_state {
        events.push(SdrEvent::DecoderState { running, decoder });
    }
    if let Some(status) = latest_dependencies {
        events.push(SdrEvent::DependencyStatus(status));
    }
    events.extend(map_points.into_iter().map(SdrEvent::MapPoint));
    events.extend(
        satcom_observations
            .into_iter()
            .map(SdrEvent::SatcomObservation),
    );
    if let Some(value) = latest_squelch {
        events.push(SdrEvent::SquelchChanged(value));
    }

    events
}

fn refresh_client_list(
    list: &ListBox,
    clients: &[ClientRecord],
    aps: &[AccessPointRecord],
    settings: &AppSettings,
    sort: &TableSortState,
    pagination: &TablePaginationUi,
    selected_key_override: Option<&str>,
    selected_key_state: Option<Rc<RefCell<Option<String>>>>,
) {
    let selected_key = selected_key_override
        .map(str::to_string)
        .or_else(|| selected_row_key(list));
    clear_listbox(list);

    let filters = pagination_filter_terms(pagination);
    let mut sorted = clients.to_vec();
    sort_clients(&mut sorted, aps, sort, &settings.watchlists);
    let watchlist_classes = watchlist_css_classes(&settings.watchlists);
    let filtered = sorted
        .into_iter()
        .filter(|client| {
            row_matches_column_filters(&filters, |column_id| {
                client_column_value_with_watchlist(client, aps, column_id, &settings.watchlists)
            })
        })
        .collect::<Vec<_>>();
    let total_items = filtered.len();
    let page_size = pagination.page_size.get();
    let (current_page, total_pages, start, end) =
        paged_indices(total_items, pagination.current_page.get(), page_size);

    for client in filtered
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
    {
        let row = ListBoxRow::new();
        row.set_widget_name(&client.mac);
        attach_row_click_selection(&row, list, selected_key_state.clone());
        let line = GtkBox::new(Orientation::Horizontal, 14);
        line.set_hexpand(true);
        let watchlist_match = client_watchlist_match(&client, aps, &settings.watchlists);
        set_row_alert_classes(
            &row,
            &line,
            watchlist_match
                .as_ref()
                .map(|matched| matched.css_class.as_str()),
            &watchlist_classes,
            false,
        );
        for column in settings
            .client_table_layout
            .columns
            .iter()
            .filter(|c| c.visible)
        {
            if let Some(value) =
                client_column_value_with_watchlist(&client, aps, &column.id, &settings.watchlists)
            {
                line.append(&label_cell(value, column.width_chars));
            }
        }
        row.set_child(Some(&line));
        list.append(&row);
    }

    update_table_pagination_summary(
        pagination,
        total_items,
        current_page,
        total_pages,
        start,
        end,
    );
    restore_listbox_selection(list, selected_key.as_deref());
}

fn drain_bluetooth_events_batch(
    receiver: &Receiver<BluetoothEvent>,
    limit: usize,
) -> Vec<BluetoothEvent> {
    let mut latest_devices: HashMap<String, BluetoothDeviceRecord> = HashMap::new();
    let mut logs = Vec::new();

    for _ in 0..limit {
        let Ok(event) = receiver.try_recv() else {
            break;
        };
        match event {
            BluetoothEvent::DeviceSeen(device) => {
                latest_devices.insert(device.mac.clone(), device);
            }
            BluetoothEvent::Log(text) => logs.push(text),
        }
    }

    let mut events = Vec::with_capacity(logs.len() + latest_devices.len());
    events.extend(logs.into_iter().map(BluetoothEvent::Log));
    events.extend(latest_devices.into_values().map(BluetoothEvent::DeviceSeen));
    events
}

fn refresh_assoc_client_list(
    list: &ListBox,
    ap_bssid: &str,
    aps: &[AccessPointRecord],
    clients: &[ClientRecord],
    watchlists: &WatchlistSettings,
    layout: &TableLayout,
    sort: &TableSortState,
    pagination: &TablePaginationUi,
) {
    clear_listbox(list);
    let filters = pagination_filter_terms(pagination);
    let mut sorted = clients_currently_on_ap(clients, ap_bssid);
    sort_assoc_clients(&mut sorted, ap_bssid, aps, sort, watchlists);
    let no_watchlist_classes: Vec<String> = Vec::new();
    let filtered = sorted
        .into_iter()
        .filter(|client| {
            row_matches_column_filters(&filters, |column_id| {
                assoc_client_column_value_with_watchlist(
                    client, ap_bssid, aps, column_id, watchlists,
                )
            })
        })
        .collect::<Vec<_>>();
    let total_items = filtered.len();
    let page_size = pagination.page_size.get();
    let (current_page, total_pages, start, end) =
        paged_indices(total_items, pagination.current_page.get(), page_size);
    for client in filtered.iter().skip(start).take(end.saturating_sub(start)) {
        let row = ListBoxRow::new();
        attach_row_click_selection(&row, list, None);
        let line = GtkBox::new(Orientation::Horizontal, 14);
        line.set_hexpand(true);
        set_row_alert_classes(&row, &line, None, &no_watchlist_classes, false);
        for column in layout.columns.iter().filter(|c| c.visible) {
            if let Some(value) = assoc_client_column_value_with_watchlist(
                client, ap_bssid, aps, &column.id, watchlists,
            ) {
                line.append(&label_cell(value, column.width_chars));
            }
        }
        row.set_child(Some(&line));
        list.append(&row);
    }
    update_table_pagination_summary(
        pagination,
        total_items,
        current_page,
        total_pages,
        start,
        end,
    );
}

fn refresh_bluetooth_list(
    list: &ListBox,
    devices: &[BluetoothDeviceRecord],
    settings: &AppSettings,
    watchlists: &WatchlistSettings,
    sort: &TableSortState,
    pagination: &TablePaginationUi,
    selected_key_override: Option<&str>,
    selected_key_state: Option<Rc<RefCell<Option<String>>>>,
) {
    let selected_key = selected_key_override
        .map(str::to_string)
        .or_else(|| selected_row_key(list));
    clear_listbox(list);

    let filters = pagination_filter_terms(pagination);
    let mut sorted = devices.to_vec();
    sort_bluetooth_devices(&mut sorted, sort, watchlists);
    let watchlist_classes = watchlist_css_classes(watchlists);
    let filtered = sorted
        .into_iter()
        .filter(|device| {
            row_matches_column_filters(&filters, |column_id| {
                bluetooth_column_value_with_watchlist(device, column_id, watchlists)
            })
        })
        .collect::<Vec<_>>();
    let total_items = filtered.len();
    let page_size = pagination.page_size.get();
    let (current_page, total_pages, start, end) =
        paged_indices(total_items, pagination.current_page.get(), page_size);

    for device in filtered
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
    {
        let row = ListBoxRow::new();
        row.set_widget_name(&device.mac);
        attach_row_click_selection(&row, list, selected_key_state.clone());
        let line = GtkBox::new(Orientation::Horizontal, 14);
        line.set_hexpand(true);
        let watchlist_match = bluetooth_watchlist_match(&device, watchlists);
        set_row_alert_classes(
            &row,
            &line,
            watchlist_match
                .as_ref()
                .map(|matched| matched.css_class.as_str()),
            &watchlist_classes,
            false,
        );
        for column in settings
            .bluetooth_table_layout
            .columns
            .iter()
            .filter(|c| c.visible)
        {
            if let Some(value) =
                bluetooth_column_value_with_watchlist(&device, &column.id, watchlists)
            {
                line.append(&label_cell(value, column.width_chars));
            }
        }
        row.set_child(Some(&line));
        list.append(&row);
    }

    update_table_pagination_summary(
        pagination,
        total_items,
        current_page,
        total_pages,
        start,
        end,
    );
    restore_listbox_selection(list, selected_key.as_deref());
}

fn clear_listbox(list: &ListBox) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
}

fn clear_box(holder: &GtkBox) {
    while let Some(widget) = holder.first_child() {
        holder.remove(&widget);
    }
}

fn attach_listbox_click_selection(list: &ListBox) {
    let click = GestureClick::new();
    click.set_button(1);
    let click_list = list.clone();
    click.connect_pressed(move |_, _, _x, y| {
        if let Some(row) = click_list.row_at_y(y as i32) {
            click_list.select_row(Some(&row));
        }
    });
    list.add_controller(click);
}

fn selected_row_key(list: &ListBox) -> Option<String> {
    list.selected_row().map(|row| row.widget_name().to_string())
}

fn restore_listbox_selection(list: &ListBox, key: Option<&str>) {
    let Some(key) = key else {
        list.unselect_all();
        return;
    };

    let already_selected = list
        .selected_row()
        .map(|row| row.widget_name().as_str() == key)
        .unwrap_or(false);
    if already_selected {
        return;
    }

    let mut child = list.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if let Ok(row) = widget.downcast::<ListBoxRow>() {
            if row.widget_name().as_str() == key {
                list.select_row(Some(&row));
                break;
            }
        }
    }
}

fn label_cell(text: String, width_chars: i32) -> Label {
    let label = Label::new(Some(&text));
    label.add_css_class("table-cell");
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let width_chars = width_chars.max(6);
    label.set_width_chars(width_chars);
    label.set_max_width_chars(width_chars);
    label.set_single_line_mode(true);
    label.set_size_request(width_chars * TABLE_CHAR_WIDTH_PX, -1);
    label.set_margin_end(6);
    label
}

fn attach_row_click_selection(
    row: &ListBoxRow,
    list: &ListBox,
    selected_key_state: Option<Rc<RefCell<Option<String>>>>,
) {
    row.set_selectable(true);
    row.set_activatable(true);
    let click = GestureClick::new();
    click.set_button(1);
    let row_ref = row.clone();
    let list_ref = list.clone();
    let row_key = row.widget_name().to_string();
    click.connect_pressed(move |_, _, _, _| {
        if let Some(selected_key_state) = selected_key_state.as_ref() {
            *selected_key_state.borrow_mut() = Some(row_key.clone());
        }
        list_ref.select_row(Some(&row_ref));
    });
    row.add_controller(click);
}

fn sdr_hardware_from_active_id(active_id: Option<glib::GString>) -> SdrHardware {
    match active_id.as_deref().map(str::trim) {
        Some("hackrf") => SdrHardware::HackRf,
        Some("bladerf") => SdrHardware::BladeRf,
        Some("ettus_b210") => SdrHardware::EttusB210,
        _ => SdrHardware::RtlSdr,
    }
}

fn load_gqrx_bookmarks() -> Vec<(String, u64)> {
    let Some(config_dir) = dirs::config_dir() else {
        return Vec::new();
    };
    let path = config_dir.join("gqrx/bookmarks.csv");
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };

    let mut bookmarks = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts = line
            .split([';', ',', '\t'])
            .map(str::trim)
            .collect::<Vec<_>>();
        if parts.len() < 2 {
            continue;
        }

        let Some(freq_hz) = parse_frequency_to_hz(parts[1]) else {
            continue;
        };
        if freq_hz < 100_000 {
            continue;
        }

        let label = if parts[0].is_empty() {
            format!("GQRX {:.6} MHz", (freq_hz as f64) / 1_000_000.0)
        } else {
            format!("GQRX {}", parts[0])
        };
        bookmarks.push((label, freq_hz));
    }
    bookmarks
}

fn parse_frequency_to_hz(value: &str) -> Option<u64> {
    let normalized = value.trim().replace('_', "");
    if normalized.is_empty() {
        return None;
    }
    if let Ok(hz) = normalized.parse::<u64>() {
        return Some(hz);
    }
    if let Ok(mhz) = normalized.parse::<f64>() {
        if mhz.is_finite() && mhz > 0.0 {
            return Some((mhz * 1_000_000.0).round() as u64);
        }
    }
    None
}

fn sdr_config_from_inputs(
    hardware_combo: &ComboBoxText,
    center_freq_entry: &Entry,
    sample_rate_entry: &Entry,
    log_enable_check: &CheckButton,
    log_dir_entry: &Entry,
    scan_enable_check: &CheckButton,
    scan_start_entry: &Entry,
    scan_end_entry: &Entry,
    scan_step_entry: &Entry,
    scan_speed_entry: &Entry,
    squelch_scale: &gtk::Scale,
    autotune_check: &CheckButton,
    bias_tee_check: &CheckButton,
    no_payload_satcom_check: &CheckButton,
) -> SdrConfig {
    let defaults = SdrConfig::default();
    let center_freq_hz = center_freq_entry
        .text()
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value >= 100_000)
        .unwrap_or(defaults.center_freq_hz);
    let sample_rate_hz = sample_rate_entry
        .text()
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value >= 200_000)
        .unwrap_or(defaults.sample_rate_hz);

    let log_output_dir = {
        let raw = log_dir_entry.text().trim().to_string();
        if raw.is_empty() {
            defaults.log_output_dir.clone()
        } else {
            PathBuf::from(raw)
        }
    };

    let mut scan_start_hz = scan_start_entry
        .text()
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value >= 100_000)
        .unwrap_or(defaults.scan_start_hz);
    let mut scan_end_hz = scan_end_entry
        .text()
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value >= 100_000)
        .unwrap_or(defaults.scan_end_hz);
    if scan_start_hz > scan_end_hz {
        std::mem::swap(&mut scan_start_hz, &mut scan_end_hz);
    }
    let scan_step_hz = scan_step_entry
        .text()
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(defaults.scan_step_hz);
    let scan_steps_per_sec = scan_speed_entry
        .text()
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(defaults.scan_steps_per_sec);
    let squelch_dbm = squelch_scale.value() as f32;

    SdrConfig {
        hardware: sdr_hardware_from_active_id(hardware_combo.active_id()),
        center_freq_hz,
        sample_rate_hz,
        fft_bins: defaults.fft_bins,
        refresh_ms: defaults.refresh_ms,
        log_output_enabled: log_enable_check.is_active(),
        log_output_dir,
        plugin_config_path: defaults.plugin_config_path,
        scan_range_enabled: scan_enable_check.is_active(),
        scan_start_hz,
        scan_end_hz,
        scan_step_hz,
        scan_steps_per_sec,
        squelch_dbm,
        auto_tune_decoders: autotune_check.is_active(),
        bias_tee_enabled: bias_tee_check.is_active(),
        no_payload_satcom: no_payload_satcom_check.is_active(),
    }
}

fn apply_sdr_runtime_controls(runtime: &SdrRuntime, config: &SdrConfig) {
    runtime.set_logging(config.log_output_enabled, config.log_output_dir.clone());
    runtime.set_scan_range(
        config.scan_range_enabled,
        config.scan_start_hz,
        config.scan_end_hz,
        config.scan_step_hz,
        config.scan_steps_per_sec,
    );
    runtime.set_squelch(config.squelch_dbm);
    runtime.set_auto_tune(config.auto_tune_decoders);
    runtime.set_bias_tee(config.bias_tee_enabled);
    runtime.set_no_payload_satcom(config.no_payload_satcom);
}

fn sdr_decode_table_header() -> Grid {
    let grid = Grid::new();
    grid.set_column_spacing(14);
    let columns = [
        ("Time", 20),
        ("Decoder", 14),
        ("Freq", 13),
        ("Protocol", 14),
        ("Message", 50),
        ("Raw", 50),
    ];
    for (idx, (label, width_chars)) in columns.iter().enumerate() {
        grid.attach(
            &static_header_widget(label, *width_chars),
            idx as i32,
            0,
            1,
            1,
        );
    }
    grid
}

fn sdr_satcom_table_header() -> Grid {
    let grid = Grid::new();
    grid.set_column_spacing(14);
    let columns = [
        ("Time", 20),
        ("Decoder", 14),
        ("Protocol", 14),
        ("Freq", 13),
        ("Band", 14),
        ("Encryption", 12),
        ("Coords", 8),
        ("Identifiers", 20),
        ("Summary", 40),
    ];
    for (idx, (label, width_chars)) in columns.iter().enumerate() {
        grid.attach(
            &static_header_widget(label, *width_chars),
            idx as i32,
            0,
            1,
            1,
        );
    }
    grid
}

fn static_header_widget(label_text: &str, width_chars: i32) -> Label {
    let label = Label::new(Some(label_text));
    label.add_css_class("heading");
    label.add_css_class("table-cell");
    label.set_xalign(0.0);
    let width_chars = width_chars.max(6);
    label.set_width_chars(width_chars);
    label.set_max_width_chars(width_chars);
    label.set_single_line_mode(true);
    label.set_size_request(width_chars * TABLE_CHAR_WIDTH_PX, -1);
    label.set_margin_end(6);
    label
}

fn sdr_decode_row_column_value(row: &SdrDecodeRow, column_id: &str) -> Option<String> {
    match column_id {
        "time" => Some(row.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
        "decoder" => Some(row.decoder.clone()),
        "freq" => Some(format!("{}", row.freq_hz)),
        "protocol" => Some(row.protocol.clone()),
        "message" => Some(row.message.clone()),
        "raw" => Some(row.raw.clone()),
        _ => None,
    }
}

fn sdr_satcom_row_column_value(row: &SdrSatcomObservation, column_id: &str) -> Option<String> {
    match column_id {
        "time" => Some(row.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
        "decoder" => Some(row.decoder.clone()),
        "protocol" => Some(row.protocol.clone()),
        "freq" => Some(row.freq_hz.to_string()),
        "band" => Some(row.band.clone()),
        "posture" => Some(row.encryption_posture.clone()),
        "coords" => Some(if row.has_coordinates {
            "Yes".to_string()
        } else {
            "No".to_string()
        }),
        "identifiers" => Some(row.identifier_hints.join(", ")),
        "summary" => Some(row.summary.clone()),
        _ => None,
    }
}

fn refresh_sdr_decode_list(list: &ListBox, rows: &[SdrDecodeRow], pagination: &TablePaginationUi) {
    clear_listbox(list);
    let filters = pagination_filter_terms(pagination);
    let filtered = rows
        .iter()
        .filter(|row| {
            row_matches_column_filters(&filters, |column_id| {
                sdr_decode_row_column_value(row, column_id)
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    let total_items = filtered.len();
    let page_size = pagination.page_size.get();
    let (current_page, total_pages, start, end) =
        paged_indices(total_items, pagination.current_page.get(), page_size);

    for row in filtered
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
    {
        let line = GtkBox::new(Orientation::Horizontal, 14);
        line.set_hexpand(true);
        line.append(&label_cell(
            row.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            20,
        ));
        line.append(&label_cell(row.decoder, 14));
        line.append(&label_cell(row.freq_hz.to_string(), 13));
        line.append(&label_cell(row.protocol, 14));
        line.append(&label_cell(row.message, 50));
        line.append(&label_cell(row.raw, 50));
        let item = ListBoxRow::new();
        item.set_child(Some(&line));
        list.append(&item);
    }

    update_table_pagination_summary(
        pagination,
        total_items,
        current_page,
        total_pages,
        start,
        end,
    );
}

fn refresh_sdr_satcom_list(
    list: &ListBox,
    rows: &[SdrSatcomObservation],
    pagination: &TablePaginationUi,
) {
    clear_listbox(list);
    let filters = pagination_filter_terms(pagination);
    let filtered = rows
        .iter()
        .filter(|row| {
            row_matches_column_filters(&filters, |column_id| {
                sdr_satcom_row_column_value(row, column_id)
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    let total_items = filtered.len();
    let page_size = pagination.page_size.get();
    let (current_page, total_pages, start, end) =
        paged_indices(total_items, pagination.current_page.get(), page_size);

    for row in filtered
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
    {
        let line = GtkBox::new(Orientation::Horizontal, 14);
        line.set_hexpand(true);
        line.append(&label_cell(
            row.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            20,
        ));
        line.append(&label_cell(row.decoder, 14));
        line.append(&label_cell(row.protocol, 14));
        line.append(&label_cell(row.freq_hz.to_string(), 13));
        line.append(&label_cell(row.band, 14));
        line.append(&label_cell(row.encryption_posture, 12));
        line.append(&label_cell(
            if row.has_coordinates {
                "Yes".to_string()
            } else {
                "No".to_string()
            },
            8,
        ));
        line.append(&label_cell(row.identifier_hints.join(", "), 20));
        line.append(&label_cell(row.summary, 40));
        let item = ListBoxRow::new();
        item.set_child(Some(&line));
        list.append(&item);
    }

    update_table_pagination_summary(
        pagination,
        total_items,
        current_page,
        total_pages,
        start,
        end,
    );
}

fn sdr_decode_signature(
    rows: &[SdrDecodeRow],
    current_page: usize,
    page_size: usize,
    filters: &[(String, String)],
) -> String {
    let latest = rows
        .last()
        .map(|row| {
            format!(
                "{}|{}|{}|{}|{}",
                row.timestamp.timestamp_millis(),
                row.decoder,
                row.freq_hz,
                row.protocol,
                row.message
            )
        })
        .unwrap_or_default();
    format!(
        "rows={}|latest={}|page={}|size={}|filters={}",
        rows.len(),
        latest,
        current_page,
        page_size,
        pagination_filter_signature(filters)
    )
}

fn sdr_satcom_signature(
    rows: &[SdrSatcomObservation],
    current_page: usize,
    page_size: usize,
    filters: &[(String, String)],
) -> String {
    let latest = rows
        .last()
        .map(|row| {
            format!(
                "{}|{}|{}|{}|{}|{}|{}",
                row.timestamp.timestamp_millis(),
                row.decoder,
                row.protocol,
                row.freq_hz,
                row.band,
                row.encryption_posture,
                row.summary
            )
        })
        .unwrap_or_default();
    format!(
        "rows={}|latest={}|page={}|size={}|filters={}",
        rows.len(),
        latest,
        current_page,
        page_size,
        pagination_filter_signature(filters)
    )
}

fn format_sdr_dependency_status(statuses: &[SdrDependencyStatus]) -> String {
    if statuses.is_empty() {
        return "Dependencies: no data".to_string();
    }
    let missing = statuses
        .iter()
        .filter(|status| !status.installed)
        .map(|status| format!("{} ({})", status.tool, status.package_hint))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        "Dependencies: all installed".to_string()
    } else {
        format!("Missing: {}", missing.join(", "))
    }
}

fn draw_sdr_fft(ctx: &cairo::Context, width: f64, height: f64, model: &SdrUiModel) {
    ctx.set_source_rgb(0.03, 0.05, 0.09);
    let _ = ctx.paint();
    if model.spectrum_bins.is_empty() {
        ctx.set_source_rgb(0.7, 0.7, 0.7);
        ctx.move_to(14.0, height / 2.0);
        let _ = ctx.show_text("No SDR spectrum data yet");
        return;
    }

    let min_db = -120.0_f64;
    let max_db = -20.0_f64;
    let bins = &model.spectrum_bins;
    let bin_count = bins.len().max(2);
    ctx.set_source_rgb(0.31, 0.83, 0.95);
    ctx.set_line_width(1.2);

    for (index, value) in bins.iter().enumerate() {
        let x = (index as f64 / (bin_count - 1) as f64) * width;
        let normalized = (((*value as f64) - min_db) / (max_db - min_db)).clamp(0.0, 1.0);
        let y = height - normalized * (height - 8.0) - 4.0;
        if index == 0 {
            ctx.move_to(x, y);
        } else {
            ctx.line_to(x, y);
        }
    }
    let _ = ctx.stroke();

    ctx.set_source_rgb(0.88, 0.42, 0.12);
    ctx.set_line_width(1.0);
    let center_x = width / 2.0;
    ctx.move_to(center_x, 0.0);
    ctx.line_to(center_x, height);
    let _ = ctx.stroke();
}

fn draw_sdr_spectrogram(ctx: &cairo::Context, width: f64, height: f64, model: &SdrUiModel) {
    ctx.set_source_rgb(0.02, 0.03, 0.06);
    let _ = ctx.paint();
    if model.spectrogram_rows.is_empty() {
        ctx.set_source_rgb(0.7, 0.7, 0.7);
        ctx.move_to(14.0, height / 2.0);
        let _ = ctx.show_text("No SDR spectrogram data yet");
        return;
    }

    let rows = &model.spectrogram_rows;
    let row_count = rows.len().max(1);
    let bins = rows.first().map(|entry| entry.len()).unwrap_or(0).max(1);
    let cell_w = (width / bins as f64).max(1.0);
    let cell_h = (height / row_count as f64).max(1.0);
    for (row_idx, row) in rows.iter().enumerate() {
        let y = height - ((row_idx + 1) as f64 * cell_h);
        for (bin_idx, power) in row.iter().enumerate() {
            let x = bin_idx as f64 * cell_w;
            let normalized = (((*power as f64) + 120.0) / 100.0).clamp(0.0, 1.0);
            let red = normalized;
            let green = (normalized * 0.75).min(1.0);
            let blue = (1.0 - normalized).clamp(0.0, 1.0);
            ctx.set_source_rgb(red, green, blue);
            ctx.rectangle(x, y, cell_w + 0.4, cell_h + 0.4);
            let _ = ctx.fill();
        }
    }
}

fn draw_sdr_map(ctx: &cairo::Context, width: f64, height: f64, model: &SdrUiModel) {
    ctx.set_source_rgb(0.02, 0.03, 0.05);
    let _ = ctx.paint();
    if model.map_points.is_empty() {
        ctx.set_source_rgb(0.7, 0.7, 0.7);
        ctx.move_to(14.0, height / 2.0);
        let _ = ctx.show_text("No decoded coordinate points yet");
        return;
    }

    let margin_left = 44.0;
    let margin_top = 14.0;
    let margin_right = 18.0;
    let margin_bottom = 20.0;
    let plot_width = (width - margin_left - margin_right).max(20.0);
    let plot_height = (height - margin_top - margin_bottom).max(20.0);

    let mut min_lat = f64::INFINITY;
    let mut max_lat = f64::NEG_INFINITY;
    let mut min_lon = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;
    for point in &model.map_points {
        min_lat = min_lat.min(point.latitude);
        max_lat = max_lat.max(point.latitude);
        min_lon = min_lon.min(point.longitude);
        max_lon = max_lon.max(point.longitude);
    }

    if !min_lat.is_finite() || !max_lat.is_finite() || !min_lon.is_finite() || !max_lon.is_finite()
    {
        ctx.set_source_rgb(0.7, 0.7, 0.7);
        ctx.move_to(14.0, height / 2.0);
        let _ = ctx.show_text("Coordinate data invalid");
        return;
    }

    let mut lat_span = (max_lat - min_lat).abs();
    let mut lon_span = (max_lon - min_lon).abs();
    if lat_span < 1e-6 {
        min_lat -= 0.01;
        max_lat += 0.01;
        lat_span = max_lat - min_lat;
    }
    if lon_span < 1e-6 {
        min_lon -= 0.01;
        max_lon += 0.01;
        lon_span = max_lon - min_lon;
    }

    ctx.set_source_rgb(0.14, 0.18, 0.22);
    ctx.rectangle(margin_left, margin_top, plot_width, plot_height);
    let _ = ctx.fill();
    ctx.set_source_rgb(0.34, 0.38, 0.42);
    ctx.set_line_width(1.0);
    ctx.rectangle(margin_left, margin_top, plot_width, plot_height);
    let _ = ctx.stroke();

    for point in model.map_points.iter().rev().take(5000).rev() {
        let x_ratio = ((point.longitude - min_lon) / lon_span).clamp(0.0, 1.0);
        let y_ratio = ((point.latitude - min_lat) / lat_span).clamp(0.0, 1.0);
        let x = margin_left + x_ratio * plot_width;
        let y = margin_top + (1.0 - y_ratio) * plot_height;
        let (r, g, b) = match point.protocol.to_ascii_lowercase().as_str() {
            "adsb" => (0.95, 0.74, 0.21),
            "ais" => (0.22, 0.80, 0.95),
            "acars" => (0.63, 0.88, 0.38),
            "iridium" => (0.86, 0.40, 0.98),
            _ => (0.34, 0.84, 0.42),
        };
        ctx.set_source_rgba(r, g, b, 0.8);
        ctx.arc(x, y, 2.2, 0.0, std::f64::consts::TAU);
        let _ = ctx.fill();
    }

    ctx.set_source_rgb(0.84, 0.86, 0.88);
    ctx.move_to(margin_left, height - 4.0);
    let _ = ctx.show_text(&format!("Lon {:.4} .. {:.4}", min_lon, max_lon));
    ctx.move_to(4.0, margin_top + 10.0);
    let _ = ctx.show_text(&format!("{:.4}", max_lat));
    ctx.move_to(4.0, margin_top + plot_height);
    let _ = ctx.show_text(&format!("{:.4}", min_lat));
}

fn ap_table_header(
    layout: &TableLayout,
    sort: &TableSortState,
    state: Rc<RefCell<AppState>>,
) -> Grid {
    make_table_header(
        layout,
        ap_column_label,
        sort,
        Rc::new(move |column_id| {
            state
                .borrow_mut()
                .toggle_table_sort(SortableTable::AccessPoints, column_id);
        }),
    )
}

fn client_table_header(
    layout: &TableLayout,
    sort: &TableSortState,
    state: Rc<RefCell<AppState>>,
) -> Grid {
    make_table_header(
        layout,
        client_column_label,
        sort,
        Rc::new(move |column_id| {
            state
                .borrow_mut()
                .toggle_table_sort(SortableTable::Clients, column_id);
        }),
    )
}

fn ap_assoc_clients_header(
    layout: &TableLayout,
    sort: &TableSortState,
    state: Rc<RefCell<AppState>>,
) -> Grid {
    make_table_header(
        layout,
        assoc_client_column_label,
        sort,
        Rc::new(move |column_id| {
            state
                .borrow_mut()
                .toggle_table_sort(SortableTable::AssocClients, column_id);
        }),
    )
}

fn bluetooth_table_header(
    layout: &TableLayout,
    sort: &TableSortState,
    state: Rc<RefCell<AppState>>,
) -> Grid {
    make_table_header(
        layout,
        bluetooth_column_label,
        sort,
        Rc::new(move |column_id| {
            state
                .borrow_mut()
                .toggle_table_sort(SortableTable::Bluetooth, column_id);
        }),
    )
}

fn make_table_header(
    layout: &TableLayout,
    label_for: fn(&str) -> &'static str,
    sort: &TableSortState,
    on_sort: Rc<dyn Fn(String)>,
) -> Grid {
    let grid = Grid::new();
    grid.set_column_spacing(14);
    for (i, column) in layout.columns.iter().filter(|c| c.visible).enumerate() {
        grid.attach(
            &sortable_header_widget(
                &column.id,
                label_for(&column.id),
                column.width_chars.max(6),
                sort,
                on_sort.clone(),
            ),
            i as i32,
            0,
            1,
            1,
        );
    }
    grid
}

fn sortable_header_widget(
    column_id: &str,
    label_text: &str,
    width_chars: i32,
    sort: &TableSortState,
    on_sort: Rc<dyn Fn(String)>,
) -> Label {
    let label = Label::new(Some(&sortable_header_text(column_id, label_text, sort)));
    label.add_css_class("heading");
    label.add_css_class("sort-header");
    label.add_css_class("table-cell");
    label.set_xalign(0.0);
    let width_chars = width_chars.max(6);
    label.set_width_chars(width_chars);
    label.set_max_width_chars(width_chars);
    label.set_single_line_mode(true);
    label.set_size_request(width_chars * TABLE_CHAR_WIDTH_PX, -1);
    label.set_margin_end(6);

    let click = GestureClick::new();
    let column_id = column_id.to_string();
    click.connect_released(move |_, _, _, _| {
        on_sort(column_id.clone());
    });
    label.add_controller(click);

    label
}

fn sortable_header_text(column_id: &str, label_text: &str, sort: &TableSortState) -> String {
    if sort.column_id == column_id {
        format!("{} {}", label_text, if sort.descending { "▼" } else { "▲" })
    } else {
        label_text.to_string()
    }
}

fn ap_column_label(id: &str) -> &'static str {
    match id {
        "watchlist_entry" => "Watchlist Entry",
        "ssid" => "SSID",
        "bssid" => "BSSID",
        "oui" => "OUI Manufacturer",
        "channel" => "Channel",
        "encryption" => "Encryption",
        "rssi" => "RSSI",
        "wps" => "WPS",
        "clients" => "Clients",
        "first_seen" => "First Seen",
        "last_seen" => "Last Seen",
        "handshakes" => "Handshakes",
        "band" => "Band",
        "frequency" => "Frequency",
        "country" => "Country",
        "full_encryption" => "Full Encryption",
        "hidden_ssid" => "Hidden SSID",
        "uptime" => "Uptime",
        "observation_count" => "Observations",
        "avg_rssi" => "Avg RSSI",
        "min_rssi" => "Min RSSI",
        "max_rssi" => "Max RSSI",
        "packet_total" => "Packets",
        "notes" => "Notes",
        "first_location" => "First Location",
        "last_location" => "Last Location",
        "strongest_location" => "Strongest Location",
        _ => "Unknown",
    }
}

fn client_column_label(id: &str) -> &'static str {
    match id {
        "watchlist_entry" => "Watchlist Entry",
        "mac" => "MAC",
        "oui" => "OUI",
        "associated_ap" => "Associated AP",
        "associated_ssid" => "Associated SSID",
        "rssi" => "RSSI",
        "wps" => "WPS",
        "probes" => "Probes",
        "first_heard" => "First Heard",
        "last_heard" => "Last Heard",
        "data_transferred" => "Data Transferred",
        "probe_count" => "Probe Count",
        "seen_ap_count" => "Seen APs",
        "handshake_network_count" => "Handshake Nets",
        "observation_count" => "Observations",
        "avg_rssi" => "Avg RSSI",
        "min_rssi" => "Min RSSI",
        "max_rssi" => "Max RSSI",
        "seen_aps" => "Seen APs",
        "handshake_networks" => "Handshake Networks",
        "first_location" => "First Location",
        "last_location" => "Last Location",
        "strongest_location" => "Strongest Location",
        _ => "Unknown",
    }
}

fn assoc_client_column_label(id: &str) -> &'static str {
    match id {
        "watchlist_entry" => "Watchlist Entry",
        "mac" => "MAC",
        "status" => "History",
        "current_ap" => "Current AP",
        "current_ssid" => "Current SSID",
        "oui" => "OUI",
        "data_transferred" => "Data Transferred",
        "rssi" => "RSSI",
        "first_heard" => "First Seen",
        "last_heard" => "Last Seen",
        "wps" => "WPS",
        "probe_count" => "Probe Count",
        "seen_ap_count" => "Seen APs",
        "handshake_network_count" => "Handshake Nets",
        _ => "Unknown",
    }
}

fn bluetooth_column_label(id: &str) -> &'static str {
    match id {
        "watchlist_entry" => "Watchlist Entry",
        "transport" => "BT/BLE",
        "mac" => "MAC",
        "oui" => "OUI",
        "name" => "Name",
        "type" => "Type",
        "first_seen" => "First Seen",
        "last_seen" => "Last Seen",
        "rssi" => "RSSI",
        "advertised_name" => "Advertised Name",
        "alias" => "Alias",
        "address_type" => "Address Type",
        "class_of_device" => "Class",
        "mfgr_ids" => "MFGRID",
        "mfgr_names" => "MFGR Names",
        "uuids" => "UUIDs",
        _ => "Unknown",
    }
}

fn ap_column_value(ap: &AccessPointRecord, id: &str) -> Option<String> {
    let (avg_rssi, min_rssi, max_rssi, sample_count) = rssi_stats(&ap.observations, ap.rssi_dbm);
    let highlights = observation_highlights(&ap.observations);
    let value = match id {
        "ssid" => ap.ssid.clone().unwrap_or_else(|| "<hidden>".to_string()),
        "bssid" => ap.bssid.clone(),
        "oui" => ap.oui_manufacturer.clone().unwrap_or_default(),
        "channel" => ap.channel.map(|c| c.to_string()).unwrap_or_default(),
        "encryption" => ap.encryption_short.clone(),
        "rssi" => format_dbm(ap.rssi_dbm),
        "wps" => {
            if ap.wps.is_some() {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        "clients" => ap.number_of_clients.to_string(),
        "first_seen" => ap.first_seen.format("%H:%M:%S").to_string(),
        "last_seen" => ap.last_seen.format("%H:%M:%S").to_string(),
        "handshakes" => ap.handshake_count.to_string(),
        "band" => ap.band.label().to_string(),
        "frequency" => ap
            .frequency_mhz
            .map(|v| format!("{v} MHz"))
            .unwrap_or_default(),
        "country" => ap.country_code_80211d.clone().unwrap_or_default(),
        "full_encryption" => ap.encryption_full.clone(),
        "hidden_ssid" => bool_text(ap_hidden_ssid(ap)),
        "uptime" => format_beacon_uptime(ap.uptime_beacons),
        "observation_count" => ap.observations.len().to_string(),
        "avg_rssi" => format_dbm(avg_rssi),
        "min_rssi" => format_dbm(min_rssi),
        "max_rssi" => format_dbm(max_rssi),
        "packet_total" => ap.packet_mix.total().to_string(),
        "notes" => ap.notes.clone().unwrap_or_default(),
        "first_location" => highlights
            .first
            .as_ref()
            .map(format_observation_location_time)
            .unwrap_or_default(),
        "last_location" => highlights
            .last
            .as_ref()
            .map(format_observation_location_time)
            .unwrap_or_default(),
        "strongest_location" => {
            format_strongest_observation(highlights.strongest.as_ref(), !ap.observations.is_empty())
        }
        _ => return None,
    };
    let value = if id == "observation_count" && sample_count == 0 {
        ap.observations.len().to_string()
    } else {
        value
    };
    Some(value)
}

fn ap_watchlist_entry_value(ap: &AccessPointRecord, watchlists: &WatchlistSettings) -> String {
    ap_watchlist_match(ap, watchlists)
        .map(|matched| matched.label)
        .unwrap_or_default()
}

fn ap_column_value_with_watchlist(
    ap: &AccessPointRecord,
    id: &str,
    watchlists: &WatchlistSettings,
) -> Option<String> {
    if id == "watchlist_entry" {
        Some(ap_watchlist_entry_value(ap, watchlists))
    } else {
        ap_column_value(ap, id)
    }
}

fn associated_ssid_for_client(aps: &[AccessPointRecord], client: &ClientRecord) -> Option<String> {
    let ap_bssid = client.associated_ap.as_deref()?;
    aps.iter()
        .find(|ap| ap.bssid.eq_ignore_ascii_case(ap_bssid))
        .map(|ap| ap.ssid.clone().unwrap_or_else(|| "<hidden>".to_string()))
}

fn client_column_value(
    client: &ClientRecord,
    aps: &[AccessPointRecord],
    id: &str,
) -> Option<String> {
    let (avg_rssi, min_rssi, max_rssi, _) = rssi_stats(&client.observations, client.rssi_dbm);
    let highlights = observation_highlights(&client.observations);
    let value = match id {
        "mac" => client.mac.clone(),
        "oui" => client.oui_manufacturer.clone().unwrap_or_default(),
        "associated_ap" => client.associated_ap.clone().unwrap_or_default(),
        "associated_ssid" => associated_ssid_for_client(aps, client).unwrap_or_default(),
        "rssi" => format_dbm(client.rssi_dbm),
        "wps" => {
            if client.wps.is_some() {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        "probes" => client.probes.join(","),
        "first_heard" => client.first_seen.format("%H:%M:%S").to_string(),
        "last_heard" => client.last_seen.format("%H:%M:%S").to_string(),
        "data_transferred" => client.data_transferred_bytes.to_string(),
        "probe_count" => client.probes.len().to_string(),
        "seen_ap_count" => client.seen_access_points.len().to_string(),
        "handshake_network_count" => client.handshake_networks.len().to_string(),
        "observation_count" => client.observations.len().to_string(),
        "avg_rssi" => format_dbm(avg_rssi),
        "min_rssi" => format_dbm(min_rssi),
        "max_rssi" => format_dbm(max_rssi),
        "seen_aps" => client.seen_access_points.join(", "),
        "handshake_networks" => client.handshake_networks.join(", "),
        "band" => client.network_intel.band.label().to_string(),
        "channel" => client
            .network_intel
            .last_channel
            .map(|value| value.to_string())
            .unwrap_or_default(),
        "frequency" => client
            .network_intel
            .last_frequency_mhz
            .map(|value| format!("{value} MHz"))
            .unwrap_or_default(),
        "uplink_bytes" => client.network_intel.uplink_bytes.to_string(),
        "downlink_bytes" => client.network_intel.downlink_bytes.to_string(),
        "retry_count" => client.network_intel.retry_frame_count.to_string(),
        "retry_rate" => retry_rate_text(client),
        "power_save" => bool_text(client.network_intel.power_save_observed),
        "eapol_frames" => client.network_intel.eapol_frame_count.to_string(),
        "pmkid_count" => client.network_intel.pmkid_count.to_string(),
        "first_location" => highlights
            .first
            .as_ref()
            .map(format_observation_location_time)
            .unwrap_or_default(),
        "last_location" => highlights
            .last
            .as_ref()
            .map(format_observation_location_time)
            .unwrap_or_default(),
        "strongest_location" => format_strongest_observation(
            highlights.strongest.as_ref(),
            !client.observations.is_empty(),
        ),
        _ => return None,
    };
    Some(value)
}

fn client_watchlist_entry_value(
    client: &ClientRecord,
    aps: &[AccessPointRecord],
    watchlists: &WatchlistSettings,
) -> String {
    client_watchlist_match(client, aps, watchlists)
        .map(|matched| matched.label)
        .unwrap_or_default()
}

fn client_column_value_with_watchlist(
    client: &ClientRecord,
    aps: &[AccessPointRecord],
    id: &str,
    watchlists: &WatchlistSettings,
) -> Option<String> {
    if id == "watchlist_entry" {
        Some(client_watchlist_entry_value(client, aps, watchlists))
    } else {
        client_column_value(client, aps, id)
    }
}

fn assoc_client_column_value(
    client: &ClientRecord,
    ap_bssid: &str,
    aps: &[AccessPointRecord],
    id: &str,
) -> Option<String> {
    let value = match id {
        "mac" => client.mac.clone(),
        "status" => {
            if client.associated_ap.as_deref() == Some(ap_bssid) {
                "Current".to_string()
            } else {
                "Historical".to_string()
            }
        }
        "current_ap" => client.associated_ap.clone().unwrap_or_default(),
        "current_ssid" => associated_ssid_for_client(aps, client).unwrap_or_default(),
        "oui" => client.oui_manufacturer.clone().unwrap_or_default(),
        "data_transferred" => client.data_transferred_bytes.to_string(),
        "rssi" => format_dbm(client.rssi_dbm),
        "first_heard" => client.first_seen.format("%H:%M:%S").to_string(),
        "last_heard" => client.last_seen.format("%H:%M:%S").to_string(),
        "wps" => {
            if client.wps.is_some() {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        "probe_count" => client.probes.len().to_string(),
        "seen_ap_count" => client.seen_access_points.len().to_string(),
        "handshake_network_count" => client.handshake_networks.len().to_string(),
        _ => return None,
    };
    Some(value)
}

fn assoc_client_column_value_with_watchlist(
    client: &ClientRecord,
    ap_bssid: &str,
    aps: &[AccessPointRecord],
    id: &str,
    watchlists: &WatchlistSettings,
) -> Option<String> {
    if id == "watchlist_entry" {
        Some(client_watchlist_entry_value(client, aps, watchlists))
    } else {
        assoc_client_column_value(client, ap_bssid, aps, id)
    }
}

fn bluetooth_column_value(device: &BluetoothDeviceRecord, id: &str) -> Option<String> {
    let value = match id {
        "transport" => device.transport.clone(),
        "mac" => device.mac.clone(),
        "oui" => device.oui_manufacturer.clone().unwrap_or_default(),
        "name" => bluetooth_display_name(device),
        "type" => device
            .device_type
            .clone()
            .unwrap_or_else(|| "Unknown".to_string()),
        "first_seen" => device.first_seen.format("%H:%M:%S").to_string(),
        "last_seen" => device.last_seen.format("%H:%M:%S").to_string(),
        "rssi" => format_dbm(device.rssi_dbm),
        "advertised_name" => device.advertised_name.clone().unwrap_or_default(),
        "alias" => device.alias.clone().unwrap_or_default(),
        "address_type" => device.address_type.clone().unwrap_or_default(),
        "class_of_device" => device.class_of_device.clone().unwrap_or_default(),
        "mfgr_ids" => device.mfgr_ids.join(", "),
        "mfgr_names" => device.mfgr_names.join(", "),
        "uuids" => device.uuid_names.join(", "),
        _ => return None,
    };
    Some(value)
}

fn bluetooth_watchlist_entry_value(
    device: &BluetoothDeviceRecord,
    watchlists: &WatchlistSettings,
) -> String {
    bluetooth_watchlist_match(device, watchlists)
        .map(|matched| matched.label)
        .unwrap_or_default()
}

fn bluetooth_column_value_with_watchlist(
    device: &BluetoothDeviceRecord,
    id: &str,
    watchlists: &WatchlistSettings,
) -> Option<String> {
    if id == "watchlist_entry" {
        Some(bluetooth_watchlist_entry_value(device, watchlists))
    } else {
        bluetooth_column_value(device, id)
    }
}

fn bluetooth_display_name(device: &BluetoothDeviceRecord) -> String {
    device
        .advertised_name
        .clone()
        .or_else(|| device.alias.clone())
        .unwrap_or_default()
}

fn ap_hidden_ssid(ap: &AccessPointRecord) -> bool {
    ap.ssid
        .as_deref()
        .map(|ssid| ssid.is_empty())
        .unwrap_or(true)
}

fn bool_text(value: bool) -> String {
    if value {
        "True".to_string()
    } else {
        "False".to_string()
    }
}

fn format_dbm(value: Option<i32>) -> String {
    value.map(|v| format!("{v} dBm")).unwrap_or_default()
}

fn display_dbm(value: Option<i32>) -> String {
    let text = format_dbm(value);
    if text.is_empty() {
        "Unknown".to_string()
    } else {
        text
    }
}

fn set_row_alert_classes(
    row: &ListBoxRow,
    _line: &GtkBox,
    watchlist_class: Option<&str>,
    all_watchlist_classes: &[String],
    handshake: bool,
) {
    for class_name in all_watchlist_classes {
        row.remove_css_class(class_name);
    }
    row.remove_css_class("row-handshake");
    if let Some(class_name) = watchlist_class {
        row.add_css_class(class_name);
    } else if handshake {
        row.add_css_class("row-handshake");
    }
}

fn set_detail_watchlist_highlight<W: IsA<gtk::Widget>>(label: &W, _watchlist: bool) {
    label.remove_css_class("detail-watchlist");
}

fn format_ap_detail_text(ap: &AccessPointRecord) -> String {
    let highlights = observation_highlights(&ap.observations);
    let (avg_rssi, min_rssi, max_rssi, rssi_samples) = rssi_stats(&ap.observations, ap.rssi_dbm);
    let wps = ap
        .wps
        .as_ref()
        .map(|w| {
            format!(
                "Present\n  Version: {}\n  State: {}\n  Config Methods: {}\n  Manufacturer: {}\n  Model Name: {}\n  Model Number: {}\n  Serial Number: {}",
                w.version.as_deref().unwrap_or("Unknown"),
                w.state.as_deref().unwrap_or("Unknown"),
                w.config_methods.as_deref().unwrap_or("Unknown"),
                w.manufacturer.as_deref().unwrap_or("Unknown"),
                w.model_name.as_deref().unwrap_or("Unknown"),
                w.model_number.as_deref().unwrap_or("Unknown"),
                w.serial_number.as_deref().unwrap_or("Unknown")
            )
        })
        .unwrap_or_else(|| "Not observed".to_string());
    let first_location = highlights
        .first
        .as_ref()
        .map(format_observation_location_time)
        .unwrap_or_else(|| "Unknown".to_string());
    let last_location = highlights
        .last
        .as_ref()
        .map(format_observation_location_time)
        .unwrap_or_else(|| "Unknown".to_string());
    let strongest_location =
        format_strongest_observation(highlights.strongest.as_ref(), !ap.observations.is_empty());
    let hidden_ssid = bool_text(ap_hidden_ssid(ap));
    let (security_akm, security_cipher, security_pmf) = ap_security_breakdown(ap);
    let advanced_not_captured = "Not captured yet";

    format!(
        "Identity\nSSID: {}\nHidden SSID: {}\nBSSID: {}\nOUI: {}\n802.11d Country: {}\n\nSecurity\nEncryption: {}\nFull Encryption: {}\nAKM Suites: {}\nCipher Suites: {}\nPMF: {}\nWPS:\n{}\nHandshake Count (WPA2 4-way full): {}\nPMKID Count: {}\n\nRadio\nBand: {}\nPrimary Channel: {}\nFrequency: {} MHz\nSecondary Channel: {}\nChannel Width: {}\nCenter Segment 0: {}\nCenter Segment 1: {}\nPHY Generation: {}\nHT/VHT/HE/EHT Summary: {}\nSupported Rates: {}\nBasic Rates: {}\nWMM / QoS: {}\n802.11k: {}\n802.11v: {}\n802.11r: {}\nDFS / TPC: {}\nChannel Switch Announcement: {}\nMulti-BSSID: {}\nRNR / Neighbor Report: {}\n802.11u / Hotspot 2.0: {}\nVendor IEs: {}\n\nPresence\nCurrent RSSI: {}\nAverage RSSI: {}\nMinimum RSSI: {}\nMaximum RSSI: {}\nRSSI Samples: {}\nClients: {}\nFirst Seen: {}\nLast Seen: {}\nObservation Count: {}\nFirst Location: {}\nLast Location: {}\nStrongest Location: {}\nUptime (beacon estimate): {}\nBeacon Interval: {}\nDTIM Period: {}\n\nAnalytics\nPacket Totals: total={} mgmt={} control={} data={} other={}\nBSS Load: {}\nObserved Data Rates: {}\nRetry Rate: {}\n\nNotes\n{}",
        ap.ssid.clone().unwrap_or_else(|| "<hidden>".to_string()),
        hidden_ssid,
        ap.bssid,
        ap.oui_manufacturer.clone().unwrap_or_else(|| "Unknown".into()),
        ap.country_code_80211d
            .clone()
            .unwrap_or_else(|| "Unknown".into()),
        ap.encryption_short,
        ap.encryption_full,
        security_akm,
        security_cipher,
        security_pmf,
        wps,
        ap.handshake_count,
        advanced_not_captured,
        ap.band.label(),
        ap.channel
            .map(|v| v.to_string())
            .unwrap_or_else(|| "Unknown".into()),
        ap.frequency_mhz
            .map(|v| v.to_string())
            .unwrap_or_else(|| "Unknown".into()),
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        display_dbm(ap.rssi_dbm),
        display_dbm(avg_rssi),
        display_dbm(min_rssi),
        display_dbm(max_rssi),
        rssi_samples,
        ap.number_of_clients,
        ap.first_seen,
        ap.last_seen,
        ap.observations.len(),
        first_location,
        last_location,
        strongest_location,
        format_beacon_uptime(ap.uptime_beacons),
        advanced_not_captured,
        advanced_not_captured,
        ap.packet_mix.total(),
        ap.packet_mix.management,
        ap.packet_mix.control,
        ap.packet_mix.data,
        ap.packet_mix.other,
        advanced_not_captured,
        advanced_not_captured,
        advanced_not_captured,
        ap.notes.clone().unwrap_or_else(|| "None".to_string())
    )
}

fn ap_security_breakdown(ap: &AccessPointRecord) -> (String, String, String) {
    let mut akm = "Unknown".to_string();
    let mut cipher = "Unknown".to_string();
    let mut pmf = "Unknown".to_string();

    for part in ap.encryption_full.split(" - ").map(str::trim) {
        if let Some(rest) = part.strip_prefix("AKM ") {
            akm = rest.to_string();
        } else if let Some(rest) = part.strip_prefix("Cipher ") {
            cipher = rest.to_string();
        } else if part.starts_with("PMF ") {
            pmf = part.to_string();
        }
    }

    (akm, cipher, pmf)
}

fn format_beacon_uptime(uptime_seconds: Option<u64>) -> String {
    let Some(total) = uptime_seconds else {
        return "Unknown".to_string();
    };

    let days = total / 86_400;
    let hours = (total % 86_400) / 3_600;
    let minutes = (total % 3_600) / 60;
    let seconds = total % 60;
    format!("{}d {:02}:{:02}:{:02}", days, hours, minutes, seconds)
}

fn is_randomized_mac(mac: &str) -> bool {
    let Some(first_octet) = mac.split(':').next() else {
        return false;
    };
    u8::from_str_radix(first_octet, 16)
        .map(|value| value & 0x02 != 0)
        .unwrap_or(false)
}

fn format_frame_subtype(fc_type: Option<u8>, subtype: Option<u8>) -> String {
    match (fc_type, subtype) {
        (Some(0), Some(4)) => "Probe Request".to_string(),
        (Some(0), Some(5)) => "Probe Response".to_string(),
        (Some(0), Some(8)) => "Beacon".to_string(),
        (Some(0), Some(10)) => "Disassociation".to_string(),
        (Some(0), Some(11)) => "Authentication".to_string(),
        (Some(0), Some(12)) => "Deauthentication".to_string(),
        (Some(0), Some(0)) => "Association Request".to_string(),
        (Some(0), Some(1)) => "Association Response".to_string(),
        (Some(2), Some(0)) => "Data".to_string(),
        (Some(2), Some(8)) => "QoS Data".to_string(),
        (Some(1), Some(13)) => "ACK".to_string(),
        (Some(kind), Some(subtype)) => format!("Type {kind} / Subtype {subtype}"),
        _ => "Unknown".to_string(),
    }
}

fn format_qos_priorities(priorities: &[u8]) -> String {
    if priorities.is_empty() {
        "None observed".to_string()
    } else {
        priorities
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn retry_rate_text(client: &ClientRecord) -> String {
    let total = client.network_intel.packet_mix.total();
    if total == 0 {
        return "Unknown".to_string();
    }
    let rate = client.network_intel.retry_frame_count as f64 / total as f64 * 100.0;
    format!("{:.1}%", rate)
}

fn format_client_detail_text(client: &ClientRecord, aps: &[AccessPointRecord]) -> String {
    let highlights = observation_highlights(&client.observations);
    let (avg_rssi, min_rssi, max_rssi, rssi_samples) =
        rssi_stats(&client.observations, client.rssi_dbm);
    let wps = client
        .wps
        .as_ref()
        .map(|w| {
            format!(
                "version={:?}, state={:?}, manufacturer={:?}, model={:?}, model_number={:?}, serial={:?}",
                w.version,
                w.state,
                w.manufacturer,
                w.model_name,
                w.model_number,
                w.serial_number
            )
        })
        .unwrap_or_else(|| "none observed".to_string());
    let first_location = highlights
        .first
        .as_ref()
        .map(format_observation_location_time)
        .unwrap_or_else(|| "Unknown".to_string());
    let last_location = highlights
        .last
        .as_ref()
        .map(format_observation_location_time)
        .unwrap_or_else(|| "Unknown".to_string());
    let strongest_location = format_strongest_observation(
        highlights.strongest.as_ref(),
        !client.observations.is_empty(),
    );
    let roam_count = client.seen_access_points.len().saturating_sub(1);
    let associated_ssid =
        associated_ssid_for_client(aps, client).unwrap_or_else(|| "Unknown".to_string());

    format!(
        "Identity\nMAC: {}\nOUI: {}\nRandomized MAC: {}\n\nAssociation\nAssociated AP: {}\nAssociated SSID: {}\nSeen AP Count: {}\nSeen APs: {}\nRoam Count: {}\nProbe Count: {}\nProbes: {}\nFirst Heard: {}\nLast Heard: {}\n\nRadio And Behavior\nBand: {}\nLast Channel: {}\nLast Frequency: {}\nCurrent RSSI: {}\nAverage RSSI: {}\nMinimum RSSI: {}\nMaximum RSSI: {}\nRSSI Samples: {}\nPacket Mix: mgmt={} control={} data={} other={}\nData Transferred: {} bytes\nUplink Bytes: {}\nDownlink Bytes: {}\nRetry Frames: {}\nRetry Rate: {}\nPower Save Observed: {}\nQoS Priorities: {}\nLast Frame: {}\nListen Interval: {}\n\nSecurity\nWPS: {}\nEAPOL Frames: {}\nPMKID Count: {}\nHandshake Network Count: {}\nHandshake Networks: {}\nLast Status Code: {}\nLast Reason Code: {}\n\nPresence\nObservation Count: {}\nFirst Location: {}\nLast Location: {}\nStrongest Location: {}",
        client.mac,
        client.oui_manufacturer.clone().unwrap_or_else(|| "Unknown".into()),
        bool_text(is_randomized_mac(&client.mac)),
        client.associated_ap.clone().unwrap_or_else(|| "Unknown".to_string()),
        associated_ssid,
        client.seen_access_points.len(),
        client.seen_access_points.join(", "),
        roam_count,
        client.probes.len(),
        client.probes.join(", "),
        client.first_seen,
        client.last_seen,
        client.network_intel.band.label(),
        client
            .network_intel
            .last_channel
            .map(|value| value.to_string())
            .unwrap_or_else(|| "Unknown".to_string()),
        client
            .network_intel
            .last_frequency_mhz
            .map(|value| format!("{value} MHz"))
            .unwrap_or_else(|| "Unknown".to_string()),
        display_dbm(client.rssi_dbm),
        display_dbm(avg_rssi),
        display_dbm(min_rssi),
        display_dbm(max_rssi),
        rssi_samples,
        client.network_intel.packet_mix.management,
        client.network_intel.packet_mix.control,
        client.network_intel.packet_mix.data,
        client.network_intel.packet_mix.other,
        client.data_transferred_bytes,
        client.network_intel.uplink_bytes,
        client.network_intel.downlink_bytes,
        client.network_intel.retry_frame_count,
        retry_rate_text(client),
        bool_text(client.network_intel.power_save_observed),
        format_qos_priorities(&client.network_intel.qos_priorities),
        format_frame_subtype(
            client.network_intel.last_frame_type,
            client.network_intel.last_frame_subtype,
        ),
        client
            .network_intel
            .listen_interval
            .map(|value| value.to_string())
            .unwrap_or_else(|| "Unknown".to_string()),
        wps,
        client.network_intel.eapol_frame_count,
        client.network_intel.pmkid_count,
        client.handshake_networks.len(),
        client.handshake_networks.join(", "),
        client
            .network_intel
            .last_status_code
            .map(|value| value.to_string())
            .unwrap_or_else(|| "Unknown".to_string()),
        client
            .network_intel
            .last_reason_code
            .map(|value| value.to_string())
            .unwrap_or_else(|| "Unknown".to_string()),
        client.observations.len(),
        first_location,
        last_location,
        strongest_location,
    )
}

fn format_bluetooth_identity_section(device: &BluetoothDeviceRecord) -> String {
    let highlights = observation_highlights(&device.observations);
    let first_location = highlights
        .first
        .as_ref()
        .map(format_observation_location_time)
        .unwrap_or_else(|| "Unknown".to_string());
    let last_location = highlights
        .last
        .as_ref()
        .map(format_observation_location_time)
        .unwrap_or_else(|| "Unknown".to_string());
    let strongest_location = format_strongest_observation(
        highlights.strongest.as_ref(),
        !device.observations.is_empty(),
    );

    format!(
        "MAC: {}\nTransport: {}\nAddress Type: {}\nOUI: {}\nName: {}\nAlias: {}\nDevice Type: {}\nClass: {}\nCurrent RSSI: {}\nFirst Seen: {}\nLast Seen: {}\nFirst Location: {}\nLast Location: {}\nStrongest Location: {}",
        device.mac,
        device.transport,
        device
            .address_type
            .clone()
            .unwrap_or_else(|| "Unknown".to_string()),
        device
            .oui_manufacturer
            .clone()
            .unwrap_or_else(|| "Unknown".to_string()),
        device
            .advertised_name
            .clone()
            .unwrap_or_else(|| "Unknown".to_string()),
        device.alias.clone().unwrap_or_else(|| "Unknown".to_string()),
        device
            .device_type
            .clone()
            .unwrap_or_else(|| "Unknown".to_string()),
        device
            .class_of_device
            .clone()
            .unwrap_or_else(|| "Unknown".to_string()),
        device
            .rssi_dbm
            .map(|v| format!("{} dBm", v))
            .unwrap_or_else(|| "Unknown".to_string()),
        device.first_seen,
        device.last_seen,
        first_location,
        last_location,
        strongest_location,
    )
}

fn format_bluetooth_passive_section(device: &BluetoothDeviceRecord) -> String {
    let mfgr = if device.mfgr_ids.is_empty() {
        "None observed".to_string()
    } else if device.mfgr_names.is_empty() {
        device.mfgr_ids.join(", ")
    } else {
        device
            .mfgr_ids
            .iter()
            .enumerate()
            .map(|(idx, id)| {
                let name = device
                    .mfgr_names
                    .get(idx)
                    .cloned()
                    .unwrap_or_else(|| "Unknown".to_string());
                format!("{} ({})", id, name)
            })
            .collect::<Vec<_>>()
            .join(", ")
    };

    let uuids = if device.uuids.is_empty() {
        "None observed".to_string()
    } else if device.uuid_names.is_empty() {
        device.uuids.join(", ")
    } else {
        device
            .uuids
            .iter()
            .enumerate()
            .map(|(idx, id)| {
                let name = device
                    .uuid_names
                    .get(idx)
                    .cloned()
                    .unwrap_or_else(|| "Unknown".to_string());
                format!("{} ({})", id, name)
            })
            .collect::<Vec<_>>()
            .join(", ")
    };

    format!("MFGR IDs: {}\nUUIDs: {}", mfgr, uuids)
}

fn format_bluetooth_active_summary(device: &BluetoothDeviceRecord) -> String {
    let Some(active) = device.active_enumeration.as_ref() else {
        return "Not yet enumerated. Use Connect & Enumerate to actively query services and characteristics.".to_string();
    };

    format!(
        "Last Enumerated: {}\nConnected: {}\nPaired: {}\nTrusted: {}\nBlocked: {}\nServices Resolved: {}\nTx Power: {}\nBattery: {}\nAppearance: {}\nIcon: {}\nModalias: {}\nLast Error: {}",
        active
            .last_enumerated
            .map(|value| value.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Unknown".to_string()),
        bool_text(active.connected),
        bool_text(active.paired),
        bool_text(active.trusted),
        bool_text(active.blocked),
        bool_text(active.services_resolved),
        active
            .tx_power_dbm
            .map(|value| format!("{value} dBm"))
            .unwrap_or_else(|| "Unknown".to_string()),
        active
            .battery_percent
            .map(|value| format!("{value}%"))
            .unwrap_or_else(|| "Unknown".to_string()),
        active
            .appearance_code
            .map(|code| {
                let label = active
                    .appearance_name
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string());
                format!("{} (0x{:04X})", label, code)
            })
            .unwrap_or_else(|| "Unknown".to_string()),
        active.icon.clone().unwrap_or_else(|| "Unknown".to_string()),
        active
            .modalias
            .clone()
            .unwrap_or_else(|| "Unknown".to_string()),
        active
            .last_error
            .clone()
            .unwrap_or_else(|| "None".to_string()),
    )
}

fn format_bluetooth_readable_attributes(device: &BluetoothDeviceRecord) -> String {
    let Some(active) = device.active_enumeration.as_ref() else {
        return "Not yet enumerated.".to_string();
    };
    if active.readable_attributes.is_empty() {
        return "None read".to_string();
    }
    active
        .readable_attributes
        .iter()
        .map(|attribute| {
            let name = attribute.name.as_deref().unwrap_or("Unknown Attribute");
            format!("- {} ({}): {}", name, attribute.uuid, attribute.value)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_bluetooth_services(device: &BluetoothDeviceRecord) -> String {
    let Some(active) = device.active_enumeration.as_ref() else {
        return "Not yet enumerated.".to_string();
    };
    if active.services.is_empty() {
        return "None enumerated".to_string();
    }
    active
        .services
        .iter()
        .map(|service| {
            let name = service.name.as_deref().unwrap_or("Unknown Service");
            format!(
                "- {} ({}){}",
                name,
                service.uuid,
                if service.primary { " [primary]" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_bluetooth_characteristics(device: &BluetoothDeviceRecord) -> String {
    let Some(active) = device.active_enumeration.as_ref() else {
        return "Not yet enumerated.".to_string();
    };
    if active.characteristics.is_empty() {
        return "None enumerated".to_string();
    }
    active
        .characteristics
        .iter()
        .map(|characteristic| {
            let name = characteristic
                .name
                .as_deref()
                .unwrap_or("Unknown Characteristic");
            let service = characteristic
                .service_name
                .as_deref()
                .or(characteristic.service_uuid.as_deref())
                .unwrap_or("Unknown Service");
            let flags = if characteristic.flags.is_empty() {
                "no flags".to_string()
            } else {
                characteristic.flags.join(", ")
            };
            format!(
                "- {} ({}) via {} [{}]",
                name, characteristic.uuid, service, flags
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_bluetooth_descriptors(device: &BluetoothDeviceRecord) -> String {
    let Some(active) = device.active_enumeration.as_ref() else {
        return "Not yet enumerated.".to_string();
    };
    if active.descriptors.is_empty() {
        return "None enumerated".to_string();
    }
    active
        .descriptors
        .iter()
        .map(|descriptor| {
            let name = descriptor.name.as_deref().unwrap_or("Unknown Descriptor");
            let characteristic = descriptor
                .characteristic_name
                .as_deref()
                .or(descriptor.characteristic_uuid.as_deref())
                .unwrap_or("Unknown Characteristic");
            let value = descriptor
                .value
                .as_deref()
                .map(|value| format!(": {}", value))
                .unwrap_or_default();
            format!(
                "- {} ({}) via {}{}",
                name, descriptor.uuid, characteristic, value
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn set_bluetooth_detail_sections(
    device: &BluetoothDeviceRecord,
    identity_label: &Label,
    passive_label: &Label,
    active_summary_label: &Label,
    readable_label: &Label,
    services_label: &Label,
    characteristics_label: &Label,
    descriptors_label: &Label,
) {
    identity_label.set_text(&format_bluetooth_identity_section(device));
    passive_label.set_text(&format_bluetooth_passive_section(device));
    active_summary_label.set_text(&format_bluetooth_active_summary(device));
    readable_label.set_text(&format_bluetooth_readable_attributes(device));
    services_label.set_text(&format_bluetooth_services(device));
    characteristics_label.set_text(&format_bluetooth_characteristics(device));
    descriptors_label.set_text(&format_bluetooth_descriptors(device));
}

fn clear_bluetooth_detail_sections(
    identity_label: &Label,
    passive_label: &Label,
    active_summary_label: &Label,
    readable_label: &Label,
    services_label: &Label,
    characteristics_label: &Label,
    descriptors_label: &Label,
) {
    for label in [
        identity_label,
        passive_label,
        active_summary_label,
        readable_label,
        services_label,
        characteristics_label,
        descriptors_label,
    ] {
        label.set_text("");
    }
}

fn format_observation_location_time(obs: &GeoObservation) -> String {
    format!(
        "{:.6}, {:.6} at {}",
        obs.latitude,
        obs.longitude,
        obs.timestamp.format("%Y-%m-%d %H:%M:%S UTC")
    )
}

fn format_strongest_observation(obs: Option<&GeoObservation>, has_observations: bool) -> String {
    match obs {
        Some(point) => {
            let rssi = point
                .rssi_dbm
                .map(|v| format!(" ({} dBm)", v))
                .unwrap_or_default();
            format!("{}{}", format_observation_location_time(point), rssi)
        }
        None if has_observations => "Unknown (no RSSI captured)".to_string(),
        None => "Unknown".to_string(),
    }
}

fn draw_packet_pie(ctx: &cairo::Context, width: f64, height: f64, mix: &PacketTypeBreakdown) {
    let total = mix.total() as f64;
    ctx.set_source_rgb(0.12, 0.12, 0.14);
    let _ = ctx.paint();

    if total <= 0.0 {
        ctx.set_source_rgb(0.8, 0.8, 0.8);
        ctx.move_to(20.0, height / 2.0);
        let _ = ctx.show_text("No packet data yet");
        return;
    }

    let cx = width * 0.5;
    let cy = height * 0.5;
    let radius = width.min(height) * 0.35;

    let slices = [
        (mix.management as f64, (0.3, 0.8, 1.0)),
        (mix.control as f64, (0.9, 0.6, 0.1)),
        (mix.data as f64, (0.2, 0.9, 0.3)),
        (mix.other as f64, (0.85, 0.2, 0.2)),
    ];

    let mut angle = -std::f64::consts::FRAC_PI_2;
    for (value, (r, g, b)) in slices {
        if value <= 0.0 {
            continue;
        }
        let next = angle + (value / total) * std::f64::consts::TAU;
        ctx.set_source_rgb(r, g, b);
        ctx.move_to(cx, cy);
        ctx.arc(cx, cy, radius, angle, next);
        ctx.close_path();
        let _ = ctx.fill();
        angle = next;
    }
}

fn draw_wifi_geiger_meter(
    ctx: &cairo::Context,
    width: f64,
    height: f64,
    state: &WifiGeigerUiState,
) {
    ctx.set_source_rgb(0.06, 0.08, 0.1);
    let _ = ctx.paint();

    let Some(target) = state.target.as_ref() else {
        ctx.set_source_rgb(0.85, 0.87, 0.9);
        ctx.move_to(28.0, height * 0.5);
        let _ = ctx.show_text("Select a target to start the Wi-Fi geiger counter");
        return;
    };

    let rssi = state
        .latest_update
        .as_ref()
        .map(|update| update.rssi_dbm)
        .unwrap_or(-100);
    let tone = state
        .latest_update
        .as_ref()
        .map(|update| update.tone_hz)
        .unwrap_or(0);
    let needle_fraction = state.needle_fraction.clamp(0.0, 1.0);
    let normalized = normalize_rssi_fraction(rssi);
    let pulse_strength = state
        .last_update_at
        .map(|last| (1.0 - (last.elapsed().as_secs_f64() / 0.55)).clamp(0.0, 1.0))
        .unwrap_or(0.0);

    let width = width.max(320.0);
    let height = height.max(220.0);
    let split_x = if width >= 760.0 {
        width * 0.60
    } else {
        width * 0.56
    };
    let cx = width * 0.28;
    let cy = height * 0.72;
    let radius = (width.min(height) * 0.29).clamp(72.0, 150.0);
    let start_angle = std::f64::consts::PI * 0.78;
    let end_angle = std::f64::consts::PI * 0.22;
    let title_x = split_x;
    let title_y = height * 0.16;
    let heading_font = (height * 0.05).clamp(16.0, 26.0);
    let value_font = (height * 0.13).clamp(28.0, 48.0);
    let body_font = (height * 0.055).clamp(14.0, 22.0);
    let line_gap = body_font + 8.0;

    ctx.set_line_width(18.0);
    ctx.set_source_rgb(0.16, 0.18, 0.22);
    ctx.arc_negative(cx, cy, radius, start_angle, end_angle);
    let _ = ctx.stroke();

    for (from, to, color) in [
        (0.0, 0.33, (0.24, 0.77, 0.39)),
        (0.33, 0.7, (0.96, 0.76, 0.18)),
        (0.7, 1.0, (0.91, 0.28, 0.23)),
    ] {
        let a0 = start_angle - (start_angle - end_angle) * from;
        let a1 = start_angle - (start_angle - end_angle) * to;
        ctx.set_source_rgb(color.0, color.1, color.2);
        ctx.arc_negative(cx, cy, radius, a0, a1);
        let _ = ctx.stroke();
    }

    ctx.set_line_width(1.0);
    ctx.set_source_rgb(0.86, 0.88, 0.9);
    for idx in 0..=7 {
        let frac = idx as f64 / 7.0;
        let angle = start_angle - (start_angle - end_angle) * frac;
        let inner = radius - 24.0;
        let outer = radius + 12.0;
        let x0 = cx + inner * angle.cos();
        let y0 = cy - inner * angle.sin();
        let x1 = cx + outer * angle.cos();
        let y1 = cy - outer * angle.sin();
        ctx.move_to(x0, y0);
        ctx.line_to(x1, y1);
        let _ = ctx.stroke();
    }

    let needle_angle = start_angle - (start_angle - end_angle) * needle_fraction;
    let needle_len = radius - 36.0;
    ctx.set_source_rgb(0.97, 0.98, 0.99);
    ctx.set_line_width(4.0);
    ctx.move_to(cx, cy);
    ctx.line_to(
        cx + needle_len * needle_angle.cos(),
        cy - needle_len * needle_angle.sin(),
    );
    let _ = ctx.stroke();

    ctx.set_source_rgb(0.96, 0.28 + 0.45 * pulse_strength, 0.18);
    ctx.arc(
        cx,
        cy,
        11.0 + pulse_strength * 4.0,
        0.0,
        std::f64::consts::TAU,
    );
    let _ = ctx.fill();

    ctx.set_source_rgb(0.86, 0.88, 0.9);
    ctx.set_font_size(heading_font);
    ctx.move_to(title_x, title_y);
    let _ = ctx.show_text("GEIGER");
    ctx.move_to(title_x, title_y + heading_font + 6.0);
    let _ = ctx.show_text("Signal Strength");

    ctx.set_font_size(value_font);
    ctx.move_to(title_x, title_y + heading_font + value_font + 28.0);
    let _ = ctx.show_text(&format!("{rssi} dBm"));

    ctx.set_font_size(body_font);
    let metrics_top = title_y + heading_font + value_font + 58.0;
    ctx.move_to(title_x, metrics_top);
    let _ = ctx.show_text(&format!("Tone: {tone} Hz"));
    ctx.move_to(title_x, metrics_top + line_gap);
    let _ = ctx.show_text(&format!("Channel: {}", target.channel));
    ctx.move_to(title_x, metrics_top + line_gap * 2.0);
    let _ = ctx.show_text(&format!(
        "Pulse: {}",
        if pulse_strength > 0.2 {
            "Active"
        } else {
            "Idle"
        }
    ));

    let bar_x = title_x;
    let bar_y = metrics_top + line_gap * 2.0 + 24.0;
    let bar_w = (width - bar_x - 32.0).max(140.0);
    let bar_h = 24.0;
    ctx.set_source_rgb(0.16, 0.18, 0.22);
    ctx.rectangle(bar_x, bar_y, bar_w, bar_h);
    let _ = ctx.fill();
    ctx.set_source_rgb(0.28, 0.82, 0.97);
    ctx.rectangle(bar_x, bar_y, bar_w * normalized, bar_h);
    let _ = ctx.fill();
    ctx.set_source_rgb(0.86, 0.88, 0.9);
    ctx.rectangle(bar_x, bar_y, bar_w, bar_h);
    let _ = ctx.stroke();

    ctx.move_to(bar_x, bar_y + body_font + 18.0);
    let _ = ctx.show_text(&format!("Tracking {}", target.display_name));
}

fn draw_channel_usage_chart(
    ctx: &cairo::Context,
    width: f64,
    height: f64,
    usage: &[ChannelUsagePoint],
    band_filter: Option<&str>,
) {
    ctx.set_source_rgb(0.07, 0.08, 0.1);
    let _ = ctx.paint();

    let selected = usage
        .iter()
        .filter(|u| match band_filter {
            Some("2.4") => matches!(u.band, SpectrumBand::Ghz2_4),
            Some("5") => matches!(u.band, SpectrumBand::Ghz5),
            Some("6") => matches!(u.band, SpectrumBand::Ghz6),
            _ => true,
        })
        .cloned()
        .collect::<Vec<_>>();

    if selected.is_empty() {
        ctx.set_source_rgb(0.8, 0.8, 0.8);
        ctx.move_to(20.0, height / 2.0);
        let _ = ctx.show_text("No channel usage captured yet");
        return;
    }

    let mut latest_by_channel: HashMap<u16, f32> = HashMap::new();
    for p in selected {
        latest_by_channel.insert(p.channel, p.utilization_percent);
    }

    let mut channels = latest_by_channel.keys().copied().collect::<Vec<_>>();
    channels.sort_unstable();

    let margin = 40.0;
    let plot_w = (width - 2.0 * margin).max(20.0);
    let plot_h = (height - 2.0 * margin).max(20.0);

    ctx.set_source_rgb(0.25, 0.26, 0.3);
    ctx.rectangle(margin, margin, plot_w, plot_h);
    let _ = ctx.stroke();

    let bar_w = (plot_w / channels.len() as f64).max(5.0);
    for (i, ch) in channels.iter().enumerate() {
        let util = latest_by_channel
            .get(ch)
            .copied()
            .unwrap_or(0.0)
            .clamp(0.0, 100.0);
        let h = plot_h * (util as f64 / 100.0);
        let x = margin + i as f64 * bar_w + 1.0;
        let y = margin + plot_h - h;

        ctx.set_source_rgb(0.22, 0.72, 0.98);
        ctx.rectangle(x, y, (bar_w - 2.0).max(2.0), h);
        let _ = ctx.fill();

        ctx.set_source_rgb(0.8, 0.8, 0.8);
        ctx.move_to(x, margin + plot_h + 14.0);
        let _ = ctx.show_text(&ch.to_string());
    }
}

fn install_ui_css() -> gtk::CssProvider {
    let base_provider = gtk::CssProvider::new();
    base_provider.load_from_data(
        "
.heading {
  font-weight: 700;
}
.table-cell {
  font-family: monospace;
}
.column-filter {
  padding-left: 0;
  padding-right: 0;
  border: none;
  box-shadow: none;
  background-color: transparent;
}
.column-filter text {
  padding-left: 0;
  padding-right: 0;
}
.sort-header {
  text-decoration-line: underline;
}
.row-handshake {
  background-color: rgba(255, 215, 0, 0.32);
}
",
    );

    let watchlist_provider = gtk::CssProvider::new();
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &base_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        gtk::style_context_add_provider_for_display(
            &display,
            &watchlist_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    watchlist_provider
}

fn rebuild_header_container(holder: &GtkBox, header: &Grid, filter_bar: Option<&Grid>) {
    clear_box(holder);
    holder.append(header);
    if let Some(filter_bar) = filter_bar {
        holder.append(filter_bar);
    }
}

#[derive(Debug, Clone)]
struct WatchlistMatch {
    label: String,
    css_class: String,
    alert_key: String,
}

fn normalize_watch_mac(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .map(|ch| ch.to_ascii_uppercase())
        .collect()
}

fn normalize_watch_name(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn looks_like_mac(value: &str) -> bool {
    normalize_watch_mac(value).len() == 12
}

fn normalize_watchlist_color(raw: &str) -> String {
    let trimmed = raw.trim();
    let candidate = if trimmed.is_empty() {
        crate::settings::default_watchlist_color_hex()
    } else if trimmed.starts_with('#') {
        trimmed.to_string()
    } else {
        format!("#{trimmed}")
    };
    let hex = candidate.trim_start_matches('#');
    if (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        format!("#{hex}")
    } else {
        crate::settings::default_watchlist_color_hex()
    }
}

fn parse_watchlist_color_rgba(color_hex: &str, alpha: f64) -> String {
    let hex = color_hex.trim_start_matches('#');
    let (r, g, b) = if hex.len() >= 6 {
        (
            u8::from_str_radix(&hex[0..2], 16).unwrap_or(46),
            u8::from_str_radix(&hex[2..4], 16).unwrap_or(204),
            u8::from_str_radix(&hex[4..6], 16).unwrap_or(113),
        )
    } else {
        (46, 204, 113)
    };
    format!("rgba({r}, {g}, {b}, {alpha:.3})")
}

fn watchlist_entry_label(entry: &WatchlistEntry) -> String {
    if !entry.label.trim().is_empty() {
        entry.label.trim().to_string()
    } else if !entry.name.trim().is_empty() {
        entry.name.trim().to_string()
    } else if !entry.mac.trim().is_empty() {
        entry.mac.trim().to_string()
    } else {
        "Watchlist".to_string()
    }
}

fn watchlist_css_class(index: usize) -> String {
    format!("watchlist-row-{index}")
}

fn watchlist_css_classes(watchlists: &WatchlistSettings) -> Vec<String> {
    watchlists
        .entries
        .iter()
        .enumerate()
        .map(|(index, _)| watchlist_css_class(index))
        .collect()
}

fn apply_watchlist_css(provider: &gtk::CssProvider, watchlists: &WatchlistSettings) {
    let css = watchlists
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let class_name = watchlist_css_class(index);
            let color_hex = normalize_watchlist_color(&entry.color_hex);
            let background = parse_watchlist_color_rgba(&color_hex, 0.30);
            format!(".{class_name} {{ background-color: {background}; }}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    provider.load_from_data(&css);
}

fn migrate_watchlist_settings(watchlists: &mut WatchlistSettings) {
    let mut migrated = watchlists.entries.clone();
    for raw in watchlists.networks.iter().chain(watchlists.devices.iter()) {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut entry = WatchlistEntry {
            label: trimmed.to_string(),
            device_type: WatchlistDeviceType::Wifi,
            mac: String::new(),
            name: String::new(),
            color_hex: crate::settings::default_watchlist_color_hex(),
        };
        if looks_like_mac(trimmed) {
            entry.mac = trimmed.to_string();
        } else {
            entry.name = trimmed.to_string();
        }
        migrated.push(entry);
    }

    let mut seen = HashSet::new();
    let mut normalized_entries = Vec::new();
    for mut entry in migrated {
        entry.label = entry.label.trim().to_string();
        entry.mac = entry.mac.trim().to_string();
        entry.name = entry.name.trim().to_string();
        entry.color_hex = normalize_watchlist_color(&entry.color_hex);
        if entry.mac.is_empty() && entry.name.is_empty() {
            continue;
        }
        let key = format!(
            "{:?}|{}|{}|{}",
            entry.device_type,
            normalize_watch_mac(&entry.mac),
            normalize_watch_name(&entry.name),
            watchlist_entry_label(&entry)
        );
        if seen.insert(key) {
            normalized_entries.push(entry);
        }
    }

    watchlists.entries = normalized_entries;
    watchlists.networks.clear();
    watchlists.devices.clear();
}

fn watchlist_entry_matches_mac(entry: &WatchlistEntry, mac: &str) -> bool {
    !entry.mac.trim().is_empty() && normalize_watch_mac(&entry.mac) == normalize_watch_mac(mac)
}

fn watchlist_entry_matches_name(entry: &WatchlistEntry, name: &str) -> bool {
    !entry.name.trim().is_empty() && normalize_watch_name(&entry.name) == normalize_watch_name(name)
}

fn build_watchlist_match(index: usize, entry: &WatchlistEntry) -> WatchlistMatch {
    WatchlistMatch {
        label: watchlist_entry_label(entry),
        css_class: watchlist_css_class(index),
        alert_key: format!(
            "{:?}:{}:{}:{}",
            entry.device_type,
            index,
            normalize_watch_mac(&entry.mac),
            normalize_watch_name(&entry.name)
        ),
    }
}

fn ap_watchlist_match(
    ap: &AccessPointRecord,
    watchlists: &WatchlistSettings,
) -> Option<WatchlistMatch> {
    watchlists
        .entries
        .iter()
        .enumerate()
        .find_map(|(index, entry)| {
            if entry.device_type != WatchlistDeviceType::Wifi {
                return None;
            }
            let mac_match = watchlist_entry_matches_mac(entry, &ap.bssid);
            let name_match = ap
                .ssid
                .as_deref()
                .map(|ssid| watchlist_entry_matches_name(entry, ssid))
                .unwrap_or(false);
            if mac_match || name_match {
                Some(build_watchlist_match(index, entry))
            } else {
                None
            }
        })
}

fn client_watchlist_match(
    client: &ClientRecord,
    aps: &[AccessPointRecord],
    watchlists: &WatchlistSettings,
) -> Option<WatchlistMatch> {
    watchlists
        .entries
        .iter()
        .enumerate()
        .find_map(|(index, entry)| {
            if entry.device_type != WatchlistDeviceType::Wifi {
                return None;
            }
            let mac_match = watchlist_entry_matches_mac(entry, &client.mac);
            let associated_name_match = associated_ssid_for_client(aps, client)
                .as_deref()
                .map(|ssid| watchlist_entry_matches_name(entry, ssid))
                .unwrap_or(false);
            let probe_match = client
                .probes
                .iter()
                .any(|probe| watchlist_entry_matches_name(entry, probe));
            if mac_match || associated_name_match || probe_match {
                Some(build_watchlist_match(index, entry))
            } else {
                None
            }
        })
}

fn bluetooth_watchlist_match(
    device: &BluetoothDeviceRecord,
    watchlists: &WatchlistSettings,
) -> Option<WatchlistMatch> {
    watchlists
        .entries
        .iter()
        .enumerate()
        .find_map(|(index, entry)| {
            if entry.device_type != WatchlistDeviceType::Bluetooth {
                return None;
            }
            let mac_match = watchlist_entry_matches_mac(entry, &device.mac);
            let name_match = [
                bluetooth_display_name(device),
                device.advertised_name.clone().unwrap_or_default(),
                device.alias.clone().unwrap_or_default(),
            ]
            .iter()
            .any(|name| !name.is_empty() && watchlist_entry_matches_name(entry, name));
            if mac_match || name_match {
                Some(build_watchlist_match(index, entry))
            } else {
                None
            }
        })
}

fn client_seen_on_ap(client: &ClientRecord, ap_bssid: &str) -> bool {
    client.associated_ap.as_deref() == Some(ap_bssid)
}

fn clients_currently_on_ap(clients: &[ClientRecord], ap_bssid: &str) -> Vec<ClientRecord> {
    clients
        .iter()
        .filter(|client| client_seen_on_ap(client, ap_bssid))
        .cloned()
        .collect()
}

fn emit_alert_tone(freq_hz: u32, duration_ms: u32) {
    let _ = std::process::Command::new("beep")
        .arg("-f")
        .arg(freq_hz.to_string())
        .arg("-l")
        .arg(duration_ms.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn attach_ap_context_menu(
    window: &ApplicationWindow,
    notebook: &Notebook,
    ap_list: &ListBox,
    state: Rc<RefCell<AppState>>,
    wifi_geiger_state: Rc<RefCell<WifiGeigerUiState>>,
) {
    let popover = Popover::new();
    popover.set_parent(ap_list);
    let box_ = GtkBox::new(Orientation::Vertical, 4);
    let view_btn = Button::with_label("View Details");
    let locate_btn = Button::with_label("Locate Device");
    let lock_btn = Button::with_label("Lock to AP");
    let unlock_btn = Button::with_label("Unlock WiFi Card");

    box_.append(&view_btn);
    box_.append(&locate_btn);
    box_.append(&lock_btn);
    box_.append(&unlock_btn);
    popover.set_child(Some(&box_));

    {
        let state = state.clone();
        let ap_list = ap_list.clone();
        let window = window.clone();
        view_btn.connect_clicked(move |_| {
            if let Some(ap) = selected_ap(&state, &ap_list) {
                open_ap_details_dialog(&window, &ap);
            }
        });
    }

    {
        let state = state.clone();
        let ap_list = ap_list.clone();
        let notebook = notebook.clone();
        let wifi_geiger_state = wifi_geiger_state.clone();
        locate_btn.connect_clicked(move |_| {
            if let Some(ap) = selected_ap(&state, &ap_list) {
                start_wifi_geiger_tracking_for_ap(&state, &wifi_geiger_state, &notebook, &ap);
            }
        });
    }

    {
        let state = state.clone();
        let ap_list = ap_list.clone();
        lock_btn.connect_clicked(move |_| {
            if let Some(ap) = selected_ap(&state, &ap_list) {
                if let Some(channel) = ap.channel {
                    let label = ap.ssid.clone().unwrap_or_else(|| ap.bssid.clone());
                    let _ = state
                        .borrow_mut()
                        .lock_wifi_to_channel(channel, "HT20", label);
                }
            }
        });
    }

    {
        let state = state.clone();
        unlock_btn.connect_clicked(move |_| {
            let _ = state.borrow_mut().unlock_wifi_card();
        });
    }

    let click = GestureClick::new();
    click.set_button(3);
    {
        let popover = popover.clone();
        let ap_list = ap_list.clone();
        click.connect_pressed(move |_, _, x, y| {
            if let Some(row) = ap_list.row_at_y(y as i32) {
                ap_list.select_row(Some(&row));
            }
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        });
    }
    ap_list.add_controller(click);
}

fn attach_client_context_menu(
    window: &ApplicationWindow,
    notebook: &Notebook,
    client_list: &ListBox,
    state: Rc<RefCell<AppState>>,
    wifi_geiger_state: Rc<RefCell<WifiGeigerUiState>>,
) {
    let popover = Popover::new();
    popover.set_parent(client_list);
    let box_ = GtkBox::new(Orientation::Vertical, 4);
    let locate_btn = Button::with_label("Locate Device");
    let view_btn = Button::with_label("View Details");
    let lock_btn = Button::with_label("Lock to AP");
    let unlock_btn = Button::with_label("Unlock WiFi Card");
    box_.append(&view_btn);
    box_.append(&locate_btn);
    box_.append(&lock_btn);
    box_.append(&unlock_btn);
    popover.set_child(Some(&box_));

    {
        let window = window.clone();
        let state = state.clone();
        let client_list = client_list.clone();
        view_btn.connect_clicked(move |_| {
            if let Some(client) = selected_client(&state, &client_list) {
                let aps = state.borrow().access_points.clone();
                open_client_details_dialog(&window, &client, &aps);
            }
        });
    }

    {
        let state = state.clone();
        let client_list = client_list.clone();
        let notebook = notebook.clone();
        let wifi_geiger_state = wifi_geiger_state.clone();
        locate_btn.connect_clicked(move |_| {
            if let Some(client) = selected_client(&state, &client_list) {
                start_wifi_geiger_tracking_for_client(
                    &state,
                    &wifi_geiger_state,
                    &notebook,
                    &client,
                );
            }
        });
    }

    {
        let state = state.clone();
        let client_list = client_list.clone();
        lock_btn.connect_clicked(move |_| {
            let Some(client) = selected_client(&state, &client_list) else {
                return;
            };
            let (channel, label) = {
                let s = state.borrow();
                let Some(ap_bssid) = client.associated_ap.as_ref() else {
                    drop(s);
                    state
                        .borrow_mut()
                        .push_status("selected client is not associated to an AP".to_string());
                    return;
                };
                let Some(ap) = s.access_points.iter().find(|ap| &ap.bssid == ap_bssid) else {
                    drop(s);
                    state.borrow_mut().push_status(format!(
                        "associated AP {} not yet present in the AP table",
                        ap_bssid
                    ));
                    return;
                };
                let Some(channel) = ap.channel else {
                    drop(s);
                    state
                        .borrow_mut()
                        .push_status("associated AP has no known channel to lock".to_string());
                    return;
                };
                (channel, ap.ssid.clone().unwrap_or_else(|| ap.bssid.clone()))
            };
            let _ = state
                .borrow_mut()
                .lock_wifi_to_channel(channel, "HT20", label);
        });
    }

    {
        let state = state.clone();
        unlock_btn.connect_clicked(move |_| {
            let _ = state.borrow_mut().unlock_wifi_card();
        });
    }

    let click = GestureClick::new();
    click.set_button(3);
    {
        let popover = popover.clone();
        let client_list = client_list.clone();
        click.connect_pressed(move |_, _, x, y| {
            if let Some(row) = client_list.row_at_y(y as i32) {
                client_list.select_row(Some(&row));
            }
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        });
    }
    client_list.add_controller(click);
}

fn attach_bluetooth_context_menu(
    bluetooth_list: &ListBox,
    state: Rc<RefCell<AppState>>,
    bluetooth_geiger_state: Rc<RefCell<BluetoothGeigerUiState>>,
) {
    let popover = Popover::new();
    popover.set_parent(bluetooth_list);
    let box_ = GtkBox::new(Orientation::Vertical, 4);
    let locate_btn = Button::with_label("Locate Device");
    let enumerate_btn = Button::with_label("Connect & Enumerate");
    let disconnect_btn = Button::with_label("Disconnect");
    box_.append(&locate_btn);
    box_.append(&enumerate_btn);
    box_.append(&disconnect_btn);
    popover.set_child(Some(&box_));

    {
        let state = state.clone();
        let bluetooth_list = bluetooth_list.clone();
        let bluetooth_geiger_state = bluetooth_geiger_state.clone();
        locate_btn.connect_clicked(move |_| {
            if let Some(device) = selected_bluetooth(&state, &bluetooth_list) {
                start_bluetooth_geiger_tracking(&state, &bluetooth_geiger_state, &device.mac);
            }
        });
    }

    {
        let state = state.clone();
        let bluetooth_list = bluetooth_list.clone();
        enumerate_btn.connect_clicked(move |_| {
            let Some(device) = selected_bluetooth(&state, &bluetooth_list) else {
                state
                    .borrow_mut()
                    .push_status("no bluetooth device selected for enumeration".to_string());
                return;
            };
            let (controller, sender) = {
                let s = state.borrow();
                (
                    s.settings.bluetooth_controller.clone(),
                    s.bluetooth_sender.clone(),
                )
            };
            state.borrow_mut().push_status(format!(
                "starting active bluetooth enumeration for {}",
                device.mac
            ));
            thread::spawn(move || {
                match bluetooth::connect_and_enumerate_device(controller.as_deref(), &device.mac) {
                    Ok(record) => {
                        let note = record
                            .active_enumeration
                            .as_ref()
                            .and_then(|active| active.last_error.clone())
                            .map(|error| {
                                format!(
                                    "active bluetooth enumeration completed with warning: {error}"
                                )
                            })
                            .unwrap_or_else(|| {
                                format!("active bluetooth enumeration completed for {}", record.mac)
                            });
                        let _ = sender.send(BluetoothEvent::DeviceSeen(record));
                        let _ = sender.send(BluetoothEvent::Log(note));
                    }
                    Err(err) => {
                        let _ = sender.send(BluetoothEvent::Log(format!(
                            "active bluetooth enumeration failed for {}: {err}",
                            device.mac
                        )));
                    }
                }
            });
        });
    }

    {
        let state = state.clone();
        let bluetooth_list = bluetooth_list.clone();
        disconnect_btn.connect_clicked(move |_| {
            let Some(device) = selected_bluetooth(&state, &bluetooth_list) else {
                state
                    .borrow_mut()
                    .push_status("no bluetooth device selected for disconnect".to_string());
                return;
            };
            let (controller, sender) = {
                let s = state.borrow();
                (
                    s.settings.bluetooth_controller.clone(),
                    s.bluetooth_sender.clone(),
                )
            };
            state
                .borrow_mut()
                .push_status(format!("disconnecting bluetooth device {}", device.mac));
            thread::spawn(move || {
                match bluetooth::disconnect_device(controller.as_deref(), &device.mac) {
                    Ok(()) => {
                        if let Ok(record) =
                            bluetooth::read_device_state(controller.as_deref(), &device.mac)
                        {
                            let _ = sender.send(BluetoothEvent::DeviceSeen(record));
                        }
                        let _ = sender.send(BluetoothEvent::Log(format!(
                            "bluetooth device disconnected: {}",
                            device.mac
                        )));
                    }
                    Err(err) => {
                        let _ = sender.send(BluetoothEvent::Log(format!(
                            "bluetooth disconnect failed for {}: {err}",
                            device.mac
                        )));
                    }
                }
            });
        });
    }

    let click = GestureClick::new();
    click.set_button(3);
    {
        let popover = popover.clone();
        let bluetooth_list = bluetooth_list.clone();
        click.connect_pressed(move |_, _, x, y| {
            if let Some(row) = bluetooth_list.row_at_y(y as i32) {
                bluetooth_list.select_row(Some(&row));
            }
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        });
    }
    bluetooth_list.add_controller(click);
}

fn wifi_geiger_target_for_ap(ap: &AccessPointRecord) -> Option<WifiGeigerTarget> {
    let channel = ap.channel?;
    let display_name = format!(
        "{} ({})",
        ap.ssid.clone().unwrap_or_else(|| "<hidden>".to_string()),
        ap.bssid
    );
    Some(WifiGeigerTarget {
        track_id: ap.bssid.clone(),
        display_name: display_name.clone(),
        channel,
    })
}

fn wifi_geiger_target_for_client(
    state: &AppState,
    client: &ClientRecord,
) -> Option<WifiGeigerTarget> {
    let ap_bssid = client.associated_ap.as_ref()?;
    let ap = state
        .access_points
        .iter()
        .find(|ap| &ap.bssid == ap_bssid)?;
    let channel = ap.channel?;
    let lock_label = ap.ssid.clone().unwrap_or_else(|| ap.bssid.clone());
    Some(WifiGeigerTarget {
        track_id: client.mac.clone(),
        display_name: format!("{} via {}", client.mac, lock_label),
        channel,
    })
}

fn set_wifi_geiger_preview(
    wifi_geiger_state: &Rc<RefCell<WifiGeigerUiState>>,
    target: WifiGeigerTarget,
    rssi_dbm: Option<i32>,
) {
    if let Some(stop) = wifi_geiger_state.borrow_mut().stop.take() {
        stop.store(true, Ordering::Relaxed);
    }

    let mut geiger = wifi_geiger_state.borrow_mut();
    geiger.receiver = None;
    geiger.target = Some(target);
    geiger.latest_update = rssi_dbm.map(|rssi| GeigerUpdate {
        rssi_dbm: rssi,
        tone_hz: capture::rssi_to_tone_hz(rssi),
    });
    geiger.last_update_at = None;
    geiger.needle_fraction = rssi_dbm.map(normalize_rssi_fraction).unwrap_or(0.0);
    geiger.target_fraction = geiger.needle_fraction;
    geiger.last_animation_at = Some(Instant::now());
}

fn set_wifi_geiger_preview_for_ap(
    wifi_geiger_state: &Rc<RefCell<WifiGeigerUiState>>,
    ap: &AccessPointRecord,
) {
    if let Some(target) = wifi_geiger_target_for_ap(ap) {
        set_wifi_geiger_preview(wifi_geiger_state, target, ap.rssi_dbm);
    }
}

fn set_wifi_geiger_preview_for_client(
    state: &AppState,
    wifi_geiger_state: &Rc<RefCell<WifiGeigerUiState>>,
    client: &ClientRecord,
) {
    if let Some(target) = wifi_geiger_target_for_client(state, client) {
        set_wifi_geiger_preview(wifi_geiger_state, target, client.rssi_dbm);
    }
}

fn sync_wifi_geiger_preview_for_ap_if_idle(
    wifi_geiger_state: &Rc<RefCell<WifiGeigerUiState>>,
    ap: &AccessPointRecord,
) {
    let should_update = {
        let geiger = wifi_geiger_state.borrow();
        geiger.receiver.is_none()
            && (geiger
                .target
                .as_ref()
                .map(|target| target.track_id.as_str())
                != Some(ap.bssid.as_str())
                || geiger.latest_update.as_ref().map(|update| update.rssi_dbm) != ap.rssi_dbm)
    };
    if should_update {
        set_wifi_geiger_preview_for_ap(wifi_geiger_state, ap);
    }
}

fn sync_wifi_geiger_preview_for_client_if_idle(
    state: &AppState,
    wifi_geiger_state: &Rc<RefCell<WifiGeigerUiState>>,
    client: &ClientRecord,
) {
    let should_update = {
        let geiger = wifi_geiger_state.borrow();
        geiger.receiver.is_none()
            && (geiger
                .target
                .as_ref()
                .map(|target| target.track_id.as_str())
                != Some(client.mac.as_str())
                || geiger.latest_update.as_ref().map(|update| update.rssi_dbm) != client.rssi_dbm)
    };
    if should_update {
        set_wifi_geiger_preview_for_client(state, wifi_geiger_state, client);
    }
}

fn clear_wifi_geiger_preview(wifi_geiger_state: &Rc<RefCell<WifiGeigerUiState>>) {
    if let Some(stop) = wifi_geiger_state.borrow_mut().stop.take() {
        stop.store(true, Ordering::Relaxed);
    }

    let mut geiger = wifi_geiger_state.borrow_mut();
    geiger.receiver = None;
    geiger.stop = None;
    geiger.target = None;
    geiger.latest_update = None;
    geiger.last_update_at = None;
    geiger.needle_fraction = 0.0;
    geiger.target_fraction = 0.0;
    geiger.last_animation_at = Some(Instant::now());
}

fn stop_wifi_geiger_tracking(
    state: &Rc<RefCell<AppState>>,
    wifi_geiger_state: &Rc<RefCell<WifiGeigerUiState>>,
) {
    if let Some(stop) = wifi_geiger_state.borrow_mut().stop.take() {
        stop.store(true, Ordering::Relaxed);
    }
    wifi_geiger_state.borrow_mut().receiver = None;
    state
        .borrow_mut()
        .push_status("wifi geiger tracking stopped".to_string());
}

fn start_wifi_geiger_tracking_for_ap(
    state: &Rc<RefCell<AppState>>,
    wifi_geiger_state: &Rc<RefCell<WifiGeigerUiState>>,
    notebook: &Notebook,
    ap: &AccessPointRecord,
) {
    let Some(target) = wifi_geiger_target_for_ap(ap) else {
        state
            .borrow_mut()
            .push_status("selected AP has no known channel for RSSI geiger tracking".to_string());
        return;
    };
    start_wifi_geiger_tracking_target(state, wifi_geiger_state, notebook, target);
}

fn start_wifi_geiger_tracking_for_client(
    state: &Rc<RefCell<AppState>>,
    wifi_geiger_state: &Rc<RefCell<WifiGeigerUiState>>,
    notebook: &Notebook,
    client: &ClientRecord,
) {
    let target = {
        let s = state.borrow();
        wifi_geiger_target_for_client(&s, client)
    };
    let Some(target) = target else {
        state.borrow_mut().push_status(
            "selected client has no associated AP with a known channel for RSSI geiger tracking"
                .to_string(),
        );
        return;
    };
    start_wifi_geiger_tracking_target(state, wifi_geiger_state, notebook, target);
}

fn start_wifi_geiger_tracking_target(
    state: &Rc<RefCell<AppState>>,
    wifi_geiger_state: &Rc<RefCell<WifiGeigerUiState>>,
    notebook: &Notebook,
    target: WifiGeigerTarget,
) {
    if let Some(stop) = wifi_geiger_state.borrow_mut().stop.take() {
        stop.store(true, Ordering::Relaxed);
    }

    let Some(interface) = state.borrow().active_wifi_interface_name() else {
        state.borrow_mut().push_status(
            "no active Wi-Fi interface available for RSSI geiger tracking".to_string(),
        );
        return;
    };

    let (tx, rx) = unbounded::<GeigerUpdate>();
    let stop = Arc::new(AtomicBool::new(false));
    let _ = capture::start_geiger_mode(
        &interface,
        &target.track_id,
        target.channel,
        tx,
        stop.clone(),
    );

    let mut geiger = wifi_geiger_state.borrow_mut();
    geiger.receiver = Some(rx);
    geiger.stop = Some(stop);
    geiger.target = Some(target.clone());
    if geiger.latest_update.is_none() {
        geiger.needle_fraction = 0.0;
        geiger.target_fraction = 0.0;
    }
    geiger.last_animation_at = Some(Instant::now());
    drop(geiger);

    notebook.set_current_page(Some(DETAIL_GEIGER_TAB_INDEX));
    state.borrow_mut().push_status(format!(
        "wifi geiger tracking {} on {} channel {}",
        target.display_name, interface, target.channel
    ));
}

fn open_ap_details_dialog(window: &ApplicationWindow, ap: &AccessPointRecord) {
    let dialog = Dialog::builder()
        .transient_for(window)
        .modal(true)
        .title("AP Details")
        .default_width(700)
        .default_height(520)
        .build();

    dialog.add_button("Close", ResponseType::Close);
    let area = dialog.content_area();
    let label = Label::new(Some(&format_ap_detail_text(ap)));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_selectable(true);
    area.append(&label);
    dialog.connect_response(|d, _| d.close());
    dialog.present();
}

fn open_client_details_dialog(
    window: &ApplicationWindow,
    client: &ClientRecord,
    aps: &[AccessPointRecord],
) {
    let dialog = Dialog::builder()
        .transient_for(window)
        .modal(true)
        .title("Client Details")
        .default_width(700)
        .default_height(460)
        .build();

    dialog.add_button("Close", ResponseType::Close);
    let area = dialog.content_area();
    let label = Label::new(Some(&format_client_detail_text(client, aps)));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_selectable(true);
    area.append(&label);
    dialog.connect_response(|d, _| d.close());
    dialog.present();
}

fn export_selected_ap_csv(state: &Rc<RefCell<AppState>>, ap_list: &ListBox) {
    let Some(ap) = selected_ap(state, ap_list) else {
        return;
    };

    let mut s = state.borrow_mut();
    let _ = s.exporter.export_ap_detail_csv(&ap);
    s.push_status("exported AP CSV details".to_string());
}

fn export_selected_client_csv(state: &Rc<RefCell<AppState>>, client_list: &ListBox) {
    let Some(client) = selected_client(state, client_list) else {
        return;
    };

    let mut s = state.borrow_mut();
    let _ = s.exporter.export_client_detail_csv(&client);
    s.push_status("exported client CSV details".to_string());
}

fn selected_ap(state: &Rc<RefCell<AppState>>, ap_list: &ListBox) -> Option<AccessPointRecord> {
    let row = ap_list.selected_row()?;
    let key = row.widget_name().to_string();

    let state = state.borrow();
    state
        .access_points
        .iter()
        .find(|ap| ap.bssid == key)
        .cloned()
}

fn selected_client(state: &Rc<RefCell<AppState>>, client_list: &ListBox) -> Option<ClientRecord> {
    let row = client_list.selected_row()?;
    let key = row.widget_name().to_string();
    let state = state.borrow();
    state.clients.iter().find(|c| c.mac == key).cloned()
}

fn selected_bluetooth(
    state: &Rc<RefCell<AppState>>,
    bluetooth_list: &ListBox,
) -> Option<BluetoothDeviceRecord> {
    let row = bluetooth_list.selected_row()?;
    let key = row.widget_name().to_string();
    let state = state.borrow();
    state
        .bluetooth_devices
        .iter()
        .find(|device| device.mac == key)
        .cloned()
}

fn start_bluetooth_geiger_tracking(
    state: &Rc<RefCell<AppState>>,
    geiger_state: &Rc<RefCell<BluetoothGeigerUiState>>,
    target_mac: &str,
) {
    if let Some(stop) = geiger_state.borrow_mut().stop.take() {
        stop.store(true, Ordering::Relaxed);
    }

    let (tx, rx) = unbounded::<GeigerUpdate>();
    let stop = Arc::new(AtomicBool::new(false));

    let controller = state.borrow().settings.bluetooth_controller.clone();
    let _ = bluetooth::start_geiger_mode(controller.as_deref(), target_mac, tx, stop.clone());

    let mut gs = geiger_state.borrow_mut();
    gs.receiver = Some(rx);
    gs.stop = Some(stop);
    gs.target_mac = Some(target_mac.to_string());

    state
        .borrow_mut()
        .push_status(format!("bluetooth geiger tracking {}", target_mac));
}

fn open_layout_dialog(window: &ApplicationWindow, state: Rc<RefCell<AppState>>) {
    let (
        ap_layout_initial,
        client_layout_initial,
        assoc_layout_initial,
        handshake_alerts_initial,
        watchlist_alerts_initial,
    ) = {
        let s = state.borrow();
        (
            s.settings.ap_table_layout.clone(),
            s.settings.client_table_layout.clone(),
            s.settings.assoc_client_table_layout.clone(),
            s.settings.enable_handshake_alerts,
            s.settings.enable_watchlist_alerts,
        )
    };

    let dialog = Dialog::builder()
        .transient_for(window)
        .modal(true)
        .title("Layout")
        .default_width(860)
        .default_height(680)
        .build();
    dialog.add_button("Cancel", ResponseType::Cancel);
    dialog.add_button("Apply", ResponseType::Apply);

    let area = dialog.content_area();
    let notebook = Notebook::new();
    area.append(&notebook);

    let ap_columns = Rc::new(RefCell::new(ap_layout_initial.columns));
    let client_columns = Rc::new(RefCell::new(client_layout_initial.columns));
    let assoc_columns = Rc::new(RefCell::new(assoc_layout_initial.columns));

    let layout_tab_inner = GtkBox::new(Orientation::Vertical, 10);
    layout_tab_inner.set_margin_top(8);
    layout_tab_inner.set_margin_bottom(8);
    layout_tab_inner.set_margin_start(8);
    layout_tab_inner.set_margin_end(8);
    layout_tab_inner.append(&build_table_layout_editor(
        "Access Points Table",
        ap_columns.clone(),
        ap_column_label,
    ));
    layout_tab_inner.append(&build_table_layout_editor(
        "Clients Table",
        client_columns.clone(),
        client_column_label,
    ));
    layout_tab_inner.append(&build_table_layout_editor(
        "Associated Clients Table",
        assoc_columns.clone(),
        assoc_client_column_label,
    ));

    let layout_tab = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&layout_tab_inner)
        .build();
    notebook.append_page(&layout_tab, Some(&Label::new(Some("Table Layout"))));

    let alerts_tab = GtkBox::new(Orientation::Vertical, 8);
    alerts_tab.set_margin_top(8);
    alerts_tab.set_margin_bottom(8);
    alerts_tab.set_margin_start(8);
    alerts_tab.set_margin_end(8);

    let handshake_alerts_check = CheckButton::with_label("Enable handshake alerts");
    handshake_alerts_check.set_active(handshake_alerts_initial);
    let watchlist_alerts_check = CheckButton::with_label("Enable watchlist alerts");
    watchlist_alerts_check.set_active(watchlist_alerts_initial);

    alerts_tab.append(&handshake_alerts_check);
    alerts_tab.append(&watchlist_alerts_check);
    let note = Label::new(Some(
        "Structured watchlist entries are managed in Settings -> Preferences -> Alerts / Watchlists.",
    ));
    note.set_xalign(0.0);
    note.set_wrap(true);
    alerts_tab.append(&note);

    notebook.append_page(&alerts_tab, Some(&Label::new(Some("Alerts / Watchlists"))));

    {
        let state = state.clone();
        dialog.connect_response(move |d, resp| {
            if resp == ResponseType::Apply {
                let mut s = state.borrow_mut();

                s.settings.ap_table_layout.columns = ap_columns.borrow().clone();
                s.settings.client_table_layout.columns = client_columns.borrow().clone();
                s.settings.assoc_client_table_layout.columns = assoc_columns.borrow().clone();
                sanitize_table_layout(&mut s.settings.ap_table_layout, &default_ap_table_layout());
                sanitize_table_layout(
                    &mut s.settings.client_table_layout,
                    &default_client_table_layout(),
                );
                sanitize_table_layout(
                    &mut s.settings.assoc_client_table_layout,
                    &default_assoc_client_table_layout(),
                );
                migrate_assoc_client_table_layout(&mut s.settings.assoc_client_table_layout);

                s.settings.enable_handshake_alerts = handshake_alerts_check.is_active();
                s.settings.enable_watchlist_alerts = watchlist_alerts_check.is_active();

                s.alerted_watch_entities.clear();
                s.layout_dirty = true;
                s.save_settings_to_disk();
                s.push_status("layout and alert settings applied".to_string());
            }

            d.close();
        });
    }

    dialog.present();
}

fn build_table_layout_editor(
    title: &str,
    columns: Rc<RefCell<Vec<TableColumnLayout>>>,
    label_for: fn(&str) -> &'static str,
) -> GtkBox {
    let section = GtkBox::new(Orientation::Vertical, 6);
    let heading = Label::new(Some(title));
    heading.add_css_class("heading");
    heading.set_xalign(0.0);
    section.append(&heading);

    let rows_holder = GtkBox::new(Orientation::Vertical, 4);
    section.append(&rows_holder);

    type RenderFn = Box<dyn Fn()>;
    let renderer: Rc<RefCell<Option<RenderFn>>> = Rc::new(RefCell::new(None));
    let renderer_for_rows = renderer.clone();
    let rows_holder_for_render = rows_holder.clone();
    let columns_for_render = columns.clone();

    *renderer.borrow_mut() = Some(Box::new(move || {
        clear_box(&rows_holder_for_render);

        let snapshot = columns_for_render.borrow().clone();
        for (index, column) in snapshot.iter().enumerate() {
            let row = GtkBox::new(Orientation::Horizontal, 6);

            let name = Label::new(Some(label_for(&column.id)));
            name.set_xalign(0.0);
            name.set_hexpand(true);

            let show = CheckButton::with_label("Show");
            show.set_active(column.visible);

            let width_lbl = Label::new(Some("Width"));
            let width = SpinButton::with_range(6.0, 80.0, 1.0);
            width.set_value(column.width_chars as f64);

            let up = Button::with_label("Up");
            let down = Button::with_label("Down");

            row.append(&name);
            row.append(&show);
            row.append(&width_lbl);
            row.append(&width);
            row.append(&up);
            row.append(&down);

            let columns_for_show = columns_for_render.clone();
            show.connect_toggled(move |cb| {
                if let Some(item) = columns_for_show.borrow_mut().get_mut(index) {
                    item.visible = cb.is_active();
                }
            });

            let columns_for_width = columns_for_render.clone();
            width.connect_value_changed(move |spin| {
                if let Some(item) = columns_for_width.borrow_mut().get_mut(index) {
                    item.width_chars = spin.value() as i32;
                }
            });

            let columns_for_up = columns_for_render.clone();
            let renderer_for_up = renderer_for_rows.clone();
            up.connect_clicked(move |_| {
                let mut cols = columns_for_up.borrow_mut();
                if index > 0 && index < cols.len() {
                    cols.swap(index, index - 1);
                }
                drop(cols);
                if let Some(render) = renderer_for_up.borrow().as_ref() {
                    render();
                }
            });

            let columns_for_down = columns_for_render.clone();
            let renderer_for_down = renderer_for_rows.clone();
            down.connect_clicked(move |_| {
                let mut cols = columns_for_down.borrow_mut();
                if index + 1 < cols.len() {
                    cols.swap(index, index + 1);
                }
                drop(cols);
                if let Some(render) = renderer_for_down.borrow().as_ref() {
                    render();
                }
            });

            rows_holder_for_render.append(&row);
        }
    }));

    if let Some(render) = renderer.borrow().as_ref() {
        render();
    }

    section
}

fn build_watchlist_editor(entries: Rc<RefCell<Vec<WatchlistEntry>>>) -> GtkBox {
    let section = GtkBox::new(Orientation::Vertical, 8);

    let help = Label::new(Some(
        "Each entry can match by MAC, name, or both. Matching rows in the main Access Points, Clients, or Bluetooth tables are highlighted using the selected color.",
    ));
    help.set_xalign(0.0);
    help.set_wrap(true);
    section.append(&help);

    let rows_holder = GtkBox::new(Orientation::Vertical, 8);
    section.append(&rows_holder);

    let add_button = Button::with_label("Add Watchlist Entry");
    section.append(&add_button);

    type RenderFn = Box<dyn Fn()>;
    let renderer: Rc<RefCell<Option<RenderFn>>> = Rc::new(RefCell::new(None));
    let rows_holder_for_render = rows_holder.clone();
    let entries_for_render = entries.clone();
    let renderer_for_rows = renderer.clone();

    *renderer.borrow_mut() = Some(Box::new(move || {
        clear_box(&rows_holder_for_render);

        let snapshot = entries_for_render.borrow().clone();
        for (index, entry) in snapshot.iter().enumerate() {
            let row = GtkBox::new(Orientation::Vertical, 8);

            let header = GtkBox::new(Orientation::Horizontal, 8);
            let title = Label::new(Some(&format!("Entry {}", index + 1)));
            title.add_css_class("heading");
            title.set_xalign(0.0);
            title.set_hexpand(true);
            let remove_button = Button::with_label("Remove");
            header.append(&title);
            header.append(&remove_button);
            row.append(&header);

            let grid = Grid::new();
            grid.set_column_spacing(8);
            grid.set_row_spacing(8);
            row.append(&grid);

            let alert_name_entry = Entry::new();
            alert_name_entry.set_text(&entry.label);
            alert_name_entry.set_placeholder_text(Some("Alert / display name"));

            let device_type_combo = ComboBoxText::new();
            device_type_combo.append(Some("wifi"), "Wi-Fi");
            device_type_combo.append(Some("bluetooth"), "Bluetooth");
            device_type_combo.set_active_id(Some(match entry.device_type {
                WatchlistDeviceType::Wifi => "wifi",
                WatchlistDeviceType::Bluetooth => "bluetooth",
            }));

            let mac_entry = Entry::new();
            mac_entry.set_text(&entry.mac);
            mac_entry.set_placeholder_text(Some("AA:BB:CC:DD:EE:FF"));

            let name_entry = Entry::new();
            name_entry.set_text(&entry.name);
            name_entry.set_placeholder_text(Some("SSID, client name, or Bluetooth name"));

            let color_entry = Entry::new();
            color_entry.set_text(&normalize_watchlist_color(&entry.color_hex));
            color_entry.set_placeholder_text(Some("#2ecc71"));

            for (row_index, (label_text, widget)) in [
                ("Alert Name", alert_name_entry.upcast_ref::<gtk::Widget>()),
                ("Device Type", device_type_combo.upcast_ref::<gtk::Widget>()),
                ("MAC", mac_entry.upcast_ref::<gtk::Widget>()),
                ("Name", name_entry.upcast_ref::<gtk::Widget>()),
                ("Color", color_entry.upcast_ref::<gtk::Widget>()),
            ]
            .into_iter()
            .enumerate()
            {
                let label = Label::new(Some(label_text));
                label.set_xalign(0.0);
                label.set_width_chars(14);
                grid.attach(&label, 0, row_index as i32, 1, 1);
                grid.attach(widget, 1, row_index as i32, 1, 1);
            }

            let hint = Label::new(Some(
                "Leave MAC or Name blank if you want the other field to be the only match key.",
            ));
            hint.set_xalign(0.0);
            hint.set_wrap(true);
            row.append(&hint);

            {
                let entries = entries_for_render.clone();
                alert_name_entry.connect_changed(move |entry_widget| {
                    if let Some(item) = entries.borrow_mut().get_mut(index) {
                        item.label = entry_widget.text().to_string();
                    }
                });
            }

            {
                let entries = entries_for_render.clone();
                device_type_combo.connect_changed(move |combo| {
                    if let Some(item) = entries.borrow_mut().get_mut(index) {
                        item.device_type = match combo.active_id().as_deref() {
                            Some("bluetooth") => WatchlistDeviceType::Bluetooth,
                            _ => WatchlistDeviceType::Wifi,
                        };
                    }
                });
            }

            {
                let entries = entries_for_render.clone();
                mac_entry.connect_changed(move |entry_widget| {
                    if let Some(item) = entries.borrow_mut().get_mut(index) {
                        item.mac = entry_widget.text().to_string();
                    }
                });
            }

            {
                let entries = entries_for_render.clone();
                name_entry.connect_changed(move |entry_widget| {
                    if let Some(item) = entries.borrow_mut().get_mut(index) {
                        item.name = entry_widget.text().to_string();
                    }
                });
            }

            {
                let entries = entries_for_render.clone();
                color_entry.connect_changed(move |entry_widget| {
                    if let Some(item) = entries.borrow_mut().get_mut(index) {
                        item.color_hex = entry_widget.text().to_string();
                    }
                });
            }

            {
                let entries = entries_for_render.clone();
                let renderer = renderer_for_rows.clone();
                remove_button.connect_clicked(move |_| {
                    let mut values = entries.borrow_mut();
                    if index < values.len() {
                        values.remove(index);
                    }
                    drop(values);
                    if let Some(render) = renderer.borrow().as_ref() {
                        render();
                    }
                });
            }

            rows_holder_for_render.append(&row);
        }
    }));

    {
        let entries = entries.clone();
        let renderer = renderer.clone();
        add_button.connect_clicked(move |_| {
            entries.borrow_mut().push(WatchlistEntry {
                label: String::new(),
                device_type: WatchlistDeviceType::Wifi,
                mac: String::new(),
                name: String::new(),
                color_hex: crate::settings::default_watchlist_color_hex(),
            });
            if let Some(render) = renderer.borrow().as_ref() {
                render();
            }
        });
    }

    if let Some(render) = renderer.borrow().as_ref() {
        render();
    }

    section
}

fn sanitize_table_layout(layout: &mut TableLayout, defaults: &TableLayout) {
    let known_ids = defaults
        .columns
        .iter()
        .map(|c| c.id.as_str())
        .collect::<HashSet<_>>();
    layout
        .columns
        .retain(|column| known_ids.contains(column.id.as_str()));

    if layout.columns.is_empty() {
        layout.columns = defaults.columns.clone();
    }

    for default in &defaults.columns {
        if !layout.columns.iter().any(|column| column.id == default.id) {
            layout.columns.push(default.clone());
        }
    }

    for column in &mut layout.columns {
        column.width_chars = column.width_chars.clamp(6, 80);
    }

    if !layout.columns.iter().any(|column| column.visible) {
        if let Some(first) = layout.columns.first_mut() {
            first.visible = true;
        }
    }
}

#[derive(Debug, Clone)]
struct WifiInterfaceCapability {
    interface_name: String,
    if_type: String,
    monitor_capable: bool,
    channels: Vec<capture::SupportedChannel>,
    ht_modes: Vec<String>,
}

fn detect_wifi_interface_capabilities() -> Vec<WifiInterfaceCapability> {
    let interfaces = capture::list_interfaces().unwrap_or_default();
    let mut monitor_capable = Vec::new();
    let mut fallback = Vec::new();

    for iface in interfaces {
        if iface.name == "lo" {
            continue;
        }

        let supports_monitor = capture::interface_supports_monitor_mode(&iface.name)
            .unwrap_or_else(|_| iface.if_type == "monitor");
        let ht_modes = capture::list_supported_ht_modes(&iface.name)
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec!["HT20".to_string(), "HT40+".to_string(), "HT40-".to_string()]);
        let channels = capture::list_supported_channel_details(&iface.name)
            .ok()
            .filter(|v| !v.is_empty())
            .map(|channels| filter_usable_capability_channels(&channels, &ht_modes))
            .unwrap_or_else(|| {
                [1_u16, 6, 11, 36, 40, 44, 48]
                    .into_iter()
                    .map(|ch| capture::SupportedChannel {
                        channel: ch,
                        frequency_mhz: None,
                    })
                    .collect()
            });

        let cap = WifiInterfaceCapability {
            interface_name: iface.name,
            if_type: iface.if_type,
            monitor_capable: supports_monitor,
            channels,
            ht_modes,
        };

        if supports_monitor {
            monitor_capable.push(cap);
        } else {
            fallback.push(cap);
        }
    }

    monitor_capable.extend(fallback);
    monitor_capable
}

fn fallback_band_from_channel_number(channel: u16) -> SpectrumBand {
    match channel {
        1..=14 => SpectrumBand::Ghz2_4,
        15..=177 => SpectrumBand::Ghz5,
        _ => SpectrumBand::Ghz6,
    }
}

fn channel_capability_band(channel: &capture::SupportedChannel) -> SpectrumBand {
    let by_freq = SpectrumBand::from_frequency_mhz(channel.frequency_mhz);
    if by_freq == SpectrumBand::Unknown {
        fallback_band_from_channel_number(channel.channel)
    } else {
        by_freq
    }
}

fn channel_capability_band_label(channel: &capture::SupportedChannel) -> String {
    channel_capability_band(channel).label().to_string()
}

fn channel_is_usable_for_current_ui(
    channel: &capture::SupportedChannel,
    ht_modes: &[String],
) -> bool {
    let band = channel_capability_band(channel);
    if band == SpectrumBand::Unknown {
        return false;
    }

    // Channel 14 (2484 MHz) is not valid with the HT20 hopper path used by scan mode.
    if channel.channel == 14 || channel.frequency_mhz == Some(2484) {
        return ht_modes.iter().any(|mode| mode == "NOHT");
    }

    true
}

fn filter_usable_capability_channels(
    channels: &[capture::SupportedChannel],
    ht_modes: &[String],
) -> Vec<capture::SupportedChannel> {
    let mut filtered = channels
        .iter()
        .filter(|channel| channel_is_usable_for_current_ui(channel, ht_modes))
        .cloned()
        .collect::<Vec<_>>();
    filtered.sort_by_key(|c| (c.frequency_mhz.unwrap_or(0), c.channel));
    filtered.dedup_by(|a, b| a.channel == b.channel && a.frequency_mhz == b.frequency_mhz);
    filtered
}

fn available_band_options(channels: &[capture::SupportedChannel]) -> Vec<(String, String)> {
    let mut options = Vec::new();
    for (id, label, band) in [
        ("2.4", "2.4 GHz", SpectrumBand::Ghz2_4),
        ("5", "5 GHz", SpectrumBand::Ghz5),
        ("6", "6 GHz", SpectrumBand::Ghz6),
    ] {
        if channels
            .iter()
            .any(|channel| channel_capability_band(channel) == band)
        {
            options.push((id.to_string(), label.to_string()));
        }
    }
    options
}

fn filter_channels_for_band(
    channels: &[capture::SupportedChannel],
    band: &SpectrumBand,
) -> Vec<u16> {
    let mut out = channels
        .iter()
        .filter(|ch| band == &SpectrumBand::Unknown || channel_capability_band(ch) == *band)
        .map(|ch| ch.channel)
        .collect::<Vec<_>>();
    out.sort_unstable();
    out.dedup();
    out
}

fn lock_ht_mode_choices_from_capability(ht_modes: &[String]) -> Vec<String> {
    let mut out = ht_modes
        .iter()
        .filter(|m| !m.contains("capability"))
        .cloned()
        .collect::<Vec<_>>();
    if !out.iter().any(|m| m == "HT20") {
        out.push("HT20".to_string());
    }
    out.sort();
    out.dedup();
    out
}

fn open_interface_channel_capabilities_dialog(
    window: &ApplicationWindow,
    iface_name: &str,
    channels: &[capture::SupportedChannel],
    ht_modes: &[String],
) {
    let dialog = Dialog::builder()
        .transient_for(window)
        .modal(true)
        .title(format!("{} Channel Capabilities", iface_name))
        .default_width(700)
        .default_height(420)
        .build();
    dialog.add_button("Close", ResponseType::Close);

    let area = dialog.content_area();
    let wrapper = GtkBox::new(Orientation::Vertical, 6);

    let summary = Label::new(Some(&format!(
        "Device bandwidth modes: {}",
        ht_modes.join(", ")
    )));
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    wrapper.append(&summary);

    let rows = GtkBox::new(Orientation::Vertical, 4);
    let header = GtkBox::new(Orientation::Horizontal, 10);
    header.append(&label_cell("Channel".to_string(), 10));
    header.append(&label_cell("Freq MHz".to_string(), 10));
    header.append(&label_cell("Band".to_string(), 10));
    header.append(&label_cell("Bandwidth / Modes".to_string(), 40));
    rows.append(&header);

    if channels.is_empty() {
        let empty = Label::new(Some(
            "No channel capability data available for this device.",
        ));
        empty.set_xalign(0.0);
        rows.append(&empty);
    } else {
        let widths = ht_modes.join(", ");
        for ch in channels {
            let row = GtkBox::new(Orientation::Horizontal, 10);
            row.append(&label_cell(ch.channel.to_string(), 10));
            row.append(&label_cell(
                ch.frequency_mhz.map(|f| f.to_string()).unwrap_or_default(),
                10,
            ));
            row.append(&label_cell(channel_capability_band_label(ch), 10));
            row.append(&label_cell(widths.clone(), 40));
            rows.append(&row);
        }
    }

    let scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&rows)
        .build();
    wrapper.append(&scrolled);
    area.append(&wrapper);

    dialog.connect_response(|d, _| d.close());
    dialog.present();
}

fn open_interface_settings_dialog(window: &ApplicationWindow, state: Rc<RefCell<AppState>>) {
    open_interface_settings_dialog_inner(window, state, false, None, None);
}

fn open_interface_settings_dialog_for_start(
    window: &ApplicationWindow,
    state: Rc<RefCell<AppState>>,
    start_btn: Button,
    stop_btn: Button,
) {
    open_interface_settings_dialog_inner(window, state, true, Some(start_btn), Some(stop_btn));
}

fn apply_interface_selection(
    state: Rc<RefCell<AppState>>,
    iface_name: String,
    mode: ChannelSelectionMode,
    wifi_packet_header_mode: WifiPacketHeaderMode,
    bluetooth_enabled: bool,
    bluetooth_scan_source: BluetoothScanSource,
    bluetooth_controller: Option<String>,
    ubertooth_device: Option<String>,
    output_to_files: bool,
    start_after_apply: bool,
    selected_output_root: Option<PathBuf>,
    start_btn: Option<Button>,
    stop_btn: Option<Button>,
) {
    let mut s = state.borrow_mut();
    s.settings.interfaces = vec![InterfaceSettings {
        interface_name: iface_name,
        monitor_interface_name: None,
        channel_mode: mode,
        enabled: true,
    }];
    s.settings.bluetooth_enabled = bluetooth_enabled;
    s.settings.bluetooth_scan_source = bluetooth_scan_source;
    s.settings.bluetooth_controller = bluetooth_controller;
    s.settings.ubertooth_device = ubertooth_device;
    s.settings.wifi_packet_header_mode = wifi_packet_header_mode;
    s.settings.output_to_files = output_to_files;

    if output_to_files {
        let output_root = selected_output_root.unwrap_or_else(|| s.settings.output_root.clone());
        if let Err(err) = s.reset_output_session(output_root, true, true) {
            s.push_status(format!("failed to initialize output session: {err}"));
            return;
        }
    } else if let Err(err) = s.switch_to_internal_output_session() {
        s.push_status(format!(
            "failed to initialize internal runtime session: {err}"
        ));
        return;
    }

    if start_after_apply {
        s.start_scanning();
    } else if s.capture_runtime.is_some() || s.bluetooth_runtime.is_some() {
        let _ = s.begin_async_scan_shutdown(Some(
            "interface settings applied; restarting capture".to_string(),
        ));
    } else {
        s.push_status("interface settings applied".to_string());
    }
    s.save_settings_to_disk();

    if let (Some(start_btn), Some(stop_btn)) = (&start_btn, &stop_btn) {
        set_scan_control_button_sensitivity(
            start_btn,
            stop_btn,
            s.capture_runtime.is_some(),
            s.bluetooth_runtime.is_some(),
            s.scan_start_in_progress || s.scan_stop_in_progress,
        );
    }
}

fn open_interface_settings_dialog_inner(
    window: &ApplicationWindow,
    state: Rc<RefCell<AppState>>,
    start_after_apply: bool,
    start_btn: Option<Button>,
    stop_btn: Option<Button>,
) {
    let settings_window = GtkWindow::builder()
        .transient_for(window)
        .modal(true)
        .title(if start_after_apply {
            "Select Interface and Start Scanning"
        } else {
            "Interface Settings"
        })
        .default_width(760)
        .default_height(520)
        .build();

    let root = GtkBox::new(Orientation::Vertical, 8);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);
    settings_window.set_child(Some(&root));

    let capabilities = detect_wifi_interface_capabilities();
    let capabilities_rc = Rc::new(RefCell::new(capabilities));

    let interface_combo = ComboBoxText::new();
    let interface_status = Label::new(None);
    interface_status.set_xalign(0.0);
    interface_status.set_wrap(true);

    {
        let caps = capabilities_rc.borrow();
        for cap in caps.iter() {
            interface_combo.append(
                Some(&cap.interface_name),
                &format!(
                    "{} ({}){}",
                    cap.interface_name,
                    cap.if_type,
                    if cap.monitor_capable {
                        " [monitor-capable]"
                    } else {
                        " [monitor unsupported]"
                    }
                ),
            );
        }
        if caps.is_empty() {
            interface_combo.append(Some("wlan0"), "wlan0 (manual fallback)");
        }
    }

    let mode_combo = ComboBoxText::new();
    mode_combo.append(Some("hop_specific"), "Hop Specific Channels");
    mode_combo.append(Some("hop_band"), "Hop One Band");
    mode_combo.append(Some("locked"), "Lock Channel");
    mode_combo.set_active_id(Some("hop_specific"));

    let packet_header_combo = ComboBoxText::new();
    packet_header_combo.append(Some("radiotap"), "Radiotap");
    packet_header_combo.append(Some("ppi"), "PPI");
    packet_header_combo.set_active_id(Some("radiotap"));

    let dwell_entry = Entry::new();
    dwell_entry.set_placeholder_text(Some("Dwell ms (200 = 5 ch/sec)"));
    dwell_entry.set_text("200");

    let channels_entry = Entry::new();
    channels_entry.set_placeholder_text(Some("1,6,11,36,40,44,48"));

    let show_channels_btn = Button::with_label("Show Device Channels");

    let band_combo = ComboBoxText::new();
    band_combo.append(Some("5"), "5 GHz");
    band_combo.set_active_id(Some("5"));

    let lock_channel_entry = Entry::new();
    lock_channel_entry.set_placeholder_text(Some("e.g. 36"));

    let lock_ht_combo = ComboBoxText::new();
    lock_ht_combo.append(Some("HT20"), "HT20");
    lock_ht_combo.append(Some("HT40+"), "HT40+");
    lock_ht_combo.append(Some("HT40-"), "HT40-");
    lock_ht_combo.set_active_id(Some("HT20"));

    let bluetooth_scan_check = CheckButton::with_label("Scan Bluetooth");
    let bluetooth_source_combo = ComboBoxText::new();
    bluetooth_source_combo.append(Some("bluez"), "BlueZ");
    bluetooth_source_combo.append(Some("ubertooth"), "Ubertooth");
    bluetooth_source_combo.append(Some("both"), "BlueZ + Ubertooth");
    bluetooth_source_combo.set_active_id(Some("bluez"));

    let bluetooth_controller_combo = ComboBoxText::new();
    bluetooth_controller_combo.append(Some("default"), "Default Controller");
    for ctrl in bluetooth::list_controllers().unwrap_or_default() {
        bluetooth_controller_combo.append(
            Some(&ctrl.id),
            &format!(
                "{} ({}){}",
                ctrl.id,
                if ctrl.name.is_empty() {
                    "unnamed"
                } else {
                    ctrl.name.as_str()
                },
                if ctrl.is_default { " [default]" } else { "" }
            ),
        );
    }
    bluetooth_controller_combo.set_active_id(Some("default"));

    let ubertooth_combo = ComboBoxText::new();
    ubertooth_combo.append(Some("default"), "Default Ubertooth Device");
    for device in bluetooth::list_ubertooth_devices().unwrap_or_default() {
        ubertooth_combo.append(Some(&device.id), &device.name);
    }
    ubertooth_combo.set_active_id(Some("default"));

    let output_to_files_check = CheckButton::with_label("Output to Files");
    let output_dir_entry = Entry::new();
    output_dir_entry.set_hexpand(true);
    output_dir_entry.set_placeholder_text(Some("/path/to/output"));
    let browse_output_btn = Button::with_label("Browse");

    let iface_row = GtkBox::new(Orientation::Horizontal, 8);
    let iface_label = Label::new(Some("Wi-Fi Interface"));
    iface_label.set_xalign(0.0);
    iface_label.set_width_chars(18);
    iface_row.append(&iface_label);
    iface_row.append(&interface_combo);

    let mode_row = GtkBox::new(Orientation::Horizontal, 8);
    let mode_label = Label::new(Some("Channel Mode"));
    mode_label.set_xalign(0.0);
    mode_label.set_width_chars(18);
    mode_row.append(&mode_label);
    mode_row.append(&mode_combo);

    let packet_header_row = GtkBox::new(Orientation::Horizontal, 8);
    let packet_header_label = Label::new(Some("Packet Headers"));
    packet_header_label.set_xalign(0.0);
    packet_header_label.set_width_chars(18);
    packet_header_row.append(&packet_header_label);
    packet_header_row.append(&packet_header_combo);

    let channels_row = GtkBox::new(Orientation::Horizontal, 8);
    let channels_label = Label::new(Some("Specific Channels"));
    channels_label.set_xalign(0.0);
    channels_label.set_width_chars(18);
    channels_row.append(&channels_label);
    channels_row.append(&channels_entry);
    channels_row.append(&show_channels_btn);

    let dwell_row = GtkBox::new(Orientation::Horizontal, 8);
    let dwell_label = Label::new(Some("Dwell (ms)"));
    dwell_label.set_xalign(0.0);
    dwell_label.set_width_chars(18);
    dwell_row.append(&dwell_label);
    dwell_row.append(&dwell_entry);

    let band_row = GtkBox::new(Orientation::Horizontal, 8);
    let band_label = Label::new(Some("Band"));
    band_label.set_xalign(0.0);
    band_label.set_width_chars(18);
    band_row.append(&band_label);
    band_row.append(&band_combo);

    let lock_row = GtkBox::new(Orientation::Horizontal, 8);
    let lock_label = Label::new(Some("Locked Channel"));
    lock_label.set_xalign(0.0);
    lock_label.set_width_chars(18);
    lock_row.append(&lock_label);
    lock_row.append(&lock_channel_entry);

    let ht_row = GtkBox::new(Orientation::Horizontal, 8);
    let ht_label = Label::new(Some("HT Mode"));
    ht_label.set_xalign(0.0);
    ht_label.set_width_chars(18);
    ht_row.append(&ht_label);
    ht_row.append(&lock_ht_combo);

    let bluetooth_row = GtkBox::new(Orientation::Horizontal, 8);
    let bluetooth_label = Label::new(Some("Bluetooth"));
    bluetooth_label.set_xalign(0.0);
    bluetooth_label.set_width_chars(18);
    bluetooth_row.append(&bluetooth_label);
    bluetooth_row.append(&bluetooth_scan_check);

    let bluetooth_source_row = GtkBox::new(Orientation::Horizontal, 8);
    let bluetooth_source_label = Label::new(Some("Bluetooth Source"));
    bluetooth_source_label.set_xalign(0.0);
    bluetooth_source_label.set_width_chars(18);
    bluetooth_source_row.append(&bluetooth_source_label);
    bluetooth_source_row.append(&bluetooth_source_combo);

    let bluetooth_controller_row = GtkBox::new(Orientation::Horizontal, 8);
    let bluetooth_controller_label = Label::new(Some("Bluetooth Radio"));
    bluetooth_controller_label.set_xalign(0.0);
    bluetooth_controller_label.set_width_chars(18);
    bluetooth_controller_row.append(&bluetooth_controller_label);
    bluetooth_controller_row.append(&bluetooth_controller_combo);

    let ubertooth_row = GtkBox::new(Orientation::Horizontal, 8);
    let ubertooth_label = Label::new(Some("Ubertooth Device"));
    ubertooth_label.set_xalign(0.0);
    ubertooth_label.set_width_chars(18);
    ubertooth_row.append(&ubertooth_label);
    ubertooth_row.append(&ubertooth_combo);

    let output_toggle_row = GtkBox::new(Orientation::Horizontal, 8);
    let output_toggle_label = Label::new(Some("Output"));
    output_toggle_label.set_xalign(0.0);
    output_toggle_label.set_width_chars(18);
    output_toggle_row.append(&output_toggle_label);
    output_toggle_row.append(&output_to_files_check);

    let output_dir_row = GtkBox::new(Orientation::Horizontal, 8);
    let output_dir_label = Label::new(Some("Output Directory"));
    output_dir_label.set_xalign(0.0);
    output_dir_label.set_width_chars(18);
    output_dir_row.append(&output_dir_label);
    output_dir_row.append(&output_dir_entry);
    output_dir_row.append(&browse_output_btn);

    let action_row = GtkBox::new(Orientation::Horizontal, 8);
    action_row.set_halign(gtk::Align::End);
    let cancel_btn = Button::with_label("Cancel");
    let apply_btn = Button::with_label(if start_after_apply { "Start" } else { "Apply" });
    action_row.append(&cancel_btn);
    action_row.append(&apply_btn);

    root.append(&iface_row);
    root.append(&interface_status);
    root.append(&mode_row);
    root.append(&packet_header_row);
    root.append(&channels_row);
    root.append(&dwell_row);
    root.append(&band_row);
    root.append(&lock_row);
    root.append(&ht_row);
    root.append(&bluetooth_row);
    root.append(&bluetooth_source_row);
    root.append(&bluetooth_controller_row);
    root.append(&ubertooth_row);
    root.append(&output_toggle_row);
    root.append(&output_dir_row);
    root.append(&action_row);

    let apply_interface_capability = Rc::new(RefCell::new(None::<Box<dyn Fn()>>));
    {
        let caps = capabilities_rc.clone();
        let interface_combo = interface_combo.clone();
        let channels_entry = channels_entry.clone();
        let interface_status = interface_status.clone();
        let lock_ht_combo = lock_ht_combo.clone();
        let band_combo = band_combo.clone();
        let apply = apply_interface_capability.clone();
        *apply.borrow_mut() = Some(Box::new(move || {
            let selected = interface_combo
                .active_id()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "wlan0".to_string());
            let cap = caps
                .borrow()
                .iter()
                .find(|c| c.interface_name == selected)
                .cloned();

            if let Some(cap) = cap {
                if channels_entry.text().trim().is_empty() {
                    let default_channels = cap
                        .channels
                        .iter()
                        .map(|c| c.channel.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    channels_entry.set_text(&default_channels);
                }

                interface_status.set_text(&format!(
                    "Selected {} | monitor mode: {} | {} channels discovered | modes: {}",
                    cap.interface_name,
                    if cap.monitor_capable { "yes" } else { "no" },
                    cap.channels.len(),
                    cap.ht_modes.join(", ")
                ));

                let current_band = band_combo.active_id().map(|v| v.to_string());
                band_combo.remove_all();
                let available_bands = available_band_options(&cap.channels);
                if available_bands.is_empty() {
                    band_combo.append(Some("5"), "5 GHz");
                    band_combo.set_active_id(Some("5"));
                } else {
                    for (id, label) in &available_bands {
                        band_combo.append(Some(id), label);
                    }
                    if let Some(current_band) = current_band.as_deref() {
                        if !band_combo.set_active_id(Some(current_band)) {
                            if let Some((first_id, _)) = available_bands.first() {
                                band_combo.set_active_id(Some(first_id));
                            }
                        }
                    } else if let Some((first_id, _)) = available_bands.first() {
                        band_combo.set_active_id(Some(first_id));
                    }
                }

                let current_ht = lock_ht_combo
                    .active_id()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "HT20".to_string());
                lock_ht_combo.remove_all();
                let ht_choices = lock_ht_mode_choices_from_capability(&cap.ht_modes);
                for mode in &ht_choices {
                    lock_ht_combo.append(Some(mode), mode);
                }
                if ht_choices.iter().any(|m| m == &current_ht) {
                    lock_ht_combo.set_active_id(Some(&current_ht));
                } else {
                    lock_ht_combo.set_active_id(Some("HT20"));
                }
            } else {
                interface_status.set_text("No interface capability data available.");
            }
        }));
    }

    let update_mode_visibility = Rc::new(RefCell::new(None::<Box<dyn Fn()>>));
    {
        let mode_combo = mode_combo.clone();
        let channels_row = channels_row.clone();
        let dwell_row = dwell_row.clone();
        let band_row = band_row.clone();
        let lock_row = lock_row.clone();
        let ht_row = ht_row.clone();
        let update_mode = update_mode_visibility.clone();
        *update_mode.borrow_mut() = Some(Box::new(move || {
            let mode = mode_combo
                .active_id()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "hop_specific".to_string());
            match mode.as_str() {
                "hop_band" => {
                    channels_row.set_visible(false);
                    dwell_row.set_visible(false);
                    band_row.set_visible(true);
                    lock_row.set_visible(false);
                    ht_row.set_visible(false);
                }
                "locked" => {
                    channels_row.set_visible(false);
                    dwell_row.set_visible(false);
                    band_row.set_visible(false);
                    lock_row.set_visible(true);
                    ht_row.set_visible(true);
                }
                _ => {
                    channels_row.set_visible(true);
                    dwell_row.set_visible(true);
                    band_row.set_visible(false);
                    lock_row.set_visible(false);
                    ht_row.set_visible(false);
                }
            }
        }));
    }

    {
        let caps = capabilities_rc.clone();
        let interface_combo = interface_combo.clone();
        let window = window.clone();
        let open_cap_dialog = move || {
            let selected = interface_combo
                .active_id()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "wlan0".to_string());
            if let Some(cap) = caps
                .borrow()
                .iter()
                .find(|c| c.interface_name == selected)
                .cloned()
            {
                open_interface_channel_capabilities_dialog(
                    &window,
                    &cap.interface_name,
                    &cap.channels,
                    &cap.ht_modes,
                );
            } else {
                open_interface_channel_capabilities_dialog(
                    &window,
                    &selected,
                    &[],
                    &["HT20".into()],
                );
            }
        };

        let click_handler = Rc::new(open_cap_dialog);

        {
            let click_handler = click_handler.clone();
            show_channels_btn.connect_clicked(move |_| {
                (click_handler)();
            });
        }
        {
            let click_handler = click_handler.clone();
            let click = GestureClick::new();
            click.set_button(1);
            click.connect_pressed(move |_, _, _, _| {
                (click_handler)();
            });
            channels_entry.add_controller(click);
        }
    }

    {
        let apply_interface_capability = apply_interface_capability.clone();
        interface_combo.connect_changed(move |_| {
            if let Some(cb) = apply_interface_capability.borrow().as_ref() {
                cb();
            }
        });
    }

    {
        let update_mode_visibility = update_mode_visibility.clone();
        mode_combo.connect_changed(move |_| {
            if let Some(cb) = update_mode_visibility.borrow().as_ref() {
                cb();
            }
        });
    }

    let update_bluetooth_control_visibility = Rc::new(RefCell::new(None::<Box<dyn Fn()>>));
    {
        let bluetooth_scan_check = bluetooth_scan_check.clone();
        let bluetooth_source_combo = bluetooth_source_combo.clone();
        let bluetooth_controller_combo = bluetooth_controller_combo.clone();
        let ubertooth_combo = ubertooth_combo.clone();
        let update = update_bluetooth_control_visibility.clone();
        *update.borrow_mut() = Some(Box::new(move || {
            let enabled = bluetooth_scan_check.is_active();
            let source = bluetooth_source_combo
                .active_id()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "bluez".to_string());
            let needs_bluez = matches!(source.as_str(), "bluez" | "both");
            let needs_ubertooth = matches!(source.as_str(), "ubertooth" | "both");
            bluetooth_source_combo.set_sensitive(enabled);
            bluetooth_controller_combo.set_sensitive(enabled && needs_bluez);
            ubertooth_combo.set_sensitive(enabled && needs_ubertooth);
        }));
    }

    {
        let update = update_bluetooth_control_visibility.clone();
        bluetooth_scan_check.connect_toggled(move |_| {
            if let Some(cb) = update.borrow().as_ref() {
                cb();
            }
        });
    }

    {
        let update = update_bluetooth_control_visibility.clone();
        bluetooth_source_combo.connect_changed(move |_| {
            if let Some(cb) = update.borrow().as_ref() {
                cb();
            }
        });
    }

    {
        let output_dir_entry = output_dir_entry.clone();
        let browse_output_btn = browse_output_btn.clone();
        output_to_files_check.connect_toggled(move |check| {
            let enabled = check.is_active();
            output_dir_entry.set_sensitive(enabled);
            browse_output_btn.set_sensitive(enabled);
        });
    }

    let current_interface = {
        let s = state.borrow();
        (
            s.settings.interfaces.first().cloned(),
            s.settings.wifi_packet_header_mode,
            s.settings.bluetooth_enabled,
            s.settings.bluetooth_scan_source,
            s.settings.bluetooth_controller.clone(),
            s.settings.ubertooth_device.clone(),
            s.settings.output_to_files,
            s.settings.output_root.clone(),
        )
    };

    if let Some(iface) = &current_interface.0 {
        if !iface.interface_name.is_empty() {
            interface_combo.set_active_id(Some(&iface.interface_name));
        }
        match &iface.channel_mode {
            ChannelSelectionMode::HopAll { channels, dwell_ms } => {
                mode_combo.set_active_id(Some("hop_specific"));
                channels_entry.set_text(
                    &channels
                        .iter()
                        .map(|c| c.to_string())
                        .collect::<Vec<_>>()
                        .join(","),
                );
                dwell_entry.set_text(&dwell_ms.to_string());
            }
            ChannelSelectionMode::HopBand {
                band,
                channels: _,
                dwell_ms,
            } => {
                mode_combo.set_active_id(Some("hop_band"));
                dwell_entry.set_text(&dwell_ms.to_string());
                band_combo.set_active_id(Some(match band {
                    SpectrumBand::Ghz2_4 => "2.4",
                    SpectrumBand::Ghz6 => "6",
                    _ => "5",
                }));
            }
            ChannelSelectionMode::Locked { channel, ht_mode } => {
                mode_combo.set_active_id(Some("locked"));
                lock_channel_entry.set_text(&channel.to_string());
                lock_ht_combo.set_active_id(Some(ht_mode));
            }
        }
    } else {
        interface_combo.set_active(Some(0));
    }

    packet_header_combo.set_active_id(Some(match current_interface.1 {
        WifiPacketHeaderMode::Radiotap => "radiotap",
        WifiPacketHeaderMode::Ppi => "ppi",
    }));

    bluetooth_scan_check.set_active(current_interface.2);
    bluetooth_source_combo.set_active_id(Some(match current_interface.3 {
        BluetoothScanSource::Bluez => "bluez",
        BluetoothScanSource::Ubertooth => "ubertooth",
        BluetoothScanSource::Both => "both",
    }));
    match current_interface.4.as_deref() {
        Some(ctrl) => {
            if !bluetooth_controller_combo.set_active_id(Some(ctrl)) {
                bluetooth_controller_combo.set_active_id(Some("default"));
            }
        }
        None => {
            bluetooth_controller_combo.set_active_id(Some("default"));
        }
    }
    match current_interface.5.as_deref() {
        Some(device) => {
            if !ubertooth_combo.set_active_id(Some(device)) {
                ubertooth_combo.set_active_id(Some("default"));
            }
        }
        None => {
            ubertooth_combo.set_active_id(Some("default"));
        }
    }
    output_to_files_check.set_active(current_interface.6);
    output_dir_entry.set_text(&current_interface.7.display().to_string());
    output_dir_entry.set_sensitive(current_interface.6);
    browse_output_btn.set_sensitive(current_interface.6);

    if let Some(cb) = apply_interface_capability.borrow().as_ref() {
        cb();
    }
    if let Some(cb) = update_mode_visibility.borrow().as_ref() {
        cb();
    }
    if let Some(cb) = update_bluetooth_control_visibility.borrow().as_ref() {
        cb();
    }

    {
        let settings_window = settings_window.clone();
        cancel_btn.connect_clicked(move |_| {
            settings_window.close();
        });
    }

    {
        let settings_window = settings_window.clone();
        let output_dir_entry = output_dir_entry.clone();
        browse_output_btn.connect_clicked(move |_| {
            let initial_path = {
                let value = output_dir_entry.text().to_string();
                if value.trim().is_empty() {
                    PathBuf::from(".")
                } else {
                    PathBuf::from(value)
                }
            };
            let output_dir_entry = output_dir_entry.clone();
            choose_output_dir(
                &settings_window,
                initial_path,
                move |selected_output_root| {
                    if let Some(path) = selected_output_root {
                        output_dir_entry.set_text(&path.display().to_string());
                    }
                },
            );
        });
    }

    {
        let state = state.clone();
        let capabilities_rc = capabilities_rc.clone();
        let interface_combo = interface_combo.clone();
        let mode_combo = mode_combo.clone();
        let packet_header_combo = packet_header_combo.clone();
        let channels_entry = channels_entry.clone();
        let dwell_entry = dwell_entry.clone();
        let band_combo = band_combo.clone();
        let lock_channel_entry = lock_channel_entry.clone();
        let lock_ht_combo = lock_ht_combo.clone();
        let bluetooth_scan_check = bluetooth_scan_check.clone();
        let bluetooth_source_combo = bluetooth_source_combo.clone();
        let bluetooth_controller_combo = bluetooth_controller_combo.clone();
        let ubertooth_combo = ubertooth_combo.clone();
        let output_to_files_check = output_to_files_check.clone();
        let output_dir_entry = output_dir_entry.clone();
        let start_btn = start_btn.clone();
        let stop_btn = stop_btn.clone();
        let settings_window = settings_window.clone();
        apply_btn.connect_clicked(move |_| {
            let iface_name = interface_combo
                .active_id()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "wlan0".to_string());
            let cap = capabilities_rc
                .borrow()
                .iter()
                .find(|c| c.interface_name == iface_name)
                .cloned();

            let parsed_channels = channels_entry
                .text()
                .split(',')
                .filter_map(|v| v.trim().parse::<u16>().ok())
                .collect::<Vec<_>>();
            let wifi_packet_header_mode = match packet_header_combo.active_id().as_deref() {
                Some("ppi") => WifiPacketHeaderMode::Ppi,
                _ => WifiPacketHeaderMode::Radiotap,
            };
            let dwell_ms = dwell_entry
                .text()
                .parse::<u64>()
                .unwrap_or(200)
                .clamp(50, 5000);
            let lock_ch = lock_channel_entry.text().parse::<u16>().ok().unwrap_or(1);
            let lock_ht_mode = lock_ht_combo
                .active_id()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "HT20".to_string());
            let bluetooth_enabled = bluetooth_scan_check.is_active();
            let bluetooth_scan_source = match bluetooth_source_combo.active_id().as_deref() {
                Some("ubertooth") => BluetoothScanSource::Ubertooth,
                Some("both") => BluetoothScanSource::Both,
                _ => BluetoothScanSource::Bluez,
            };
            let bluetooth_controller = if !bluetooth_enabled {
                None
            } else {
                match bluetooth_controller_combo.active_id().as_deref() {
                    Some("default") | None => None,
                    Some(id) => Some(id.to_string()),
                }
            };
            let ubertooth_device = if !bluetooth_enabled
                || !matches!(
                    bluetooth_scan_source,
                    BluetoothScanSource::Ubertooth | BluetoothScanSource::Both
                ) {
                None
            } else {
                match ubertooth_combo.active_id().as_deref() {
                    Some("default") | None => None,
                    Some(id) => Some(id.to_string()),
                }
            };
            let output_to_files = output_to_files_check.is_active();
            let output_root = if output_to_files {
                let raw = output_dir_entry.text().to_string();
                let trimmed = raw.trim();
                Some(if trimmed.is_empty() {
                    PathBuf::from(".")
                } else {
                    PathBuf::from(trimmed)
                })
            } else {
                None
            };

            let all_channel_details = cap
                .as_ref()
                .map(|c| c.channels.clone())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| {
                    [1_u16, 6, 11, 36, 40, 44, 48]
                        .into_iter()
                        .map(|ch| capture::SupportedChannel {
                            channel: ch,
                            frequency_mhz: None,
                        })
                        .collect()
                });
            let all_channels = all_channel_details
                .iter()
                .map(|c| c.channel)
                .collect::<Vec<_>>();
            let sanitized_parsed_channels = if all_channels.is_empty() {
                parsed_channels.clone()
            } else {
                parsed_channels
                    .iter()
                    .copied()
                    .filter(|ch| all_channels.contains(ch))
                    .collect::<Vec<_>>()
            };
            let dropped_requested_channels =
                !parsed_channels.is_empty() && sanitized_parsed_channels.len() != parsed_channels.len();

            let mode = match mode_combo.active_id().as_deref() {
                Some("locked") => ChannelSelectionMode::Locked {
                    channel: lock_ch,
                    ht_mode: lock_ht_mode,
                },
                Some("hop_band") => {
                    let band = match band_combo.active_id().as_deref() {
                        Some("2.4") => SpectrumBand::Ghz2_4,
                        Some("6") => SpectrumBand::Ghz6,
                        _ => SpectrumBand::Ghz5,
                    };
                    let mut band_channels = filter_channels_for_band(&all_channel_details, &band);
                    if band_channels.is_empty() {
                        band_channels = all_channels.clone();
                    }
                    ChannelSelectionMode::HopBand {
                        band,
                        channels: band_channels,
                        dwell_ms,
                    }
                }
                _ => ChannelSelectionMode::HopAll {
                    channels: if sanitized_parsed_channels.is_empty() {
                        all_channels
                    } else {
                        sanitized_parsed_channels
                    },
                    dwell_ms,
                },
            };

            {
                let mut s = state.borrow_mut();
                if dropped_requested_channels {
                    s.push_status(
                        "some requested channels are not supported on this interface and were removed"
                            .to_string(),
                    );
                }
                s.push_status(format!("preparing scan setup on {}", iface_name));
            }

            settings_window.close();
            apply_interface_selection(
                state.clone(),
                iface_name,
                mode,
                wifi_packet_header_mode,
                bluetooth_enabled,
                bluetooth_scan_source,
                bluetooth_controller,
                ubertooth_device,
                output_to_files,
                start_after_apply,
                output_root,
                start_btn.clone(),
                stop_btn.clone(),
            );
        });
    }

    settings_window.present();
}

fn open_preferences_window(
    window: &ApplicationWindow,
    state: Rc<RefCell<AppState>>,
    content_paned: &Paned,
    global_status_box: &GtkBox,
    pagination_defaults: &PaginationDefaultsUi,
    widgets: &UiWidgets,
) {
    let dialog = Dialog::builder()
        .transient_for(window)
        .modal(true)
        .title("Preferences")
        .default_width(1040)
        .default_height(760)
        .build();
    dialog.add_button("Cancel", ResponseType::Cancel);
    dialog.add_button("Apply", ResponseType::Apply);
    let root = GtkBox::new(Orientation::Horizontal, 12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);
    dialog.content_area().append(&root);

    let sidebar = StackSidebar::new();
    sidebar.set_vexpand(true);
    sidebar.set_size_request(220, -1);
    let stack = Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    sidebar.set_stack(&stack);
    root.append(&sidebar);
    root.append(&stack);

    let section_heading = |text: &str| {
        let label = Label::new(Some(text));
        label.add_css_class("heading");
        label.set_xalign(0.0);
        label
    };

    let page = |stack: &Stack, name: &str, title: &str| -> GtkBox {
        let content = GtkBox::new(Orientation::Vertical, 12);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        let wrapper = ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .child(&content)
            .build();
        stack.add_titled(&wrapper, Some(name), title);
        content
    };

    let settings_snapshot = state.borrow().settings.clone();
    let default_oui_path = settings_snapshot.oui_source_path.clone();
    let show_status_bar_check = CheckButton::with_label("Status Pane");
    show_status_bar_check.set_active(settings_snapshot.show_status_bar);
    let show_detail_pane_check = CheckButton::with_label("Details Pane");
    show_detail_pane_check.set_active(settings_snapshot.show_detail_pane);
    let show_device_pane_check = CheckButton::with_label("Device Pane");
    show_device_pane_check.set_active(settings_snapshot.show_device_pane);

    let ap_columns = Rc::new(RefCell::new(
        settings_snapshot.ap_table_layout.columns.clone(),
    ));
    let client_columns = Rc::new(RefCell::new(
        settings_snapshot.client_table_layout.columns.clone(),
    ));
    let assoc_columns = Rc::new(RefCell::new(
        settings_snapshot.assoc_client_table_layout.columns.clone(),
    ));
    let bluetooth_columns = Rc::new(RefCell::new(
        settings_snapshot.bluetooth_table_layout.columns.clone(),
    ));
    let mut watchlists_initial = settings_snapshot.watchlists.clone();
    migrate_watchlist_settings(&mut watchlists_initial);
    let watchlist_entries = Rc::new(RefCell::new(watchlists_initial.entries.clone()));

    let view_page = page(&stack, "view", "View");
    view_page.append(&section_heading("View Options"));
    view_page.append(&show_status_bar_check);
    view_page.append(&show_detail_pane_check);
    view_page.append(&show_device_pane_check);

    let general_page = page(&stack, "general", "General");
    general_page.append(&section_heading("General"));
    let rows_row = GtkBox::new(Orientation::Horizontal, 8);
    let rows_label = Label::new(Some("Default Rows Per Page"));
    rows_label.set_width_chars(24);
    rows_label.set_xalign(0.0);
    let rows_combo = ComboBoxText::new();
    for rows in TABLE_PAGE_SIZE_OPTIONS {
        rows_combo.append(Some(&rows.to_string()), &rows.to_string());
    }
    rows_combo.set_active_id(Some(&settings_snapshot.default_rows_per_page.to_string()));
    rows_row.append(&rows_label);
    rows_row.append(&rows_combo);
    general_page.append(&rows_row);

    let wifi_page = page(&stack, "wifi_capture", "Wi-Fi / Capture");
    wifi_page.append(&section_heading("Wi-Fi / Capture"));
    let packet_header_row = GtkBox::new(Orientation::Horizontal, 8);
    let packet_header_label = Label::new(Some("Packet Headers"));
    packet_header_label.set_width_chars(24);
    packet_header_label.set_xalign(0.0);
    let packet_header_combo = ComboBoxText::new();
    packet_header_combo.append(Some("radiotap"), "Radiotap");
    packet_header_combo.append(Some("ppi"), "PPI");
    packet_header_combo.set_active_id(Some(match settings_snapshot.wifi_packet_header_mode {
        WifiPacketHeaderMode::Radiotap => "radiotap",
        WifiPacketHeaderMode::Ppi => "ppi",
    }));
    packet_header_row.append(&packet_header_label);
    packet_header_row.append(&packet_header_combo);
    wifi_page.append(&packet_header_row);

    let wifi_summary = Label::new(None);
    wifi_summary.set_xalign(0.0);
    wifi_summary.set_wrap(true);
    {
        let summary = settings_snapshot
            .interfaces
            .first()
            .map(|iface| {
                format!(
                    "Current interface: {} | mode: {}",
                    iface.interface_name,
                    describe_channel_mode(&iface.channel_mode)
                )
            })
            .unwrap_or_else(|| "No Wi-Fi interface configured".to_string());
        wifi_summary.set_text(&summary);
    }
    let interface_button = Button::with_label("Open Wi-Fi Interface Configuration");
    {
        let window = window.clone();
        let state = state.clone();
        interface_button.connect_clicked(move |_| {
            open_interface_settings_dialog(&window, state.clone());
        });
    }
    wifi_page.append(&wifi_summary);
    wifi_page.append(&interface_button);

    let gps_page = page(&stack, "gps", "GPS");
    gps_page.append(&section_heading("GPS"));
    let gps_mode_combo = ComboBoxText::new();
    gps_mode_combo.append(Some("disabled"), "Disabled");
    gps_mode_combo.append(Some("interface"), "Interface");
    gps_mode_combo.append(Some("gpsd"), "GPSD");
    gps_mode_combo.append(Some("stream"), "Stream (TCP/UDP NMEA)");
    gps_mode_combo.append(Some("static"), "Static Location");

    let gps_interface_entry = Entry::new();
    gps_interface_entry.set_placeholder_text(Some("/dev/ttyUSB0"));
    let gps_host_entry = Entry::new();
    gps_host_entry.set_placeholder_text(Some("127.0.0.1"));
    let gps_port_entry = Entry::new();
    gps_port_entry.set_placeholder_text(Some("2947"));
    let gps_protocol_combo = ComboBoxText::new();
    gps_protocol_combo.append(Some("tcp"), "TCP");
    gps_protocol_combo.append(Some("udp"), "UDP");
    let gps_lat_entry = Entry::new();
    gps_lat_entry.set_placeholder_text(Some("37.7749"));
    let gps_lon_entry = Entry::new();
    gps_lon_entry.set_placeholder_text(Some("-122.4194"));
    let gps_alt_entry = Entry::new();
    gps_alt_entry.set_placeholder_text(Some("15.0"));

    match &settings_snapshot.gps {
        GpsSettings::Disabled => {
            gps_mode_combo.set_active_id(Some("disabled"));
        }
        GpsSettings::Interface { device_path } => {
            gps_mode_combo.set_active_id(Some("interface"));
            gps_interface_entry.set_text(device_path);
        }
        GpsSettings::Gpsd { host, port } => {
            gps_mode_combo.set_active_id(Some("gpsd"));
            gps_host_entry.set_text(host);
            gps_port_entry.set_text(&port.to_string());
        }
        GpsSettings::Stream {
            protocol,
            host,
            port,
        } => {
            gps_mode_combo.set_active_id(Some("stream"));
            gps_protocol_combo.set_active_id(Some(match protocol {
                StreamProtocol::Tcp => "tcp",
                StreamProtocol::Udp => "udp",
            }));
            gps_host_entry.set_text(host);
            gps_port_entry.set_text(&port.to_string());
        }
        GpsSettings::Static {
            latitude,
            longitude,
            altitude_m,
        } => {
            gps_mode_combo.set_active_id(Some("static"));
            gps_lat_entry.set_text(&latitude.to_string());
            gps_lon_entry.set_text(&longitude.to_string());
            if let Some(altitude_m) = altitude_m {
                gps_alt_entry.set_text(&altitude_m.to_string());
            }
        }
    }

    for (label_text, widget) in [
        ("Mode", gps_mode_combo.upcast_ref::<gtk::Widget>()),
        (
            "Interface Path",
            gps_interface_entry.upcast_ref::<gtk::Widget>(),
        ),
        ("Host", gps_host_entry.upcast_ref::<gtk::Widget>()),
        ("Port", gps_port_entry.upcast_ref::<gtk::Widget>()),
        (
            "Stream Protocol",
            gps_protocol_combo.upcast_ref::<gtk::Widget>(),
        ),
        ("Static Latitude", gps_lat_entry.upcast_ref::<gtk::Widget>()),
        (
            "Static Longitude",
            gps_lon_entry.upcast_ref::<gtk::Widget>(),
        ),
        (
            "Static Altitude M",
            gps_alt_entry.upcast_ref::<gtk::Widget>(),
        ),
    ] {
        let row = GtkBox::new(Orientation::Horizontal, 8);
        let label = Label::new(Some(label_text));
        label.set_width_chars(24);
        label.set_xalign(0.0);
        row.append(&label);
        row.append(widget);
        gps_page.append(&row);
    }

    let bluetooth_page = page(&stack, "bluetooth", "Bluetooth");
    bluetooth_page.append(&section_heading("Bluetooth"));
    let bluetooth_enabled_check = CheckButton::with_label("Enable Bluetooth Scanning");
    bluetooth_enabled_check.set_active(settings_snapshot.bluetooth_enabled);
    bluetooth_page.append(&bluetooth_enabled_check);

    let bluetooth_controller_combo = ComboBoxText::new();
    bluetooth_controller_combo.append(Some("default"), "Default Controller");
    for ctrl in bluetooth::list_controllers().unwrap_or_default() {
        bluetooth_controller_combo.append(
            Some(&ctrl.id),
            &format!(
                "{} ({}){}",
                ctrl.id,
                if ctrl.name.is_empty() {
                    "unnamed"
                } else {
                    ctrl.name.as_str()
                },
                if ctrl.is_default { " [default]" } else { "" }
            ),
        );
    }
    bluetooth_controller_combo.set_active_id(
        settings_snapshot
            .bluetooth_controller
            .as_deref()
            .or(Some("default")),
    );
    let bluetooth_source_combo = ComboBoxText::new();
    bluetooth_source_combo.append(Some("bluez"), "BlueZ");
    bluetooth_source_combo.append(Some("ubertooth"), "Ubertooth");
    bluetooth_source_combo.append(Some("both"), "BlueZ + Ubertooth");
    bluetooth_source_combo.set_active_id(Some(match settings_snapshot.bluetooth_scan_source {
        BluetoothScanSource::Bluez => "bluez",
        BluetoothScanSource::Ubertooth => "ubertooth",
        BluetoothScanSource::Both => "both",
    }));

    let ubertooth_device_combo = ComboBoxText::new();
    ubertooth_device_combo.append(Some("default"), "Default Ubertooth Device");
    for device in bluetooth::list_ubertooth_devices().unwrap_or_default() {
        ubertooth_device_combo.append(Some(&device.id), &device.name);
    }
    ubertooth_device_combo.set_active_id(
        settings_snapshot
            .ubertooth_device
            .as_deref()
            .or(Some("default")),
    );

    let bluetooth_timeout_entry = Entry::new();
    bluetooth_timeout_entry.set_text(&settings_snapshot.bluetooth_scan_timeout_secs.to_string());
    let bluetooth_pause_entry = Entry::new();
    bluetooth_pause_entry.set_text(&settings_snapshot.bluetooth_scan_pause_ms.to_string());

    for (label_text, widget) in [
        (
            "Bluetooth Source",
            bluetooth_source_combo.upcast_ref::<gtk::Widget>(),
        ),
        (
            "Bluetooth Radio",
            bluetooth_controller_combo.upcast_ref::<gtk::Widget>(),
        ),
        (
            "Ubertooth Device",
            ubertooth_device_combo.upcast_ref::<gtk::Widget>(),
        ),
        (
            "Scan Timeout Seconds",
            bluetooth_timeout_entry.upcast_ref::<gtk::Widget>(),
        ),
        (
            "Scan Pause Milliseconds",
            bluetooth_pause_entry.upcast_ref::<gtk::Widget>(),
        ),
    ] {
        let row = GtkBox::new(Orientation::Horizontal, 8);
        let label = Label::new(Some(label_text));
        label.set_width_chars(24);
        label.set_xalign(0.0);
        row.append(&label);
        row.append(widget);
        bluetooth_page.append(&row);
    }

    {
        let bluetooth_enabled_check = bluetooth_enabled_check.clone();
        let bluetooth_source_combo = bluetooth_source_combo.clone();
        let bluetooth_controller_combo = bluetooth_controller_combo.clone();
        let ubertooth_device_combo = ubertooth_device_combo.clone();
        let enabled_for_update = bluetooth_enabled_check.clone();
        let source_for_update = bluetooth_source_combo.clone();
        let controller_for_update = bluetooth_controller_combo.clone();
        let ubertooth_for_update = ubertooth_device_combo.clone();
        let update_controls = move || {
            let enabled = enabled_for_update.is_active();
            let source = source_for_update
                .active_id()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "bluez".to_string());
            let uses_bluez = matches!(source.as_str(), "bluez" | "both");
            let uses_ubertooth = matches!(source.as_str(), "ubertooth" | "both");
            source_for_update.set_sensitive(enabled);
            controller_for_update.set_sensitive(enabled && uses_bluez);
            ubertooth_for_update.set_sensitive(enabled && uses_ubertooth);
        };
        update_controls();

        let update_controls = Rc::new(update_controls);

        {
            let update_controls = update_controls.clone();
            let bluetooth_enabled_check = bluetooth_enabled_check.clone();
            bluetooth_enabled_check.connect_toggled(move |_| {
                (update_controls)();
            });
        }

        {
            let update_controls = update_controls.clone();
            let bluetooth_source_combo = bluetooth_source_combo.clone();
            bluetooth_source_combo.connect_changed(move |_| {
                (update_controls)();
            });
        }
    }

    {
        let bluetooth_source_combo = bluetooth_source_combo.clone();
        let ubertooth_device_combo = ubertooth_device_combo.clone();
        let bluetooth_enabled_check = bluetooth_enabled_check.clone();
        let refresh_button = Button::with_label("Refresh Ubertooth List");
        let refresh_row = GtkBox::new(Orientation::Horizontal, 8);
        let refresh_label = Label::new(Some(""));
        refresh_label.set_width_chars(24);
        refresh_row.append(&refresh_label);
        refresh_row.append(&refresh_button);
        bluetooth_page.append(&refresh_row);

        refresh_button.connect_clicked(move |_| {
            let current = ubertooth_device_combo.active_id().map(|v| v.to_string());
            ubertooth_device_combo.remove_all();
            ubertooth_device_combo.append(Some("default"), "Default Ubertooth Device");
            for device in bluetooth::list_ubertooth_devices().unwrap_or_default() {
                ubertooth_device_combo.append(Some(&device.id), &device.name);
            }
            if let Some(current) = current {
                if !ubertooth_device_combo.set_active_id(Some(&current)) {
                    ubertooth_device_combo.set_active_id(Some("default"));
                }
            } else {
                ubertooth_device_combo.set_active_id(Some("default"));
            }

            if !bluetooth_enabled_check.is_active() {
                ubertooth_device_combo.set_sensitive(false);
            } else {
                let uses_ubertooth = matches!(
                    bluetooth_source_combo.active_id().as_deref(),
                    Some("ubertooth") | Some("both")
                );
                ubertooth_device_combo.set_sensitive(uses_ubertooth);
            }
        });
    }

    let data_sources_page = page(&stack, "data_sources", "Data Sources");
    data_sources_page.append(&section_heading("Data Sources"));
    let oui_row = GtkBox::new(Orientation::Horizontal, 8);
    let oui_label = Label::new(Some("OUI File"));
    oui_label.set_width_chars(24);
    oui_label.set_xalign(0.0);
    let oui_entry = Entry::new();
    oui_entry.set_hexpand(true);
    oui_entry.set_text(&settings_snapshot.oui_source_path.display().to_string());
    let oui_browse_btn = Button::with_label("Browse");
    oui_row.append(&oui_label);
    oui_row.append(&oui_entry);
    oui_row.append(&oui_browse_btn);
    data_sources_page.append(&oui_row);

    let ap_fields_page = page(&stack, "table_fields_ap", "Table Fields: Wi-Fi AP");
    ap_fields_page.append(&build_table_layout_editor(
        "Wi-Fi Access Point Columns",
        ap_columns.clone(),
        ap_column_label,
    ));

    let client_fields_page = page(&stack, "table_fields_client", "Table Fields: Wi-Fi Clients");
    client_fields_page.append(&build_table_layout_editor(
        "Wi-Fi Client Columns",
        client_columns.clone(),
        client_column_label,
    ));

    let assoc_fields_page = page(
        &stack,
        "table_fields_assoc",
        "Table Fields: Associated Clients",
    );
    assoc_fields_page.append(&build_table_layout_editor(
        "Associated Client Columns",
        assoc_columns.clone(),
        assoc_client_column_label,
    ));

    let bluetooth_fields_page = page(&stack, "table_fields_bluetooth", "Table Fields: Bluetooth");
    bluetooth_fields_page.append(&build_table_layout_editor(
        "Bluetooth Device Columns",
        bluetooth_columns.clone(),
        bluetooth_column_label,
    ));

    let alerts_page = page(&stack, "alerts", "Alerts / Watchlists");
    alerts_page.append(&section_heading("Alerts / Watchlists"));
    let handshake_alerts_check = CheckButton::with_label("Enable handshake alerts");
    handshake_alerts_check.set_active(settings_snapshot.enable_handshake_alerts);
    let watchlist_alerts_check = CheckButton::with_label("Enable watchlist alerts");
    watchlist_alerts_check.set_active(settings_snapshot.enable_watchlist_alerts);

    alerts_page.append(&handshake_alerts_check);
    alerts_page.append(&watchlist_alerts_check);
    alerts_page.append(&build_watchlist_editor(watchlist_entries.clone()));

    {
        let dialog = dialog.clone();
        let oui_entry = oui_entry.clone();
        oui_browse_btn.connect_clicked(move |_| {
            let current = oui_entry.text().to_string();
            let initial = if current.trim().is_empty() {
                PathBuf::from(".")
            } else {
                PathBuf::from(current)
            };
            let oui_entry = oui_entry.clone();
            choose_file_path(&dialog, "Select OUI File", initial, move |selected| {
                if let Some(path) = selected {
                    oui_entry.set_text(&path.display().to_string());
                }
            });
        });
    }

    {
        let state = state.clone();
        let window = window.clone();
        let content_paned = content_paned.clone();
        let global_status_box = global_status_box.clone();
        let pagination_defaults = pagination_defaults.clone();
        let widgets = widgets.clone();
        let default_oui_path = default_oui_path.clone();
        dialog.connect_response(move |d, resp| {
            if resp == ResponseType::Apply {
                let requested_rows = rows_combo
                    .active_id()
                    .and_then(|value| value.as_str().parse::<usize>().ok())
                    .unwrap_or(DEFAULT_TABLE_PAGE_SIZE)
                    .max(1);

                let gps_settings = match gps_mode_combo.active_id().as_deref() {
                    Some("interface") => GpsSettings::Interface {
                        device_path: gps_interface_entry.text().to_string(),
                    },
                    Some("gpsd") => GpsSettings::Gpsd {
                        host: gps_host_entry.text().to_string(),
                        port: gps_port_entry.text().parse::<u16>().unwrap_or(2947),
                    },
                    Some("stream") => GpsSettings::Stream {
                        protocol: match gps_protocol_combo.active_id().as_deref() {
                            Some("udp") => StreamProtocol::Udp,
                            _ => StreamProtocol::Tcp,
                        },
                        host: gps_host_entry.text().to_string(),
                        port: gps_port_entry.text().parse::<u16>().unwrap_or(10110),
                    },
                    Some("static") => GpsSettings::Static {
                        latitude: gps_lat_entry.text().parse::<f64>().unwrap_or(0.0),
                        longitude: gps_lon_entry.text().parse::<f64>().unwrap_or(0.0),
                        altitude_m: gps_alt_entry.text().parse::<f64>().ok(),
                    },
                    _ => GpsSettings::Disabled,
                };
                let wifi_packet_header_mode = match packet_header_combo.active_id().as_deref() {
                    Some("ppi") => WifiPacketHeaderMode::Ppi,
                    _ => WifiPacketHeaderMode::Radiotap,
                };

                let bluetooth_enabled = bluetooth_enabled_check.is_active();
                let bluetooth_scan_source = match bluetooth_source_combo.active_id().as_deref() {
                    Some("ubertooth") => BluetoothScanSource::Ubertooth,
                    Some("both") => BluetoothScanSource::Both,
                    _ => BluetoothScanSource::Bluez,
                };
                let bluetooth_controller = if !bluetooth_enabled {
                    None
                } else {
                    match bluetooth_controller_combo.active_id().as_deref() {
                        Some("default") | None => None,
                        Some(id) => Some(id.to_string()),
                    }
                };
                let ubertooth_device = if !bluetooth_enabled
                    || !matches!(
                        bluetooth_scan_source,
                        BluetoothScanSource::Ubertooth | BluetoothScanSource::Both
                    ) {
                    None
                } else {
                    match ubertooth_device_combo.active_id().as_deref() {
                        Some("default") | None => None,
                        Some(id) => Some(id.to_string()),
                    }
                };
                let bluetooth_timeout = bluetooth_timeout_entry
                    .text()
                    .parse::<u64>()
                    .unwrap_or(4)
                    .clamp(2, 12);
                let bluetooth_pause = bluetooth_pause_entry
                    .text()
                    .parse::<u64>()
                    .unwrap_or(500)
                    .clamp(100, 5_000);

                let oui_path_text = oui_entry.text().to_string();
                let oui_path = if oui_path_text.trim().is_empty() {
                    default_oui_path.clone()
                } else {
                    PathBuf::from(oui_path_text.trim())
                };

                let mut full_restart_needed = false;
                let mut bluetooth_restart_needed = false;
                let mut applied_messages = Vec::new();

                let view_changed = {
                    let mut s = state.borrow_mut();
                    let view_changed = s.settings.show_status_bar
                        != show_status_bar_check.is_active()
                        || s.settings.show_detail_pane != show_detail_pane_check.is_active()
                        || s.settings.show_device_pane != show_device_pane_check.is_active();

                    if s.settings.default_rows_per_page != requested_rows {
                        s.settings.default_rows_per_page = requested_rows;
                        for pagination in [
                            &pagination_defaults.ap,
                            &pagination_defaults.client,
                            &pagination_defaults.assoc,
                            &pagination_defaults.bluetooth,
                        ] {
                            pagination
                                .page_size_combo
                                .set_active_id(Some(&requested_rows.to_string()));
                            pagination.current_page.set(0);
                            pagination
                                .generation
                                .set(pagination.generation.get().saturating_add(1));
                        }
                        applied_messages
                            .push(format!("default rows per page set to {}", requested_rows));
                    }

                    if s.settings.gps != gps_settings {
                        s.update_gps_provider(gps_settings);
                        applied_messages.push("gps settings applied".to_string());
                    }

                    let bluetooth_changed = s.settings.bluetooth_enabled != bluetooth_enabled
                        || s.settings.bluetooth_scan_source != bluetooth_scan_source
                        || s.settings.bluetooth_controller != bluetooth_controller
                        || s.settings.ubertooth_device != ubertooth_device
                        || s.settings.bluetooth_scan_timeout_secs != bluetooth_timeout
                        || s.settings.bluetooth_scan_pause_ms != bluetooth_pause;
                    if bluetooth_changed {
                        s.settings.bluetooth_enabled = bluetooth_enabled;
                        s.settings.bluetooth_scan_source = bluetooth_scan_source;
                        s.settings.bluetooth_controller = bluetooth_controller;
                        s.settings.ubertooth_device = ubertooth_device;
                        s.settings.bluetooth_scan_timeout_secs = bluetooth_timeout;
                        s.settings.bluetooth_scan_pause_ms = bluetooth_pause;
                        bluetooth_restart_needed = s.bluetooth_runtime.is_some();
                        applied_messages.push("bluetooth settings applied".to_string());
                    }

                    if s.settings.oui_source_path != oui_path {
                        let previous_oui_path = s.settings.oui_source_path.clone();
                        s.settings.oui_source_path = oui_path.clone();
                        match s.reload_oui_from_settings() {
                            Ok(count) => applied_messages.push(format!(
                                "OUI database loaded from {} ({} entries)",
                                oui_path.display(),
                                count
                            )),
                            Err(err) => {
                                s.settings.oui_source_path = previous_oui_path;
                                let _ = s.reload_oui_from_settings();
                                applied_messages.push(format!(
                                    "failed to load OUI database from {}: {}",
                                    oui_path.display(),
                                    err
                                ));
                            }
                        }
                        s.layout_dirty = true;
                    }

                    if s.settings.wifi_packet_header_mode != wifi_packet_header_mode {
                        s.settings.wifi_packet_header_mode = wifi_packet_header_mode;
                        full_restart_needed |= s.capture_runtime.is_some();
                        applied_messages.push(format!(
                            "wifi packet headers set to {}",
                            match wifi_packet_header_mode {
                                WifiPacketHeaderMode::Radiotap => "Radiotap",
                                WifiPacketHeaderMode::Ppi => "PPI",
                            }
                        ));
                    }

                    let ap_previous = s.settings.ap_table_layout.columns.clone();
                    let client_previous = s.settings.client_table_layout.columns.clone();
                    let assoc_previous = s.settings.assoc_client_table_layout.columns.clone();
                    let bluetooth_previous = s.settings.bluetooth_table_layout.columns.clone();
                    s.settings.ap_table_layout.columns = ap_columns.borrow().clone();
                    s.settings.client_table_layout.columns = client_columns.borrow().clone();
                    s.settings.assoc_client_table_layout.columns = assoc_columns.borrow().clone();
                    s.settings.bluetooth_table_layout.columns = bluetooth_columns.borrow().clone();
                    sanitize_table_layout(
                        &mut s.settings.ap_table_layout,
                        &default_ap_table_layout(),
                    );
                    sanitize_table_layout(
                        &mut s.settings.client_table_layout,
                        &default_client_table_layout(),
                    );
                    sanitize_table_layout(
                        &mut s.settings.assoc_client_table_layout,
                        &default_assoc_client_table_layout(),
                    );
                    migrate_assoc_client_table_layout(&mut s.settings.assoc_client_table_layout);
                    sanitize_table_layout(
                        &mut s.settings.bluetooth_table_layout,
                        &default_bluetooth_table_layout(),
                    );
                    if s.settings.ap_table_layout.columns != ap_previous
                        || s.settings.client_table_layout.columns != client_previous
                        || s.settings.assoc_client_table_layout.columns != assoc_previous
                        || s.settings.bluetooth_table_layout.columns != bluetooth_previous
                    {
                        s.layout_dirty = true;
                        applied_messages.push("table field preferences applied".to_string());
                    }

                    let watchlists_previous = s.settings.watchlists.clone();
                    let handshake_alerts_previous = s.settings.enable_handshake_alerts;
                    let watchlist_alerts_previous = s.settings.enable_watchlist_alerts;
                    s.settings.enable_handshake_alerts = handshake_alerts_check.is_active();
                    s.settings.enable_watchlist_alerts = watchlist_alerts_check.is_active();
                    let mut watchlists = WatchlistSettings {
                        entries: watchlist_entries.borrow().clone(),
                        ..WatchlistSettings::default()
                    };
                    migrate_watchlist_settings(&mut watchlists);
                    s.settings.watchlists = watchlists;
                    if s.settings.watchlists != watchlists_previous
                        || s.settings.enable_handshake_alerts != handshake_alerts_previous
                        || s.settings.enable_watchlist_alerts != watchlist_alerts_previous
                    {
                        apply_watchlist_css(&s.watchlist_css_provider, &s.settings.watchlists);
                        s.alerted_watch_entities.clear();
                        s.layout_dirty = true;
                        applied_messages.push("alert and watchlist settings applied".to_string());
                    }

                    for message in applied_messages {
                        s.push_status(message);
                    }

                    if full_restart_needed {
                        let _ = s.begin_async_scan_shutdown(Some(
                            "settings applied; restarting capture".to_string(),
                        ));
                    } else if bluetooth_restart_needed {
                        s.restart_bluetooth_scan();
                    } else {
                        s.push_status("preferences applied".to_string());
                    }
                    view_changed
                };

                apply_view_preferences(
                    &state,
                    &content_paned,
                    &global_status_box,
                    &widgets,
                    Some(show_status_bar_check.is_active()),
                    Some(show_detail_pane_check.is_active()),
                    Some(show_device_pane_check.is_active()),
                );
                state.borrow_mut().save_settings_to_disk();
                if view_changed {
                    if let Some(app) = window.application() {
                        sync_view_menu_action_state(
                            &app,
                            "settings_show_status_bar",
                            show_status_bar_check.is_active(),
                        );
                        sync_view_menu_action_state(
                            &app,
                            "settings_show_detail_pane",
                            show_detail_pane_check.is_active(),
                        );
                        sync_view_menu_action_state(
                            &app,
                            "settings_show_device_pane",
                            show_device_pane_check.is_active(),
                        );
                    }
                }
            }
            d.close();
        });
    }

    dialog.present();
}

fn open_gps_settings_dialog(window: &ApplicationWindow, state: Rc<RefCell<AppState>>) {
    let dialog = Dialog::builder()
        .transient_for(window)
        .modal(true)
        .title("GPS Settings")
        .default_width(580)
        .default_height(350)
        .build();

    dialog.add_button("Cancel", ResponseType::Cancel);
    dialog.add_button("Apply", ResponseType::Apply);

    let area = dialog.content_area();

    let mode_combo = ComboBoxText::new();
    mode_combo.append(Some("disabled"), "Disabled");
    mode_combo.append(Some("interface"), "Interface");
    mode_combo.append(Some("gpsd"), "GPSD");
    mode_combo.append(Some("stream"), "Stream (TCP/UDP NMEA)");
    mode_combo.append(Some("static"), "Static Location");
    mode_combo.set_active_id(Some("disabled"));

    let interface_entry = Entry::new();
    interface_entry.set_placeholder_text(Some("/dev/ttyUSB0"));

    let host_entry = Entry::new();
    host_entry.set_placeholder_text(Some("127.0.0.1"));
    let port_entry = Entry::new();
    port_entry.set_placeholder_text(Some("2947"));

    let protocol_combo = ComboBoxText::new();
    protocol_combo.append(Some("tcp"), "TCP");
    protocol_combo.append(Some("udp"), "UDP");
    protocol_combo.set_active_id(Some("tcp"));

    let lat_entry = Entry::new();
    lat_entry.set_placeholder_text(Some("37.7749"));
    let lon_entry = Entry::new();
    lon_entry.set_placeholder_text(Some("-122.4194"));
    let alt_entry = Entry::new();
    alt_entry.set_placeholder_text(Some("15.0"));

    area.append(&Label::new(Some("Mode")));
    area.append(&mode_combo);
    area.append(&Label::new(Some("Interface Path")));
    area.append(&interface_entry);
    area.append(&Label::new(Some("Host")));
    area.append(&host_entry);
    area.append(&Label::new(Some("Port")));
    area.append(&port_entry);
    area.append(&Label::new(Some("Stream Protocol")));
    area.append(&protocol_combo);
    area.append(&Label::new(Some("Static Latitude")));
    area.append(&lat_entry);
    area.append(&Label::new(Some("Static Longitude")));
    area.append(&lon_entry);
    area.append(&Label::new(Some("Static Altitude M")));
    area.append(&alt_entry);

    {
        let state = state.clone();
        dialog.connect_response(move |d, resp| {
            if resp == ResponseType::Apply {
                let settings = match mode_combo.active_id().as_deref() {
                    Some("interface") => GpsSettings::Interface {
                        device_path: interface_entry.text().to_string(),
                    },
                    Some("gpsd") => GpsSettings::Gpsd {
                        host: host_entry.text().to_string(),
                        port: port_entry.text().parse::<u16>().unwrap_or(2947),
                    },
                    Some("stream") => GpsSettings::Stream {
                        protocol: match protocol_combo.active_id().as_deref() {
                            Some("udp") => StreamProtocol::Udp,
                            _ => StreamProtocol::Tcp,
                        },
                        host: host_entry.text().to_string(),
                        port: port_entry.text().parse::<u16>().unwrap_or(10110),
                    },
                    Some("static") => GpsSettings::Static {
                        latitude: lat_entry.text().parse::<f64>().unwrap_or(0.0),
                        longitude: lon_entry.text().parse::<f64>().unwrap_or(0.0),
                        altitude_m: alt_entry.text().parse::<f64>().ok(),
                    },
                    _ => GpsSettings::Disabled,
                };

                let mut s = state.borrow_mut();
                s.update_gps_provider(settings);
                s.save_settings_to_disk();
                s.push_status("gps settings applied".to_string());
            }

            d.close();
        });
    }

    dialog.present();
}

fn open_bluetooth_settings_dialog(window: &ApplicationWindow, state: Rc<RefCell<AppState>>) {
    let dialog = Dialog::builder()
        .transient_for(window)
        .modal(true)
        .title("Bluetooth Settings")
        .default_width(520)
        .default_height(260)
        .build();

    dialog.add_button("Cancel", ResponseType::Cancel);
    dialog.add_button("Apply", ResponseType::Apply);

    let area = dialog.content_area();

    let controller_combo = ComboBoxText::new();
    controller_combo.append(Some("default"), "Default Controller");
    let controllers = bluetooth::list_controllers().unwrap_or_default();
    for ctrl in &controllers {
        controller_combo.append(
            Some(&ctrl.id),
            &format!(
                "{} ({}){}",
                ctrl.id,
                if ctrl.name.is_empty() {
                    "unnamed"
                } else {
                    ctrl.name.as_str()
                },
                if ctrl.is_default { " [default]" } else { "" }
            ),
        );
    }

    let source_combo = ComboBoxText::new();
    source_combo.append(Some("bluez"), "BlueZ");
    source_combo.append(Some("ubertooth"), "Ubertooth");
    source_combo.append(Some("both"), "BlueZ + Ubertooth");

    let ubertooth_combo = ComboBoxText::new();
    ubertooth_combo.append(Some("default"), "Default Ubertooth Device");
    for device in bluetooth::list_ubertooth_devices().unwrap_or_default() {
        ubertooth_combo.append(Some(&device.id), &device.name);
    }

    let scan_timeout_entry = Entry::new();
    scan_timeout_entry.set_placeholder_text(Some("Scan timeout seconds"));
    let scan_pause_entry = Entry::new();
    scan_pause_entry.set_placeholder_text(Some("Pause milliseconds"));

    {
        let s = state.borrow();
        controller_combo.set_active_id(
            s.settings
                .bluetooth_controller
                .as_deref()
                .or(Some("default")),
        );
        source_combo.set_active_id(Some(match s.settings.bluetooth_scan_source {
            BluetoothScanSource::Bluez => "bluez",
            BluetoothScanSource::Ubertooth => "ubertooth",
            BluetoothScanSource::Both => "both",
        }));
        ubertooth_combo.set_active_id(s.settings.ubertooth_device.as_deref().or(Some("default")));
        scan_timeout_entry.set_text(&s.settings.bluetooth_scan_timeout_secs.to_string());
        scan_pause_entry.set_text(&s.settings.bluetooth_scan_pause_ms.to_string());
    }

    area.append(&Label::new(Some("Bluetooth Source")));
    area.append(&source_combo);
    area.append(&Label::new(Some("Bluetooth Radio")));
    area.append(&controller_combo);
    area.append(&Label::new(Some("Ubertooth Device")));
    area.append(&ubertooth_combo);
    area.append(&Label::new(Some("Scan Timeout Seconds")));
    area.append(&scan_timeout_entry);
    area.append(&Label::new(Some("Scan Pause Milliseconds")));
    area.append(&scan_pause_entry);

    {
        let source_combo = source_combo.clone();
        let controller_combo = controller_combo.clone();
        let ubertooth_combo = ubertooth_combo.clone();
        let source_for_update = source_combo.clone();
        let controller_for_update = controller_combo.clone();
        let ubertooth_for_update = ubertooth_combo.clone();
        let update_visibility = move || {
            let source = source_for_update
                .active_id()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "bluez".to_string());
            controller_for_update.set_sensitive(matches!(source.as_str(), "bluez" | "both"));
            ubertooth_for_update.set_sensitive(matches!(source.as_str(), "ubertooth" | "both"));
        };
        update_visibility();
        let update_visibility = Rc::new(update_visibility);
        {
            let update_visibility = update_visibility.clone();
            let source_combo = source_combo.clone();
            source_combo.connect_changed(move |_| {
                (update_visibility)();
            });
        }
    }

    {
        let state = state.clone();
        dialog.connect_response(move |d, resp| {
            if resp == ResponseType::Apply {
                let source = match source_combo.active_id().as_deref() {
                    Some("ubertooth") => BluetoothScanSource::Ubertooth,
                    Some("both") => BluetoothScanSource::Both,
                    _ => BluetoothScanSource::Bluez,
                };
                let controller = match controller_combo.active_id().as_deref() {
                    Some("default") | None => None,
                    Some(v) => Some(v.to_string()),
                };
                let ubertooth_device = match ubertooth_combo.active_id().as_deref() {
                    Some("default") | None => None,
                    Some(v) => Some(v.to_string()),
                };
                let timeout = scan_timeout_entry
                    .text()
                    .parse::<u64>()
                    .unwrap_or(4)
                    .clamp(2, 12);
                let pause = scan_pause_entry
                    .text()
                    .parse::<u64>()
                    .unwrap_or(500)
                    .clamp(100, 5_000);

                let mut s = state.borrow_mut();
                s.settings.bluetooth_scan_source = source;
                s.settings.bluetooth_controller = controller;
                s.settings.ubertooth_device = ubertooth_device;
                s.settings.bluetooth_scan_timeout_secs = timeout;
                s.settings.bluetooth_scan_pause_ms = pause;
                s.save_settings_to_disk();
                s.restart_bluetooth_scan();
                s.push_status("bluetooth settings applied".to_string());
            }
            d.close();
        });
    }

    dialog.present();
}

fn choose_output_dir<W: IsA<gtk::Window>>(
    window: &W,
    initial_path: PathBuf,
    on_selected: impl FnOnce(Option<PathBuf>) + 'static,
) {
    let chooser = FileChooserDialog::builder()
        .transient_for(window)
        .modal(true)
        .title("Select Output Folder")
        .action(FileChooserAction::SelectFolder)
        .build();
    chooser.add_button("Cancel", ResponseType::Cancel);
    chooser.add_button("Select", ResponseType::Accept);
    let initial_folder = gio::File::for_path(&initial_path);
    let _ = chooser.set_current_folder(Some(&initial_folder));

    let callback = Rc::new(RefCell::new(Some(
        Box::new(on_selected) as Box<dyn FnOnce(Option<PathBuf>)>
    )));
    let fallback_path = initial_path.clone();
    let chooser_keepalive = chooser.clone();
    chooser.connect_response(move |d, resp| {
        let _keepalive = &chooser_keepalive;
        let cancelled = matches!(
            resp,
            ResponseType::Cancel | ResponseType::Close | ResponseType::DeleteEvent
        );
        let path = if cancelled {
            None
        } else {
            d.file()
                .and_then(|f| f.path())
                .or_else(|| d.current_folder().and_then(|f| f.path()))
                .or_else(|| Some(fallback_path.clone()))
        };
        d.close();
        if let Some(cb) = callback.borrow_mut().take() {
            cb(path);
        }
    });
    chooser.present();
}

fn choose_file_path<W: IsA<gtk::Window>>(
    window: &W,
    title: &str,
    initial_path: PathBuf,
    on_selected: impl FnOnce(Option<PathBuf>) + 'static,
) {
    let chooser = FileChooserDialog::builder()
        .transient_for(window)
        .modal(true)
        .title(title)
        .action(FileChooserAction::Open)
        .build();
    chooser.add_button("Cancel", ResponseType::Cancel);
    chooser.add_button("Select", ResponseType::Accept);
    let initial_file = gio::File::for_path(&initial_path);
    let _ = chooser.set_file(&initial_file);

    let callback = Rc::new(RefCell::new(Some(
        Box::new(on_selected) as Box<dyn FnOnce(Option<PathBuf>)>
    )));
    let chooser_keepalive = chooser.clone();
    chooser.connect_response(move |d, resp| {
        let _keepalive = &chooser_keepalive;
        let path = if matches!(
            resp,
            ResponseType::Cancel | ResponseType::Close | ResponseType::DeleteEvent
        ) {
            None
        } else {
            d.file().and_then(|f| f.path())
        };
        d.close();
        if let Some(cb) = callback.borrow_mut().take() {
            cb(path);
        }
    });
    chooser.present();
}

fn detect_interface_settings() -> Vec<InterfaceSettings> {
    let Some(cap) = detect_wifi_interface_capabilities().into_iter().next() else {
        return Vec::new();
    };

    vec![InterfaceSettings {
        interface_name: cap.interface_name,
        monitor_interface_name: None,
        channel_mode: ChannelSelectionMode::HopAll {
            channels: cap.channels.into_iter().map(|c| c.channel).collect(),
            dwell_ms: 200,
        },
        enabled: true,
    }]
}

fn merge_ap(aps: &mut Vec<AccessPointRecord>, incoming: AccessPointRecord) {
    if let Some(existing) = aps.iter_mut().find(|ap| ap.bssid == incoming.bssid) {
        if incoming.ssid.is_some() {
            existing.ssid = incoming.ssid;
        }
        if incoming.oui_manufacturer.is_some() {
            existing.oui_manufacturer = incoming.oui_manufacturer;
        }
        if incoming.country_code_80211d.is_some() {
            existing.country_code_80211d = incoming.country_code_80211d;
        }
        existing.channel = incoming.channel.or(existing.channel);
        existing.frequency_mhz = incoming.frequency_mhz.or(existing.frequency_mhz);
        existing.band = incoming.band;
        if incoming.encryption_short != "Unknown" {
            existing.encryption_short = incoming.encryption_short;
        }
        if incoming.encryption_full != "Unknown" {
            existing.encryption_full = incoming.encryption_full;
        }
        if incoming.uptime_beacons.is_some() {
            existing.uptime_beacons = incoming.uptime_beacons;
        }
        existing.rssi_dbm = incoming.rssi_dbm.or(existing.rssi_dbm);
        existing.last_seen = incoming.last_seen;
        existing.first_seen = existing.first_seen.min(incoming.first_seen);
        existing.handshake_count = existing.handshake_count.max(incoming.handshake_count);
        existing.packet_mix.management = existing
            .packet_mix
            .management
            .max(incoming.packet_mix.management);
        existing.packet_mix.control = existing.packet_mix.control.max(incoming.packet_mix.control);
        existing.packet_mix.data = existing.packet_mix.data.max(incoming.packet_mix.data);
        existing.packet_mix.other = existing.packet_mix.other.max(incoming.packet_mix.other);
        existing.observations.extend(incoming.observations);
        existing.number_of_clients = incoming.number_of_clients;
    } else {
        aps.push(incoming);
    }
}

fn client_count_for_ap(clients: &[ClientRecord], ap_bssid: &str) -> u32 {
    clients
        .iter()
        .filter(|client| client_seen_on_ap(client, ap_bssid))
        .count() as u32
}

fn refresh_ap_client_count_for_bssid(
    access_points: &mut [AccessPointRecord],
    clients: &[ClientRecord],
    ap_bssid: &str,
) {
    if let Some(ap) = access_points.iter_mut().find(|ap| ap.bssid == ap_bssid) {
        ap.number_of_clients = client_count_for_ap(clients, ap_bssid);
    }
}

fn merge_client(clients: &mut Vec<ClientRecord>, incoming: ClientRecord) {
    if let Some(existing) = clients.iter_mut().find(|c| c.mac == incoming.mac) {
        if incoming.oui_manufacturer.is_some() {
            existing.oui_manufacturer = incoming.oui_manufacturer;
        }
        if incoming.associated_ap.is_some() {
            existing.associated_ap = incoming.associated_ap;
        }
        existing.data_transferred_bytes = existing
            .data_transferred_bytes
            .max(incoming.data_transferred_bytes);
        existing.rssi_dbm = incoming.rssi_dbm.or(existing.rssi_dbm);
        for p in incoming.probes {
            if !existing.probes.contains(&p) {
                existing.probes.push(p);
            }
        }
        existing.first_seen = existing.first_seen.min(incoming.first_seen);
        existing.last_seen = incoming.last_seen.max(existing.last_seen);
        for ap in incoming.seen_access_points {
            if !existing.seen_access_points.contains(&ap) {
                existing.seen_access_points.push(ap);
            }
        }
        for hs in incoming.handshake_networks {
            if !existing.handshake_networks.contains(&hs) {
                existing.handshake_networks.push(hs);
            }
        }
        merge_client_network_intel(existing, &incoming.network_intel);
        existing.observations.extend(incoming.observations);
    } else {
        clients.push(incoming);
    }
}

fn merge_client_network_intel(
    existing: &mut ClientRecord,
    incoming: &crate::model::ClientNetworkIntel,
) {
    existing.network_intel.uplink_bytes = existing
        .network_intel
        .uplink_bytes
        .max(incoming.uplink_bytes);
    existing.network_intel.downlink_bytes = existing
        .network_intel
        .downlink_bytes
        .max(incoming.downlink_bytes);
    existing.network_intel.packet_mix.management = existing
        .network_intel
        .packet_mix
        .management
        .max(incoming.packet_mix.management);
    existing.network_intel.packet_mix.control = existing
        .network_intel
        .packet_mix
        .control
        .max(incoming.packet_mix.control);
    existing.network_intel.packet_mix.data = existing
        .network_intel
        .packet_mix
        .data
        .max(incoming.packet_mix.data);
    existing.network_intel.packet_mix.other = existing
        .network_intel
        .packet_mix
        .other
        .max(incoming.packet_mix.other);
    existing.network_intel.retry_frame_count = existing
        .network_intel
        .retry_frame_count
        .max(incoming.retry_frame_count);
    existing.network_intel.power_save_observed |= incoming.power_save_observed;
    for priority in &incoming.qos_priorities {
        if !existing.network_intel.qos_priorities.contains(priority) {
            existing.network_intel.qos_priorities.push(*priority);
        }
    }
    existing.network_intel.qos_priorities.sort_unstable();
    existing.network_intel.eapol_frame_count = existing
        .network_intel
        .eapol_frame_count
        .max(incoming.eapol_frame_count);
    existing.network_intel.pmkid_count =
        existing.network_intel.pmkid_count.max(incoming.pmkid_count);
    existing.network_intel.last_frame_type = incoming
        .last_frame_type
        .or(existing.network_intel.last_frame_type);
    existing.network_intel.last_frame_subtype = incoming
        .last_frame_subtype
        .or(existing.network_intel.last_frame_subtype);
    existing.network_intel.last_channel = incoming
        .last_channel
        .or(existing.network_intel.last_channel);
    existing.network_intel.last_frequency_mhz = incoming
        .last_frequency_mhz
        .or(existing.network_intel.last_frequency_mhz);
    if incoming.band != SpectrumBand::Unknown {
        existing.network_intel.band = incoming.band.clone();
    }
    existing.network_intel.last_reason_code = incoming
        .last_reason_code
        .or(existing.network_intel.last_reason_code);
    existing.network_intel.last_status_code = incoming
        .last_status_code
        .or(existing.network_intel.last_status_code);
    existing.network_intel.listen_interval = incoming
        .listen_interval
        .or(existing.network_intel.listen_interval);
}

fn refresh_ap_client_counts_for_client(
    access_points: &mut [AccessPointRecord],
    clients: &[ClientRecord],
    client: &ClientRecord,
) -> Vec<String> {
    let mut related_bssids = client.seen_access_points.clone();
    if let Some(ap_bssid) = &client.associated_ap {
        if !related_bssids.iter().any(|seen| seen == ap_bssid) {
            related_bssids.push(ap_bssid.clone());
        }
    }

    let mut changed = Vec::new();
    for ap_bssid in related_bssids {
        if let Some(ap) = access_points.iter_mut().find(|ap| ap.bssid == ap_bssid) {
            let updated_count = client_count_for_ap(clients, &ap_bssid);
            if ap.number_of_clients != updated_count {
                ap.number_of_clients = updated_count;
                changed.push(ap_bssid);
            }
        }
    }
    changed
}

fn merge_bluetooth_device(
    devices: &mut Vec<BluetoothDeviceRecord>,
    incoming: BluetoothDeviceRecord,
) {
    if let Some(existing) = devices.iter_mut().find(|d| d.mac == incoming.mac) {
        if incoming.address_type.is_some() {
            existing.address_type = incoming.address_type;
        }
        if incoming.transport != "Unknown" {
            existing.transport = incoming.transport;
        }
        if incoming.oui_manufacturer.is_some() {
            existing.oui_manufacturer = incoming.oui_manufacturer;
        }
        if incoming.advertised_name.is_some() {
            existing.advertised_name = incoming.advertised_name;
        }
        if incoming.alias.is_some() {
            existing.alias = incoming.alias;
        }
        if incoming.device_type.is_some() {
            existing.device_type = incoming.device_type;
        }
        if incoming.class_of_device.is_some() {
            existing.class_of_device = incoming.class_of_device;
        }
        existing.rssi_dbm = incoming.rssi_dbm.or(existing.rssi_dbm);
        existing.first_seen = existing.first_seen.min(incoming.first_seen);
        existing.last_seen = existing.last_seen.max(incoming.last_seen);

        for id in incoming.mfgr_ids {
            if !existing.mfgr_ids.contains(&id) {
                existing.mfgr_ids.push(id);
            }
        }
        for name in incoming.mfgr_names {
            if !existing.mfgr_names.contains(&name) {
                existing.mfgr_names.push(name);
            }
        }
        for uuid in incoming.uuids {
            if !existing.uuids.contains(&uuid) {
                existing.uuids.push(uuid);
            }
        }
        for name in incoming.uuid_names {
            if !existing.uuid_names.contains(&name) {
                existing.uuid_names.push(name);
            }
        }
        if incoming.active_enumeration.is_some() {
            existing.active_enumeration = incoming.active_enumeration;
        }
        existing.observations.extend(incoming.observations);
    } else {
        devices.push(incoming);
    }
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn should_record_observation(
    map: &mut HashMap<String, chrono::DateTime<Utc>>,
    key: &str,
    now: chrono::DateTime<Utc>,
) -> bool {
    let min_interval = chrono::Duration::seconds(3);
    match map.get(key).copied() {
        Some(last) if now - last < min_interval => false,
        _ => {
            map.insert(key.to_string(), now);
            true
        }
    }
}

fn should_persist_device_update(
    map: &mut HashMap<String, chrono::DateTime<Utc>>,
    key: &str,
    now: chrono::DateTime<Utc>,
) -> bool {
    let min_interval = chrono::Duration::seconds(2);
    match map.get(key).copied() {
        Some(last) if now - last < min_interval => false,
        _ => {
            map.insert(key.to_string(), now);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_detail_text_excludes_ip_metadata_sections() {
        let now = Utc::now();
        let mut client = ClientRecord::new("AA:BB:CC:DD:EE:FF", now);
        client.oui_manufacturer = Some("Example Vendor".to_string());
        client.associated_ap = Some("11:22:33:44:55:66".to_string());
        client.rssi_dbm = Some(-52);
        client.probes = vec!["ExampleWiFi".to_string()];
        client.seen_access_points = vec!["11:22:33:44:55:66".to_string()];
        client.handshake_networks = vec!["11:22:33:44:55:66".to_string()];
        client.network_intel.uplink_bytes = 512;
        client.network_intel.downlink_bytes = 1024;
        client.network_intel.retry_frame_count = 2;
        client.network_intel.power_save_observed = true;
        client.network_intel.qos_priorities = vec![0, 5];
        client.network_intel.eapol_frame_count = 1;
        client.network_intel.pmkid_count = 1;
        client.network_intel.last_frame_type = Some(2);
        client.network_intel.last_frame_subtype = Some(8);
        client.network_intel.last_channel = Some(6);
        client.network_intel.last_frequency_mhz = Some(2437);
        client.network_intel.band = SpectrumBand::Ghz2_4;
        client.network_intel.last_reason_code = Some(7);
        client.network_intel.last_status_code = Some(0);
        client.network_intel.listen_interval = Some(10);
        client.network_intel.packet_mix.data = 12;

        let rendered = format_client_detail_text(&client, &[]);
        assert!(rendered.contains("Radio And Behavior"));
        assert!(rendered.contains("Security"));
        assert!(!rendered.contains("Open Network Metadata"));
        assert!(!rendered.contains("HTTP"));
        assert!(!rendered.contains("DNS"));
    }

    #[test]
    fn client_detail_signature_changes_when_packet_mix_changes() {
        let now = Utc::now();
        let mut client = ClientRecord::new("AA:BB:CC:DD:EE:FF", now);
        let before = client_detail_signature(&client);
        client.network_intel.packet_mix.data = 2;
        client.network_intel.retry_frame_count = 1;
        let after = client_detail_signature(&client);
        assert_ne!(before, after);
    }

    #[test]
    fn table_filter_columns_follow_visible_active_layout() {
        let mut layout = default_ap_table_layout();
        for column in &mut layout.columns {
            match column.id.as_str() {
                "ssid" => column.width_chars = 23,
                "bssid" => column.visible = false,
                _ => {}
            }
        }

        let columns = table_filter_columns(&layout, ap_column_label);
        assert!(columns
            .iter()
            .any(|(id, _, width)| id == "ssid" && *width == 23));
        assert!(!columns.iter().any(|(id, _, _)| id == "bssid"));
    }

    #[test]
    fn client_seen_on_ap_uses_current_association_only() {
        let now = Utc::now();
        let mut client = ClientRecord::new("AA:BB:CC:DD:EE:FF", now);
        client.associated_ap = Some("11:22:33:44:55:66".to_string());
        client.seen_access_points = vec!["77:88:99:AA:BB:CC".to_string()];

        assert!(client_seen_on_ap(&client, "11:22:33:44:55:66"));
        assert!(!client_seen_on_ap(&client, "77:88:99:AA:BB:CC"));
    }

    #[test]
    fn clients_currently_on_ap_excludes_historical_entries() {
        let now = Utc::now();
        let mut current = ClientRecord::new("AA:BB:CC:DD:EE:01", now);
        current.associated_ap = Some("11:22:33:44:55:66".to_string());

        let mut historical = ClientRecord::new("AA:BB:CC:DD:EE:02", now);
        historical.associated_ap = Some("77:88:99:AA:BB:CC".to_string());
        historical.seen_access_points = vec!["11:22:33:44:55:66".to_string()];

        let unassociated = ClientRecord::new("AA:BB:CC:DD:EE:03", now);
        let clients = vec![current.clone(), historical, unassociated];

        let filtered = clients_currently_on_ap(&clients, "11:22:33:44:55:66");
        let macs = filtered
            .iter()
            .map(|client| client.mac.as_str())
            .collect::<Vec<_>>();

        assert_eq!(macs, vec![current.mac.as_str()]);
    }

    #[test]
    fn assoc_clients_signature_ignores_non_current_clients() {
        let now = Utc::now();
        let mut current = ClientRecord::new("AA:BB:CC:DD:EE:01", now);
        current.associated_ap = Some("11:22:33:44:55:66".to_string());

        let mut other_ap = ClientRecord::new("AA:BB:CC:DD:EE:02", now);
        other_ap.associated_ap = Some("77:88:99:AA:BB:CC".to_string());

        let layout = default_assoc_client_table_layout();

        let sig = assoc_clients_signature(
            &[current, other_ap],
            "11:22:33:44:55:66",
            &[],
            &layout,
            &TableSortState::new("last_heard", true),
            1,
            50,
            &[],
            &WatchlistSettings::default(),
        );

        assert!(sig.starts_with("1|"));
    }

    #[test]
    fn row_matches_column_filters_supports_partial_case_insensitive_matching() {
        let filters = vec![
            ("ssid".to_string(), "homenet".to_string()),
            ("encryption".to_string(), "wPa2".to_string()),
        ];
        let values = std::collections::HashMap::from([
            ("ssid".to_string(), "HomeNetwork".to_string()),
            ("encryption".to_string(), "WPA2-PSK".to_string()),
        ]);

        assert!(row_matches_column_filters(&filters, |column| values
            .get(column)
            .cloned()));
    }

    #[test]
    fn row_matches_column_filters_requires_all_active_columns_to_match() {
        let filters = vec![
            ("ssid".to_string(), "home".to_string()),
            ("channel".to_string(), "11".to_string()),
        ];
        let values = std::collections::HashMap::from([
            ("ssid".to_string(), "HomeNetwork".to_string()),
            ("channel".to_string(), "6".to_string()),
        ]);

        assert!(!row_matches_column_filters(&filters, |column| values
            .get(column)
            .cloned()));
    }

    #[test]
    fn runtime_output_root_uses_effective_uid_namespace() {
        let root = internal_runtime_output_root();
        let uid = unsafe { libc::geteuid() };
        let marker = format!("wirelessexplorer-runtime-uid{}", uid);
        assert!(
            root.to_string_lossy().contains(&marker),
            "expected runtime output root to include `{}` but got `{}`",
            marker,
            root.display()
        );
    }
}

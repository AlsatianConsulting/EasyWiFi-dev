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
use crate::settings::{
    default_ap_table_layout, default_assoc_client_table_layout, default_bluetooth_table_layout,
    default_client_table_layout, default_hop_ht_mode, settings_file_path, AppSettings,
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
    ProgressBar, ResponseType, ScrolledWindow, SpinButton, Stack, StackSidebar, TextView, Viewport,
    ToggleButton, Window as GtkWindow,
};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
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
        .application_id("com.easywifi.app")
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
const UI_POLL_INTERVAL_MS: u64 = 120;
const MAX_CAPTURE_EVENTS_PER_TICK: usize = 1200;
const MAX_BLUETOOTH_EVENTS_PER_TICK: usize = 200;
const MAX_WIFI_GEIGER_UPDATES_PER_TICK: usize = 8;
const MIN_LIST_REFRESH_INTERVAL_MS: u64 = 140;
const WIFI_WATCHDOG_STALL_TIMEOUT_SECS: u64 = 20;
const BLUETOOTH_WATCHDOG_STALL_TIMEOUT_SECS: u64 = 20;
const WATCHDOG_RESTART_GRACE_SECS: u64 = 12;
const WATCHDOG_MAX_CONSECUTIVE_RESTARTS: u8 = 3;
const TABLE_CHAR_WIDTH_PX: i32 = 10;
const AP_TABLE_MIN_WIDTH_PX: i32 = 1200;
const CLIENT_TABLE_MIN_WIDTH_PX: i32 = 1200;
const BLUETOOTH_TABLE_MIN_WIDTH_PX: i32 = 1200;
const DEFAULT_TABLE_PAGE_SIZE: usize = 50;
const TABLE_PAGE_SIZE_OPTIONS: &[usize] = &[25, 50, 100, 200];
const DEFAULT_WINDOW_WIDTH: i32 = 720;
const DEFAULT_WINDOW_HEIGHT: i32 = 720;
const MIN_WINDOW_WIDTH: i32 = 720;
const MIN_WINDOW_HEIGHT: i32 = 720;
const DEFAULT_CONTENT_PANE_POSITION: i32 = 420;
const DEFAULT_AP_ROOT_POSITION: i32 = 240;
const DEFAULT_AP_SUMMARY_ROW_POSITION: i32 = 320;
const DEFAULT_AP_DETAIL_SECTIONS_POSITION: i32 = 260;
const DEFAULT_AP_BOTTOM_POSITION: i32 = 500;
const DEFAULT_CLIENT_ROOT_POSITION: i32 = 240;
const DEFAULT_BLUETOOTH_BOTTOM_POSITION: i32 = 300;
const DEFAULT_BLUETOOTH_ROOT_POSITION: i32 = 240;
const DEFAULT_CHANNEL_ROOT_POSITION: i32 = 240;
const UI_BUILD_MARKER: &str = "SCROLLFIX-2026-04-02-B";

fn is_small_display() -> bool {
    let model = std::fs::read_to_string("/proc/device-tree/model")
        .unwrap_or_default()
        .to_ascii_lowercase();
    model.contains("raspberry pi")
}

fn effective_min_window_size() -> (i32, i32) {
    if is_small_display() {
        (520, 520)
    } else {
        (MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT)
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InterfaceHealthState {
    Idle,
    Active,
    Stalled,
    Restarting,
    Error,
}

#[derive(Debug, Clone)]
struct BluetoothEnumerationStatus {
    message: String,
    is_error: bool,
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
    bluetooth_enumeration_status: HashMap<String, BluetoothEnumerationStatus>,
    wifi_health_state: InterfaceHealthState,
    wifi_health_detail: String,
    wifi_last_data_at: Option<Instant>,
    wifi_restart_count: u32,
    wifi_consecutive_watchdog_restarts: u8,
    wifi_watchdog_block_until: Option<Instant>,
    bluetooth_health_state: InterfaceHealthState,
    bluetooth_health_detail: String,
    bluetooth_last_data_at: Option<Instant>,
    bluetooth_restart_count: u32,
    bluetooth_consecutive_watchdog_restarts: u8,
    bluetooth_watchdog_block_until: Option<Instant>,
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

    fn set_bluetooth_enumeration_status(
        &mut self,
        mac: impl Into<String>,
        message: impl Into<String>,
        is_error: bool,
    ) {
        self.bluetooth_enumeration_status.insert(
            mac.into(),
            BluetoothEnumerationStatus {
                message: message.into(),
                is_error,
            },
        );
    }

    fn set_wifi_health_state(&mut self, state: InterfaceHealthState, detail: impl Into<String>) {
        self.wifi_health_state = state;
        self.wifi_health_detail = detail.into();
    }

    fn set_bluetooth_health_state(
        &mut self,
        state: InterfaceHealthState,
        detail: impl Into<String>,
    ) {
        self.bluetooth_health_state = state;
        self.bluetooth_health_detail = detail.into();
    }

    fn note_wifi_activity(&mut self, detail: impl Into<String>) {
        self.wifi_last_data_at = Some(Instant::now());
        self.wifi_consecutive_watchdog_restarts = 0;
        self.wifi_watchdog_block_until = None;
        self.set_wifi_health_state(InterfaceHealthState::Active, detail);
    }

    fn note_bluetooth_activity(&mut self, detail: impl Into<String>) {
        self.bluetooth_last_data_at = Some(Instant::now());
        self.bluetooth_consecutive_watchdog_restarts = 0;
        self.bluetooth_watchdog_block_until = None;
        self.set_bluetooth_health_state(InterfaceHealthState::Active, detail);
    }

    fn interface_runtime_status_text(&self) -> String {
        let wifi_iface = self
            .settings
            .interfaces
            .iter()
            .find(|iface| iface.enabled)
            .map(|iface| {
                iface
                    .monitor_interface_name
                    .clone()
                    .unwrap_or_else(|| iface.interface_name.clone())
            });
        let bluetooth_controller = self
            .settings
            .bluetooth_controller
            .clone()
            .unwrap_or_else(|| "default".to_string());

        let wifi_line = if !self.settings.interfaces.iter().any(|iface| iface.enabled) {
            "Wi-Fi: disabled".to_string()
        } else {
            format!(
                "Wi-Fi {} | {} | {} | last data: {} | restarts: {}",
                wifi_iface.unwrap_or_else(|| "<unknown>".to_string()),
                interface_health_state_label(self.wifi_health_state),
                self.wifi_health_detail,
                format_health_elapsed(self.wifi_last_data_at),
                self.wifi_restart_count
            )
        };

        let bluetooth_line = if !self.settings.bluetooth_enabled {
            "Bluetooth: disabled".to_string()
        } else {
            format!(
                "Bluetooth {} | {} | {} | last data: {} | restarts: {}",
                bluetooth_controller,
                interface_health_state_label(self.bluetooth_health_state),
                self.bluetooth_health_detail,
                format_health_elapsed(self.bluetooth_last_data_at),
                self.bluetooth_restart_count
            )
        };

        format!(
            "{wifi_line}\n{}\n{bluetooth_line}\n{}",
            self.wifi_adapter_activity_summary(),
            self.bluetooth_adapter_activity_summary()
        )
    }

    fn wifi_adapter_activity_summary(&self) -> String {
        let mut ap_counts: HashMap<String, usize> = HashMap::new();
        let mut client_counts: HashMap<String, usize> = HashMap::new();

        for ap in &self.access_points {
            let adapters = if ap.source_adapters.is_empty() {
                vec!["unknown".to_string()]
            } else {
                ap.source_adapters.clone()
            };
            let unique = adapters.into_iter().collect::<HashSet<_>>();
            for adapter in unique {
                *ap_counts.entry(adapter).or_insert(0) += 1;
            }
        }

        for client in &self.clients {
            let adapters = if client.source_adapters.is_empty() {
                vec!["unknown".to_string()]
            } else {
                client.source_adapters.clone()
            };
            let unique = adapters.into_iter().collect::<HashSet<_>>();
            for adapter in unique {
                *client_counts.entry(adapter).or_insert(0) += 1;
            }
        }

        let mut adapters = ap_counts
            .keys()
            .chain(client_counts.keys())
            .cloned()
            .collect::<Vec<_>>();
        adapters.sort();
        adapters.dedup();
        if adapters.is_empty() {
            return "Wi-Fi adapters: no AP/client data yet".to_string();
        }

        let parts = adapters
            .into_iter()
            .map(|adapter| {
                format!(
                    "{} aps={} clients={}",
                    adapter,
                    ap_counts.get(&adapter).copied().unwrap_or(0),
                    client_counts.get(&adapter).copied().unwrap_or(0)
                )
            })
            .collect::<Vec<_>>();
        format!("Wi-Fi adapters: {}", parts.join(" | "))
    }

    fn bluetooth_adapter_activity_summary(&self) -> String {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for device in &self.bluetooth_devices {
            let adapters = if device.source_adapters.is_empty() {
                vec!["unknown".to_string()]
            } else {
                device.source_adapters.clone()
            };
            let unique = adapters.into_iter().collect::<HashSet<_>>();
            for adapter in unique {
                *counts.entry(adapter).or_insert(0) += 1;
            }
        }

        if counts.is_empty() {
            return "Bluetooth adapters: no device data yet".to_string();
        }

        let mut adapters = counts.keys().cloned().collect::<Vec<_>>();
        adapters.sort();
        let parts = adapters
            .into_iter()
            .map(|adapter| format!("{} devices={}", adapter, counts[&adapter]))
            .collect::<Vec<_>>();
        format!("Bluetooth adapters: {}", parts.join(" | "))
    }

    fn maybe_run_scan_watchdog(&mut self) -> bool {
        if self.scan_start_in_progress || self.scan_stop_in_progress {
            return false;
        }

        let now = Instant::now();
        let mut changed = false;

        if self.capture_runtime.is_some()
            && self.settings.interfaces.iter().any(|iface| iface.enabled)
        {
            if self
                .wifi_watchdog_block_until
                .map(|until| now >= until)
                .unwrap_or(false)
            {
                self.wifi_watchdog_block_until = None;
            }

            let stalled = self
                .wifi_last_data_at
                .map(|last| {
                    now.saturating_duration_since(last).as_secs()
                        >= WIFI_WATCHDOG_STALL_TIMEOUT_SECS
                })
                .unwrap_or(false);

            if stalled && self.wifi_watchdog_block_until.is_none() {
                let was_error = self.wifi_health_state == InterfaceHealthState::Error;
                self.set_wifi_health_state(
                    InterfaceHealthState::Stalled,
                    format!("no Wi-Fi frames for {}s", WIFI_WATCHDOG_STALL_TIMEOUT_SECS),
                );

                if self.wifi_consecutive_watchdog_restarts >= WATCHDOG_MAX_CONSECUTIVE_RESTARTS {
                    if !was_error {
                        self.push_status(
                            "Wi-Fi watchdog reached restart limit; manual restart required"
                                .to_string(),
                        );
                    }
                    self.set_wifi_health_state(
                        InterfaceHealthState::Error,
                        "restart limit reached; waiting for manual action".to_string(),
                    );
                    self.wifi_watchdog_block_until =
                        Some(now + Duration::from_secs(WATCHDOG_RESTART_GRACE_SECS));
                    changed = true;
                } else {
                    self.wifi_consecutive_watchdog_restarts =
                        self.wifi_consecutive_watchdog_restarts.saturating_add(1);
                    self.wifi_restart_count = self.wifi_restart_count.saturating_add(1);
                    self.wifi_watchdog_block_until =
                        Some(now + Duration::from_secs(WATCHDOG_RESTART_GRACE_SECS));
                    self.wifi_last_data_at = Some(now);
                    self.set_wifi_health_state(
                        InterfaceHealthState::Restarting,
                        "watchdog restarting Wi-Fi capture".to_string(),
                    );
                    self.push_status(
                        "Wi-Fi watchdog: stalled capture detected; restarting Wi-Fi scan"
                            .to_string(),
                    );
                    self.restart_wifi_scan();
                    return true;
                }
            }
        }

        if self.bluetooth_runtime.is_some() && self.settings.bluetooth_enabled {
            if self
                .bluetooth_watchdog_block_until
                .map(|until| now >= until)
                .unwrap_or(false)
            {
                self.bluetooth_watchdog_block_until = None;
            }

            let stalled = self
                .bluetooth_last_data_at
                .map(|last| {
                    now.saturating_duration_since(last).as_secs()
                        >= BLUETOOTH_WATCHDOG_STALL_TIMEOUT_SECS
                })
                .unwrap_or(false);

            if stalled && self.bluetooth_watchdog_block_until.is_none() {
                let was_error = self.bluetooth_health_state == InterfaceHealthState::Error;
                self.set_bluetooth_health_state(
                    InterfaceHealthState::Stalled,
                    format!(
                        "no Bluetooth devices for {}s",
                        BLUETOOTH_WATCHDOG_STALL_TIMEOUT_SECS
                    ),
                );

                if self.bluetooth_consecutive_watchdog_restarts >= WATCHDOG_MAX_CONSECUTIVE_RESTARTS
                {
                    if !was_error {
                        self.push_status(
                            "Bluetooth watchdog reached restart limit; manual restart required"
                                .to_string(),
                        );
                    }
                    self.set_bluetooth_health_state(
                        InterfaceHealthState::Error,
                        "restart limit reached; waiting for manual action".to_string(),
                    );
                    self.bluetooth_watchdog_block_until =
                        Some(now + Duration::from_secs(WATCHDOG_RESTART_GRACE_SECS));
                    changed = true;
                } else {
                    self.bluetooth_consecutive_watchdog_restarts = self
                        .bluetooth_consecutive_watchdog_restarts
                        .saturating_add(1);
                    self.bluetooth_restart_count = self.bluetooth_restart_count.saturating_add(1);
                    self.bluetooth_watchdog_block_until =
                        Some(now + Duration::from_secs(WATCHDOG_RESTART_GRACE_SECS));
                    self.bluetooth_last_data_at = Some(now);
                    self.set_bluetooth_health_state(
                        InterfaceHealthState::Restarting,
                        "watchdog restarting Bluetooth scan".to_string(),
                    );
                    self.push_status(
                        "Bluetooth watchdog: stalled scan detected; restarting Bluetooth scan"
                            .to_string(),
                    );
                    self.restart_bluetooth_scan();
                    return true;
                }
            }
        }

        changed
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

    fn status_header_line(&self) -> String {
        if self.scan_start_in_progress {
            "starting scans...".to_string()
        } else if self.scan_stop_in_progress {
            "stopping scans...".to_string()
        } else {
            match (
                self.capture_runtime.is_some(),
                self.bluetooth_runtime.is_some(),
            ) {
                (true, true) => "scanning active (Wi-Fi + Bluetooth)".to_string(),
                (true, false) => "scanning active (Wi-Fi only)".to_string(),
                (false, true) => "scanning active (Bluetooth only)".to_string(),
                (false, false) => "scanning idle (click Start)".to_string(),
            }
        }
    }

    fn status_text(&self) -> String {
        let mut lines = Vec::with_capacity(self.status_lines.len() + 1);
        lines.push(self.status_header_line());
        lines.extend(self.status_lines.iter().cloned());
        lines.join("\n")
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
            "GPS {} | {} | Last Fix: {} | {}",
            status.mode, state, last_fix, status.detail
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

        let sqlite_path = exporter.paths.session_dir.join("easywifi.sqlite");
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

    fn export_session_snapshots(&mut self) {
        let session_log_path = self.exporter.paths.logs_dir.join("session.log");
        let append_session_log = |line: &str| {
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&session_log_path)
            {
                let _ = writeln!(file, "{line}");
            }
        };

        append_session_log(&format!(
            "snapshot export start ap={} clients={} bt={}",
            self.access_points.len(),
            self.clients.len(),
            self.bluetooth_devices.len()
        ));
        self.push_status(format!(
            "exporting session snapshots: ap={} clients={} bt={}",
            self.access_points.len(),
            self.clients.len(),
            self.bluetooth_devices.len()
        ));
        let mut failures = Vec::new();

        if let Err(err) = self.exporter.export_access_points_csv(&self.access_points) {
            failures.push(format!("ap csv: {err}"));
        }
        if let Err(err) = self.exporter.export_clients_csv(&self.clients) {
            failures.push(format!("client csv: {err}"));
        }
        if let Err(err) = self.exporter.export_location_logs_csv(
            &self.access_points,
            &self.clients,
            &self.bluetooth_devices,
        ) {
            failures.push(format!("location csv: {err}"));
        }
        if let Err(err) = self.exporter.export_location_logs_kml(
            &self.access_points,
            &self.clients,
            &self.bluetooth_devices,
        ) {
            failures.push(format!("location kml: {err}"));
        }
        if let Err(err) = self.exporter.export_location_logs_kmz(
            &self.access_points,
            &self.clients,
            &self.bluetooth_devices,
        ) {
            failures.push(format!("location kmz: {err}"));
        }
        if let Err(err) = self.exporter.export_summary_json(
            &self.access_points,
            &self.clients,
            &self.bluetooth_devices,
        ) {
            failures.push(format!("summary json: {err}"));
        }

        if failures.is_empty() {
            append_session_log("snapshot export complete");
            self.push_status("session artifacts refreshed (CSV/KML/KMZ/summary)".to_string());
        } else {
            append_session_log(&format!(
                "snapshot export incomplete: {}",
                failures.join(" | ")
            ));
            self.push_status(format!(
                "session artifact refresh incomplete: {}",
                failures.join(" | ")
            ));
        }
    }

    fn apply_capture_event(&mut self, event: CaptureEvent) -> Result<UiRefreshHint> {
        match event {
            CaptureEvent::AccessPointSeen(mut ap) => {
                self.note_wifi_activity("capturing Wi-Fi packets");
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
                self.note_wifi_activity("capturing Wi-Fi packets");
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
                self.note_wifi_activity("capturing Wi-Fi packets");
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
                    &self.gps_track,
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
                self.note_wifi_activity("capturing Wi-Fi packets");
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
                self.note_bluetooth_activity("scanning Bluetooth advertisements");
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
                if let Some(status) = bluetooth_enumeration_status_from_device(&device) {
                    self.set_bluetooth_enumeration_status(
                        device.mac.clone(),
                        status.message,
                        status.is_error,
                    );
                }
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
            BluetoothEvent::EnumerationStatus {
                mac,
                message,
                is_error,
            } => {
                self.set_bluetooth_enumeration_status(mac, message, is_error);
                Ok(UiRefreshHint {
                    ap_list: false,
                    client_list: false,
                    bluetooth_list: false,
                    channel_chart: false,
                    status: false,
                })
            }
            BluetoothEvent::Log(text) => {
                if text.contains("scan failed") {
                    self.set_bluetooth_health_state(InterfaceHealthState::Error, text.clone());
                }
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
        let fix = self.gps_provider.current_fix()?;
        Some(GeoObservation {
            timestamp: Utc::now(),
            latitude: fix.latitude,
            longitude: fix.longitude,
            altitude_m: fix.altitude_m,
            rssi_dbm,
        })
    }

    fn maybe_record_gps_track_point(&mut self) {
        let now = Utc::now();
        if let Some(last) = self.last_gps_track_point_at {
            if now - last < chrono::Duration::seconds(1) {
                return;
            }
        }

        let Some(fix) = self.gps_provider.current_fix() else {
            return;
        };

        if fix.latitude.abs() > 90.0 || fix.longitude.abs() > 180.0 {
            return;
        }

        let point = GeoObservation {
            timestamp: now,
            latitude: fix.latitude,
            longitude: fix.longitude,
            altitude_m: fix.altitude_m,
            rssi_dbm: None,
        };
        self.gps_track.push(point.clone());
        self.last_gps_track_point_at = Some(now);
        self.enqueue_persistence(PersistenceCommand::AddGpsTrackPoint(point));
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

        let wifi_enabled = self.settings.interfaces.iter().any(|iface| iface.enabled);
        let need_wifi = wifi_enabled && self.capture_runtime.is_none();
        let need_bluetooth = self.settings.bluetooth_enabled && self.bluetooth_runtime.is_none();
        if !need_wifi && !need_bluetooth {
            if self.capture_runtime.is_some() || self.bluetooth_runtime.is_some() {
                self.push_status("scanning already running".to_string());
            } else {
                self.push_status(
                    "no scanning services enabled; enable Wi-Fi and/or Bluetooth".to_string(),
                );
            }
            return;
        }

        let (tx, rx) = unbounded::<StartCompletion>();
        self.pending_start_completion = Some(rx);
        self.scan_start_in_progress = true;
        if need_wifi {
            self.set_wifi_health_state(
                InterfaceHealthState::Restarting,
                "starting Wi-Fi capture".to_string(),
            );
            self.wifi_last_data_at = Some(Instant::now());
        }
        if need_bluetooth {
            self.set_bluetooth_health_state(
                InterfaceHealthState::Restarting,
                "starting Bluetooth scan".to_string(),
            );
            self.bluetooth_last_data_at = Some(Instant::now());
        }
        self.push_status("starting scans...".to_string());

        let interfaces = self.settings.interfaces.clone();
        let session_capture_path = self.session_capture_path.clone();
        let geoip_city_db_path = self.settings.geoip_city_db_path.clone();
        let wifi_packet_header_mode = self.settings.wifi_packet_header_mode;
        let wifi_frame_parsing_enabled = self.settings.enable_wifi_frame_parsing;
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
                    geoip_city_db_path,
                    wifi_packet_header_mode,
                    wifi_frame_parsing_enabled,
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
            self.set_bluetooth_health_state(
                InterfaceHealthState::Idle,
                "Bluetooth scanning disabled".to_string(),
            );
            return;
        }

        self.set_bluetooth_health_state(
            InterfaceHealthState::Restarting,
            "restarting Bluetooth scan".to_string(),
        );
        self.bluetooth_last_data_at = Some(Instant::now());

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

    fn restart_wifi_scan(&mut self) {
        if let Some(runtime) = self.capture_runtime.take() {
            runtime.stop();
        }
        if !self.settings.interfaces.iter().any(|iface| iface.enabled) {
            self.set_wifi_health_state(
                InterfaceHealthState::Idle,
                "Wi-Fi scanning disabled".to_string(),
            );
            return;
        }

        self.set_wifi_health_state(
            InterfaceHealthState::Restarting,
            "restarting Wi-Fi capture".to_string(),
        );
        self.wifi_last_data_at = Some(Instant::now());
        self.start_scanning();
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
    let live_root = std::env::temp_dir().join("easywifi-live");
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
    if let Some(base) = dirs::data_local_dir() {
        return base.join("EasyWiFi").join("runtime");
    }
    std::env::temp_dir().join(format!("easywifi-runtime-uid{}", unsafe {
        libc::geteuid()
    }))
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

#[derive(Clone)]
struct UiWidgets {
    ap_root: Paned,
    ap_bottom: Paned,
    ap_detail_notebook: Notebook,
    ap_assoc_box: GtkBox,
    ap_header_holder: GtkBox,
    ap_header_scrolled: ScrolledWindow,
    ap_scrolled: ScrolledWindow,
    ap_list_canvas: GtkBox,
    ap_list: ListBox,
    ap_pagination: TablePaginationUi,
    ap_scroll_adj: gtk::Adjustment,
    ap_selection_suppressed: Rc<RefCell<bool>>,
    ap_selected_key: Rc<RefCell<Option<String>>>,
    ap_detail_label: Label,
    ap_detail_scroll: ScrolledWindow,
    ap_notes_view: TextView,
    ap_assoc_header_holder: GtkBox,
    ap_assoc_list: ListBox,
    ap_assoc_pagination: TablePaginationUi,
    ap_packet_draw: DrawingArea,
    ap_selected_packet_mix: Rc<RefCell<PacketTypeBreakdown>>,
    ap_scroll_debug_label: Label,
    client_header_holder: GtkBox,
    client_header_scrolled: ScrolledWindow,
    client_scrolled: ScrolledWindow,
    client_list: ListBox,
    client_pagination: TablePaginationUi,
    client_scroll_adj: gtk::Adjustment,
    client_scroll_debug_label: Label,
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
    bluetooth_header_scrolled: ScrolledWindow,
    bluetooth_scrolled: ScrolledWindow,
    bluetooth_pagination: TablePaginationUi,
    bluetooth_scroll_adj: gtk::Adjustment,
    bluetooth_scroll_debug_label: Label,
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
    bluetooth_enumeration_status_label: Label,
    bluetooth_root: Paned,
    bluetooth_bottom: Paned,
    bluetooth_geiger_rssi: Label,
    bluetooth_geiger_tone: Label,
    bluetooth_geiger_progress: ProgressBar,
    bluetooth_geiger_state: Rc<RefCell<BluetoothGeigerUiState>>,
    channel_draw: DrawingArea,
    status_label: Label,
    interface_health_label: Label,
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
        let entry_width = (*width_chars).max(8).min(24);
        entry.set_width_chars(entry_width);
        entry.set_max_width_chars(entry_width);
        entry.set_size_request(entry_width * TABLE_CHAR_WIDTH_PX, -1);
        entry.set_margin_end(6);
        entry.set_placeholder_text(Some(column_label));
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
        .title(format!("EasyWiFi [{}]", UI_BUILD_MARKER))
        .default_width(DEFAULT_WINDOW_WIDTH)
        .default_height(DEFAULT_WINDOW_HEIGHT)
        .build();
    let (min_window_width, min_window_height) = effective_min_window_size();
    window.set_size_request(min_window_width, min_window_height);
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
        Err(_err) if !settings_path.exists() => (AppSettings::default(), None),
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
    settings.window_width = settings.window_width.max(MIN_WINDOW_WIDTH);
    settings.window_height = settings.window_height.max(MIN_WINDOW_HEIGHT);
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
    apply_dark_mode_preference(settings.dark_mode);
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
                ht_mode: default_hop_ht_mode(),
                channel_ht_modes: BTreeMap::new(),
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

    let sqlite_path = exporter.paths.session_dir.join("easywifi.sqlite");
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
    let session_capture_path = prepare_live_capture_path(&session_id)?;

    let runtime: Option<CaptureRuntime> = None;
    let bluetooth_runtime: Option<BluetoothRuntime> = None;

    let initial_gps_track_points = existing_gps_track.len();
    let bluetooth_controller_status = settings
        .bluetooth_controller
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
        bluetooth_enumeration_status: HashMap::new(),
        wifi_health_state: InterfaceHealthState::Idle,
        wifi_health_detail: "Wi-Fi scanning idle".to_string(),
        wifi_last_data_at: None,
        wifi_restart_count: 0,
        wifi_consecutive_watchdog_restarts: 0,
        wifi_watchdog_block_until: None,
        bluetooth_health_state: InterfaceHealthState::Idle,
        bluetooth_health_detail: "Bluetooth scanning idle".to_string(),
        bluetooth_last_data_at: None,
        bluetooth_restart_count: 0,
        bluetooth_consecutive_watchdog_restarts: 0,
        bluetooth_watchdog_block_until: None,
        session_capture_path,
        gps_track: existing_gps_track,
        last_gps_track_point_at: None,
        status_lines: {
            let mut lines = Vec::new();
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
            lines.push(format!(
                "loaded GPS track points: {}",
                initial_gps_track_points
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
    window.set_default_size(
        state.borrow().settings.window_width,
        state.borrow().settings.window_height,
    );

    let global_status_label = Label::new(Some(&format!("starting [{}]", UI_BUILD_MARKER)));
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
    content_paned.set_shrink_start_child(true);
    content_paned.set_shrink_end_child(true);
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
    let content_scrolled = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&content_paned)
        .build();
    root.append(&content_scrolled);

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
    {
        let s = state.borrow_mut();
        if is_small_display() {
            window.unfullscreen();
            window.unmaximize();
            window.set_default_size(680, 680);
            window.set_resizable(true);
        } else if s.settings.window_fullscreen {
            window.fullscreen();
        } else if s.settings.window_maximized {
            window.maximize();
        }
    }
    {
        let state = state.clone();
        window.connect_close_request(move |w| {
            let mut s = state.borrow_mut();
            let (min_width, min_height) = effective_min_window_size();
            s.settings.window_width = w.width().max(min_width);
            s.settings.window_height = w.height().max(min_height);
            s.settings.window_maximized = w.is_maximized();
            s.settings.window_fullscreen = w.is_fullscreen();
            s.save_settings_to_disk();
            glib::Propagation::Proceed
        });
    }
    window.present();

    bind_poll_loop(
        capture_rx,
        bluetooth_rx,
        state.clone(),
        widgets,
        capture_start_btn,
        capture_stop_btn,
        global_status_label,
        global_gps_status_label,
        notebook.clone(),
        &window,
    );

    if std::env::var_os("SIMPLESTG_AUTOSTART").is_some() {
        state.borrow_mut().start_scanning();
    }

    if let Some(value) = std::env::var_os("SIMPLESTG_AUTOSTOP_AFTER_SECS") {
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
            let _ = s.exporter.export_access_points_csv(&s.access_points);
            let _ = s.exporter.export_clients_csv(&s.clients);
            let gps_pcap = s
                .exporter
                .export_session_pcap_with_gps(&s.session_capture_path, &s.gps_track);
            match gps_pcap {
                Ok(_) => {
                    s.push_status("exported AP/client CSV and consolidated GPS PCAPNG".to_string())
                }
                Err(err) => s.push_status(format!(
                    "exported CSV; consolidated GPS PCAPNG failed: {err}"
                )),
            }
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
            match (csv, kml) {
                (Ok(_), Ok(_)) => s.push_status("exported location logs (CSV + KML)".to_string()),
                (csv_res, kml_res) => s.push_status(format!(
                    "location export incomplete: csv={} kml={}",
                    csv_res
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "ok".to_string()),
                    kml_res
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

    let dark_mode_initial = state.borrow().settings.dark_mode;
    let settings_dark_mode_action = gio::SimpleAction::new_stateful(
        "settings_dark_mode",
        None,
        &glib::Variant::from(dark_mode_initial),
    );
    {
        let state = state.clone();
        settings_dark_mode_action.connect_activate(move |action, _| {
            let current = action
                .state()
                .and_then(|variant| variant.get::<bool>())
                .unwrap_or(false);
            let next = !current;
            action.set_state(&glib::Variant::from(next));
            apply_dark_mode_preference(next);
            let mut s = state.borrow_mut();
            if s.settings.dark_mode != next {
                s.settings.dark_mode = next;
                s.save_settings_to_disk();
            }
            s.push_status(format!(
                "dark mode {}",
                if next { "enabled" } else { "disabled" }
            ));
        });
    }
    app.add_action(&settings_dark_mode_action);

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
        Some("Export CSV + Consolidated PCAP"),
        Some("app.export_all"),
    );
    file_menu.append(
        Some("Export Location Logs (CSV + KML)"),
        Some("app.export_locations"),
    );
    file_menu.append(Some("Update OUI Database"), Some("app.update_oui"));
    file_menu.append(Some("Quit"), Some("app.quit_app"));

    let view_menu = gio::Menu::new();
    view_menu.append(Some("Device Pane"), Some("app.settings_show_device_pane"));
    view_menu.append(Some("Details Pane"), Some("app.settings_show_detail_pane"));
    view_menu.append(Some("Status Pane"), Some("app.settings_show_status_bar"));
    view_menu.append(Some("Dark Mode"), Some("app.settings_dark_mode"));

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
        ChannelSelectionMode::HopAll {
            channels,
            dwell_ms,
            ht_mode,
            channel_ht_modes,
        } => {
            let mixed = channel_ht_modes
                .values()
                .collect::<HashSet<_>>()
                .len()
                > 1;
            format!(
                "Hop Specific [{} channels @ {} ms, {}{}]",
                channels.len(),
                dwell_ms,
                ht_mode,
                if mixed { ", mixed" } else { "" }
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
    let small = is_small_display();
    content_paned.set_position(if small { 400 } else { DEFAULT_CONTENT_PANE_POSITION });
    status_container.set_visible(settings.show_status_bar);

    let show_ap_bottom = settings.show_detail_pane || settings.show_device_pane;
    widgets
        .ap_root
        .set_position(if small { 220 } else { DEFAULT_AP_ROOT_POSITION });
    widgets.ap_root.set_resize_end_child(show_ap_bottom);
    widgets.ap_bottom.set_visible(show_ap_bottom);
    widgets
        .ap_detail_notebook
        .set_visible(settings.show_detail_pane);
    widgets.ap_assoc_box.set_visible(settings.show_device_pane);
    widgets
        .ap_bottom
        .set_position(if small { 340 } else { DEFAULT_AP_BOTTOM_POSITION });

    widgets
        .client_root
        .set_position(if small { 220 } else { DEFAULT_CLIENT_ROOT_POSITION });
    widgets
        .client_root
        .set_resize_end_child(settings.show_detail_pane);
    widgets
        .client_detail_notebook
        .set_visible(settings.show_detail_pane);

    widgets
        .bluetooth_root
        .set_position(if small { 220 } else { DEFAULT_BLUETOOTH_ROOT_POSITION });
    widgets
        .bluetooth_root
        .set_resize_end_child(settings.show_detail_pane);
    widgets
        .bluetooth_bottom
        .set_visible(settings.show_detail_pane);
    widgets
        .bluetooth_bottom
        .set_position(if small { 280 } else { DEFAULT_BLUETOOTH_BOTTOM_POSITION });
}

fn apply_dark_mode_preference(enabled: bool) {
    if let Some(gtk_settings) = gtk::Settings::default() {
        gtk_settings.set_gtk_application_prefer_dark_theme(enabled);
    }
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
    geoip_city_db_path: PathBuf,
    wifi_packet_header_mode: WifiPacketHeaderMode,
    wifi_frame_parsing_enabled: bool,
    gps_enabled: bool,
    capture_sender: Sender<CaptureEvent>,
) -> WifiStartResult {
    let mut status_lines = Vec::new();
    let mut privilege_alert = None;
    let mut wifi_interface_restore_types = HashMap::new();

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
            wifi_frame_parsing_enabled,
            geoip_city_db_path: Some(geoip_city_db_path),
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
            "EasyWiFi could not prepare {} for Wi-Fi capture.\n\n{}\n\nWi-Fi capture was not started.\n\nEasyWiFi is already running as root, so no additional privilege prompt was used. Verify the interface name, driver support, and that `ip`/`iw` succeeded under this root session.",
            interface, error_text
        )
    } else {
        format!(
            "EasyWiFi could not prepare {} for Wi-Fi capture.\n\n{}\n\nWi-Fi capture was not started.\n\nRun the GUI as your normal user. One of these privilege paths must work:\n1. `pkexec` with a working polkit agent\n2. passwordless `sudo -n` for `{}`\n3. helper capabilities:\n   sudo setcap cap_net_admin,cap_net_raw=eip {}",
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
        "EasyWiFi could not start Wi-Fi capture because a root-level Wi-Fi command failed."
    } else {
        "EasyWiFi could not start Wi-Fi capture because privilege escalation failed."
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
    ap_list.set_activate_on_single_click(false);
    attach_listbox_click_selection(&ap_list);
    let ap_list_canvas = GtkBox::new(Orientation::Horizontal, 0);
    ap_list_canvas.set_hexpand(false);
    ap_list_canvas.set_halign(gtk::Align::Start);
    ap_list_canvas.append(&ap_list);
    let ap_scroll_adj = gtk::Adjustment::new(0.0, 0.0, 0.0, 24.0, 160.0, 680.0);
    let ap_viewport = Viewport::new(Some(&ap_scroll_adj), None::<&gtk::Adjustment>);
    ap_viewport.set_child(Some(&ap_list_canvas));
    let ap_scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(false)
        .hscrollbar_policy(gtk::PolicyType::Always)
        .child(&ap_viewport)
        .build();
    ap_scrolled.set_propagate_natural_width(false);
    ap_scrolled.set_overlay_scrolling(false);
    let (ap_pagination_row, ap_pagination) = build_table_pagination_controls(
        default_rows_per_page,
        table_filter_columns(&ap_layout, ap_column_label),
    );

    let ap_header_holder = GtkBox::new(Orientation::Vertical, 0);
    ap_header_holder.append(&ap_table_header(&ap_layout, &ap_sort, state.clone()));
    ap_header_holder.append(&ap_pagination.filter_bar);
    let ap_header_scrolled = ScrolledWindow::builder()
        .hexpand(false)
        .vexpand(false)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&ap_header_holder)
        .build();
    ap_header_scrolled.set_propagate_natural_width(false);
    let ap_header_hadj = ap_header_scrolled.hadjustment();
    {
        let ap_header_hadj = ap_header_hadj.clone();
        ap_scroll_adj.connect_value_changed(move |adj| {
            let max_value = (ap_header_hadj.upper() - ap_header_hadj.page_size()).max(0.0);
            let value = adj.value().clamp(0.0, max_value);
            if (ap_header_hadj.value() - value).abs() > f64::EPSILON {
                ap_header_hadj.set_value(value);
            }
        });
    }
    let ap_top = GtkBox::new(Orientation::Vertical, 4);
    ap_top.set_hexpand(false);
    ap_top.set_halign(gtk::Align::Start);
    ap_top.append(&ap_header_scrolled);
    ap_top.append(&ap_scrolled);
    let ap_scroll_debug_label = Label::new(Some("AP scroll: pending"));
    ap_scroll_debug_label.set_xalign(0.0);
    ap_scroll_debug_label.add_css_class("caption");
    ap_top.append(&ap_scroll_debug_label);
    let ap_scroll_controls = GtkBox::new(Orientation::Horizontal, 6);
    let ap_scroll_home_btn = Button::with_label("<<");
    let ap_scroll_left_btn = Button::with_label("<");
    let ap_scroll_right_btn = Button::with_label(">");
    let ap_scroll_end_btn = Button::with_label(">>");
    {
        let adj = ap_scroll_adj.clone();
        ap_scroll_home_btn.connect_clicked(move |_| adj.set_value(0.0));
    }
    {
        let adj = ap_scroll_adj.clone();
        ap_scroll_left_btn.connect_clicked(move |_| {
            nudge_adjustment(&adj, -160.0);
        });
    }
    {
        let adj = ap_scroll_adj.clone();
        ap_scroll_right_btn.connect_clicked(move |_| {
            nudge_adjustment(&adj, 160.0);
        });
    }
    {
        let adj = ap_scroll_adj.clone();
        ap_scroll_end_btn.connect_clicked(move |_| {
            let max_value = (adj.upper() - adj.page_size()).max(0.0);
            adj.set_value(max_value);
        });
    }
    for button in [
        &ap_scroll_home_btn,
        &ap_scroll_left_btn,
        &ap_scroll_right_btn,
        &ap_scroll_end_btn,
    ] {
        ap_scroll_controls.append(button);
    }
    ap_top.append(&ap_scroll_controls);
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
        .hscrollbar_policy(gtk::PolicyType::Automatic)
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
    ap_summary_row.set_shrink_start_child(true);
    ap_summary_row.set_shrink_end_child(true);
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
    ap_detail_sections.set_shrink_start_child(true);
    ap_detail_sections.set_shrink_end_child(true);
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
    ap_assoc_list.set_activate_on_single_click(false);
    attach_listbox_click_selection(&ap_assoc_list);
    let ap_assoc_scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
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
    let ap_assoc_outer_scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Always)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&ap_assoc_box)
        .build();

    let ap_bottom = Paned::new(Orientation::Horizontal);
    ap_bottom.set_wide_handle(true);
    ap_bottom.set_position(DEFAULT_AP_BOTTOM_POSITION);
    ap_bottom.set_resize_start_child(true);
    ap_bottom.set_resize_end_child(false);
    ap_bottom.set_shrink_start_child(true);
    ap_bottom.set_shrink_end_child(true);
    ap_bottom.set_end_child(Some(&ap_assoc_outer_scrolled));

    let ap_root = Paned::new(Orientation::Vertical);
    ap_root.set_wide_handle(true);
    ap_root.set_position(DEFAULT_AP_ROOT_POSITION);
    ap_root.set_resize_start_child(true);
    ap_root.set_resize_end_child(true);
    ap_root.set_shrink_start_child(true);
    ap_root.set_shrink_end_child(true);
    ap_root.set_start_child(Some(&ap_top));
    ap_root.set_end_child(Some(&ap_bottom));

    notebook.append_page(&ap_root, Some(&Label::new(Some("Access Points"))));

    let client_list = ListBox::new();
    client_list.set_selection_mode(gtk::SelectionMode::Single);
    client_list.set_activate_on_single_click(false);
    attach_listbox_click_selection(&client_list);
    let client_list_canvas = GtkBox::new(Orientation::Horizontal, 0);
    client_list_canvas.set_hexpand(false);
    client_list_canvas.set_halign(gtk::Align::Start);
    client_list_canvas.append(&client_list);
    let client_scroll_adj = gtk::Adjustment::new(0.0, 0.0, 0.0, 24.0, 160.0, 680.0);
    let client_viewport = Viewport::new(Some(&client_scroll_adj), None::<&gtk::Adjustment>);
    client_viewport.set_child(Some(&client_list_canvas));
    let client_selection_suppressed = Rc::new(RefCell::new(false));
    let client_selected_key = Rc::new(RefCell::new(None::<String>));
    let client_scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(false)
        .hscrollbar_policy(gtk::PolicyType::Always)
        .child(&client_viewport)
        .build();
    client_scrolled.set_propagate_natural_width(false);
    client_scrolled.set_overlay_scrolling(false);
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
    let client_header_scrolled = ScrolledWindow::builder()
        .hexpand(false)
        .vexpand(false)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&client_header_holder)
        .build();
    client_header_scrolled.set_propagate_natural_width(false);
    let client_header_hadj = client_header_scrolled.hadjustment();
    {
        let client_header_hadj = client_header_hadj.clone();
        client_scroll_adj.connect_value_changed(move |adj| {
            let max_value = (client_header_hadj.upper() - client_header_hadj.page_size()).max(0.0);
            let value = adj.value().clamp(0.0, max_value);
            if (client_header_hadj.value() - value).abs() > f64::EPSILON {
                client_header_hadj.set_value(value);
            }
        });
    }
    let client_top = GtkBox::new(Orientation::Vertical, 4);
    client_top.set_hexpand(false);
    client_top.set_halign(gtk::Align::Start);
    client_top.append(&client_header_scrolled);
    client_top.append(&client_scrolled);
    let client_scroll_debug_label = Label::new(Some("Client scroll: pending"));
    client_scroll_debug_label.set_xalign(0.0);
    client_scroll_debug_label.add_css_class("caption");
    client_top.append(&client_scroll_debug_label);
    let client_scroll_controls = GtkBox::new(Orientation::Horizontal, 6);
    let client_scroll_home_btn = Button::with_label("<<");
    let client_scroll_left_btn = Button::with_label("<");
    let client_scroll_right_btn = Button::with_label(">");
    let client_scroll_end_btn = Button::with_label(">>");
    {
        let adj = client_scroll_adj.clone();
        client_scroll_home_btn.connect_clicked(move |_| adj.set_value(0.0));
    }
    {
        let adj = client_scroll_adj.clone();
        client_scroll_left_btn.connect_clicked(move |_| {
            nudge_adjustment(&adj, -160.0);
        });
    }
    {
        let adj = client_scroll_adj.clone();
        client_scroll_right_btn.connect_clicked(move |_| {
            nudge_adjustment(&adj, 160.0);
        });
    }
    {
        let adj = client_scroll_adj.clone();
        client_scroll_end_btn.connect_clicked(move |_| {
            let max_value = (adj.upper() - adj.page_size()).max(0.0);
            adj.set_value(max_value);
        });
    }
    for button in [
        &client_scroll_home_btn,
        &client_scroll_left_btn,
        &client_scroll_right_btn,
        &client_scroll_end_btn,
    ] {
        client_scroll_controls.append(button);
    }
    client_top.append(&client_scroll_controls);
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
        .hscrollbar_policy(gtk::PolicyType::Automatic)
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
    ap_detail_notebook.set_scrollable(true);
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
    client_detail_notebook.set_scrollable(true);
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
    client_root.set_shrink_start_child(true);
    client_root.set_shrink_end_child(true);
    client_root.set_start_child(Some(&client_top));
    client_root.set_end_child(Some(&client_detail_notebook));

    notebook.append_page(&client_root, Some(&Label::new(Some("Clients"))));

    let bluetooth_geiger_state = Rc::new(RefCell::new(BluetoothGeigerUiState::default()));

    let bluetooth_list = ListBox::new();
    bluetooth_list.set_selection_mode(gtk::SelectionMode::Single);
    bluetooth_list.set_activate_on_single_click(false);
    attach_listbox_click_selection(&bluetooth_list);
    let bluetooth_list_canvas = GtkBox::new(Orientation::Horizontal, 0);
    bluetooth_list_canvas.set_hexpand(false);
    bluetooth_list_canvas.set_halign(gtk::Align::Start);
    bluetooth_list_canvas.append(&bluetooth_list);
    let bluetooth_scroll_adj = gtk::Adjustment::new(0.0, 0.0, 0.0, 24.0, 160.0, 680.0);
    let bluetooth_viewport =
        Viewport::new(Some(&bluetooth_scroll_adj), None::<&gtk::Adjustment>);
    bluetooth_viewport.set_child(Some(&bluetooth_list_canvas));
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
        .hexpand(false)
        .hscrollbar_policy(gtk::PolicyType::Always)
        .child(&bluetooth_viewport)
        .build();
    bluetooth_scrolled.set_propagate_natural_width(false);
    bluetooth_scrolled.set_overlay_scrolling(false);
    let (bluetooth_pagination_row, bluetooth_pagination) = build_table_pagination_controls(
        default_rows_per_page,
        table_filter_columns(&bluetooth_layout, bluetooth_column_label),
    );
    bluetooth_header_holder.append(&bluetooth_pagination.filter_bar);
    let bluetooth_header_scrolled = ScrolledWindow::builder()
        .hexpand(false)
        .vexpand(false)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&bluetooth_header_holder)
        .build();
    bluetooth_header_scrolled.set_propagate_natural_width(false);
    let bluetooth_header_hadj = bluetooth_header_scrolled.hadjustment();
    {
        let bluetooth_header_hadj = bluetooth_header_hadj.clone();
        bluetooth_scroll_adj.connect_value_changed(move |adj| {
            let max_value =
                (bluetooth_header_hadj.upper() - bluetooth_header_hadj.page_size()).max(0.0);
            let value = adj.value().clamp(0.0, max_value);
            if (bluetooth_header_hadj.value() - value).abs() > f64::EPSILON {
                bluetooth_header_hadj.set_value(value);
            }
        });
    }
    let bluetooth_top = GtkBox::new(Orientation::Vertical, 4);
    bluetooth_top.set_hexpand(false);
    bluetooth_top.set_halign(gtk::Align::Start);
    bluetooth_top.append(&bluetooth_header_scrolled);
    bluetooth_top.append(&bluetooth_scrolled);
    let bluetooth_scroll_debug_label = Label::new(Some("Bluetooth scroll: pending"));
    bluetooth_scroll_debug_label.set_xalign(0.0);
    bluetooth_scroll_debug_label.add_css_class("caption");
    bluetooth_top.append(&bluetooth_scroll_debug_label);
    let bluetooth_scroll_controls = GtkBox::new(Orientation::Horizontal, 6);
    let bluetooth_scroll_home_btn = Button::with_label("<<");
    let bluetooth_scroll_left_btn = Button::with_label("<");
    let bluetooth_scroll_right_btn = Button::with_label(">");
    let bluetooth_scroll_end_btn = Button::with_label(">>");
    {
        let adj = bluetooth_scroll_adj.clone();
        bluetooth_scroll_home_btn.connect_clicked(move |_| adj.set_value(0.0));
    }
    {
        let adj = bluetooth_scroll_adj.clone();
        bluetooth_scroll_left_btn.connect_clicked(move |_| {
            nudge_adjustment(&adj, -160.0);
        });
    }
    {
        let adj = bluetooth_scroll_adj.clone();
        bluetooth_scroll_right_btn.connect_clicked(move |_| {
            nudge_adjustment(&adj, 160.0);
        });
    }
    {
        let adj = bluetooth_scroll_adj.clone();
        bluetooth_scroll_end_btn.connect_clicked(move |_| {
            let max_value = (adj.upper() - adj.page_size()).max(0.0);
            adj.set_value(max_value);
        });
    }
    for button in [
        &bluetooth_scroll_home_btn,
        &bluetooth_scroll_left_btn,
        &bluetooth_scroll_right_btn,
        &bluetooth_scroll_end_btn,
    ] {
        bluetooth_scroll_controls.append(button);
    }
    bluetooth_top.append(&bluetooth_scroll_controls);
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
    let bluetooth_enumeration_status_label = Label::new(Some("Enumeration status: idle"));
    bluetooth_enumeration_status_label.set_xalign(0.0);
    bluetooth_enumeration_status_label.set_wrap(true);
    bluetooth_enumeration_status_label.set_use_markup(true);
    set_bluetooth_enumeration_status_label(&bluetooth_enumeration_status_label, None, None);

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
    bluetooth_detail_box.append(&bluetooth_enumeration_status_label);
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
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(220)
        .child(&bluetooth_detail_box)
        .build();

    let bluetooth_bottom = Paned::new(Orientation::Horizontal);
    bluetooth_bottom.set_wide_handle(true);
    bluetooth_bottom.set_position(DEFAULT_BLUETOOTH_BOTTOM_POSITION);
    bluetooth_bottom.set_resize_start_child(true);
    bluetooth_bottom.set_resize_end_child(true);
    bluetooth_bottom.set_shrink_start_child(true);
    bluetooth_bottom.set_shrink_end_child(true);
    bluetooth_bottom.set_start_child(Some(&bluetooth_geiger_scrolled));
    bluetooth_bottom.set_end_child(Some(&bluetooth_detail_scrolled));

    let bluetooth_root = Paned::new(Orientation::Vertical);
    bluetooth_root.set_wide_handle(true);
    bluetooth_root.set_position(DEFAULT_BLUETOOTH_ROOT_POSITION);
    bluetooth_root.set_resize_start_child(true);
    bluetooth_root.set_resize_end_child(true);
    bluetooth_root.set_shrink_start_child(true);
    bluetooth_root.set_shrink_end_child(true);
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
    let interface_health_label = Label::new(Some("initializing interface status..."));
    interface_health_label.set_xalign(0.0);
    interface_health_label.set_wrap(true);
    interface_health_label.set_selectable(true);

    let gps_status_label = Label::new(Some("GPS status initializing"));
    gps_status_label.set_xalign(0.0);
    gps_status_label.set_wrap(true);
    gps_status_label.set_selectable(true);

    let channel_status_box = GtkBox::new(Orientation::Vertical, 6);
    channel_status_box.append(&Label::new(Some("Status")));
    channel_status_box.append(&status_label);
    channel_status_box.append(&Label::new(Some("Interface Status")));
    channel_status_box.append(&interface_health_label);
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
    channel_root.set_shrink_start_child(true);
    channel_root.set_shrink_end_child(true);
    channel_root.set_start_child(Some(&channel_controls));
    channel_root.set_end_child(Some(&channel_status_scrolled));
    notebook.append_page(&channel_root, Some(&Label::new(Some("Channel Usage"))));

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
        let window = window.clone();
        let state = state.clone();
        ap_list.connect_row_activated(move |_, row| {
            let key = row.widget_name().to_string();
            let ap = {
                let s = state.borrow();
                s.access_points.iter().find(|entry| entry.bssid == key).cloned()
            };
            if let Some(ap) = ap {
                open_ap_details_dialog(&window, &ap);
            }
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
        let window = window.clone();
        let state = state.clone();
        client_list.connect_row_activated(move |_, row| {
            let key = row.widget_name().to_string();
            let (client, aps) = {
                let s = state.borrow();
                (
                    s.clients.iter().find(|entry| entry.mac == key).cloned(),
                    s.access_points.clone(),
                )
            };
            if let Some(client) = client {
                open_client_details_dialog(&window, &client, &aps);
            }
        });
    }
    {
        let window = window.clone();
        let state = state.clone();
        ap_assoc_list.connect_row_activated(move |_, row| {
            let key = row.widget_name().to_string();
            let (client, aps) = {
                let s = state.borrow();
                (
                    s.clients.iter().find(|entry| entry.mac == key).cloned(),
                    s.access_points.clone(),
                )
            };
            if let Some(client) = client {
                open_client_details_dialog(&window, &client, &aps);
            }
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
        let window = window.clone();
        let state = state.clone();
        bluetooth_list.connect_row_activated(move |_, row| {
            let key = row.widget_name().to_string();
            let device = {
                let s = state.borrow();
                s.bluetooth_devices
                    .iter()
                    .find(|entry| entry.mac == key)
                    .cloned()
            };
            if let Some(device) = device {
                open_bluetooth_details_dialog(&window, &device);
            }
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

            let device_mac = device.mac.clone();
            let (controller, sender) = {
                let s = state.borrow();
                (
                    s.settings.bluetooth_controller.clone(),
                    s.bluetooth_sender.clone(),
                )
            };
            {
                let mut s = state.borrow_mut();
                s.set_bluetooth_enumeration_status(
                    device_mac.clone(),
                    format!("Enumerating {}...", device_mac),
                    false,
                );
                s.push_status(format!(
                    "starting active bluetooth enumeration for {}",
                    device_mac
                ));
            }

            thread::spawn(move || {
                match bluetooth::connect_and_enumerate_device(controller.as_deref(), &device_mac) {
                    Ok(record) => {
                        let status = bluetooth_enumeration_status_from_device(&record).unwrap_or(
                            BluetoothEnumerationStatus {
                                message: format!("Enumeration complete for {}", record.mac),
                                is_error: false,
                            },
                        );
                        let log_line = if status.is_error {
                            format!(
                                "active bluetooth enumeration completed with warning for {}: {}",
                                record.mac, status.message
                            )
                        } else {
                            format!("active bluetooth enumeration completed for {}", record.mac)
                        };
                        let mac = record.mac.clone();
                        let _ = sender.send(BluetoothEvent::DeviceSeen(record));
                        let _ = sender.send(BluetoothEvent::EnumerationStatus {
                            mac,
                            message: status.message,
                            is_error: status.is_error,
                        });
                        let _ = sender.send(BluetoothEvent::Log(log_line));
                    }
                    Err(err) => {
                        let message = format!("Enumeration failed for {}: {}", device_mac, err);
                        let _ = sender.send(BluetoothEvent::EnumerationStatus {
                            mac: device_mac.clone(),
                            message,
                            is_error: true,
                        });
                        let _ = sender.send(BluetoothEvent::Log(format!(
                            "active bluetooth enumeration failed for {}: {err}",
                            device_mac
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
        window,
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
                let out = s.exporter.export_filtered_pcap(
                    &s.session_capture_path,
                    &format!("ap_{}.pcapng", sanitize_name(&ap.bssid)),
                    &format!("wlan.bssid == {}", ap.bssid),
                    &s.gps_track,
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
                let out = s.exporter.export_handshake_pcap(
                    &s.session_capture_path,
                    &ap.bssid,
                    &s.gps_track,
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
                let out = s.exporter.export_filtered_pcap(
                    &s.session_capture_path,
                    &format!("client_{}.pcapng", sanitize_name(&client.mac)),
                    &format!("wlan.sa == {} || wlan.da == {}", client.mac, client.mac),
                    &s.gps_track,
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

    (
        notebook,
        UiWidgets {
            ap_root,
            ap_bottom,
            ap_detail_notebook,
            ap_assoc_box,
            ap_header_holder,
            ap_header_scrolled,
            ap_scrolled,
            ap_list_canvas,
            ap_list,
            ap_pagination,
            ap_scroll_adj,
            ap_selection_suppressed,
            ap_selected_key,
            ap_detail_label,
            ap_detail_scroll,
            ap_notes_view,
            ap_assoc_header_holder,
            ap_assoc_list,
            ap_assoc_pagination,
            ap_packet_draw,
            ap_selected_packet_mix: selected_packet_mix,
            ap_scroll_debug_label,
            client_header_holder,
            client_header_scrolled,
            client_scrolled,
            client_list,
            client_pagination,
            client_scroll_adj,
            client_scroll_debug_label,
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
            bluetooth_header_scrolled,
            bluetooth_scrolled,
            bluetooth_pagination,
            bluetooth_scroll_adj,
            bluetooth_scroll_debug_label,
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
            bluetooth_enumeration_status_label,
            bluetooth_root,
            bluetooth_bottom,
            bluetooth_geiger_rssi,
            bluetooth_geiger_tone,
            bluetooth_geiger_progress,
            bluetooth_geiger_state,
            channel_draw,
            status_label,
            interface_health_label,
            gps_status_label,
        },
    )
}

fn bind_poll_loop(
    receiver: Receiver<CaptureEvent>,
    bluetooth_receiver: Receiver<BluetoothEvent>,
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
        ap_header_scrolled,
        ap_scrolled,
        ap_list_canvas,
        ap_list,
        ap_pagination,
        ap_scroll_adj,
        ap_selection_suppressed,
        ap_selected_key,
        ap_detail_label,
        ap_detail_scroll,
        ap_notes_view,
        ap_assoc_header_holder,
        ap_assoc_list,
        ap_assoc_pagination,
        ap_packet_draw,
        ap_selected_packet_mix,
        ap_scroll_debug_label,
        client_header_holder,
        client_header_scrolled,
        client_scrolled,
        client_list,
        client_pagination,
        client_scroll_adj,
        client_scroll_debug_label,
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
        bluetooth_header_scrolled,
        bluetooth_scrolled,
        bluetooth_pagination,
        bluetooth_scroll_adj,
        bluetooth_scroll_debug_label,
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
        bluetooth_enumeration_status_label,
        bluetooth_root: _bluetooth_root,
        bluetooth_bottom: _bluetooth_bottom,
        bluetooth_geiger_rssi,
        bluetooth_geiger_tone,
        bluetooth_geiger_progress,
        bluetooth_geiger_state,
        channel_draw,
        status_label,
        interface_health_label,
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
    let last_ap_pagination_generation = Cell::new(ap_pagination.generation.get());
    let last_ap_assoc_pagination_generation = Cell::new(ap_assoc_pagination.generation.get());
    let last_client_pagination_generation = Cell::new(client_pagination.generation.get());
    let last_bluetooth_pagination_generation = Cell::new(bluetooth_pagination.generation.get());
    let pending_ap_refresh = Cell::new(true);
    let pending_client_refresh = Cell::new(true);
    let pending_bluetooth_refresh = Cell::new(true);
    let pending_channel_refresh = Cell::new(true);

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

            // Drain any residual events buffered between stop request and runtime shutdown
            // so on-stop exports reflect the final observed state.
            loop {
                let events = drain_capture_events_batch(&receiver, MAX_CAPTURE_EVENTS_PER_TICK);
                if events.is_empty() {
                    break;
                }
                for event in events {
                    refresh.merge(s.apply_capture_event(event).unwrap_or_default());
                }
            }
            loop {
                let events = drain_bluetooth_events_batch(
                    &bluetooth_receiver,
                    MAX_BLUETOOTH_EVENTS_PER_TICK,
                );
                if events.is_empty() {
                    break;
                }
                for event in events {
                    refresh.merge(s.apply_bluetooth_event(event).unwrap_or_default());
                }
            }

            s.export_session_snapshots();
            if let Some(message) = restart_message {
                s.push_status(message);
                s.start_scanning();
            } else {
                s.set_wifi_health_state(InterfaceHealthState::Idle, "Wi-Fi scanning stopped");
                s.set_bluetooth_health_state(
                    InterfaceHealthState::Idle,
                    "Bluetooth scanning stopped",
                );
                s.wifi_last_data_at = None;
                s.bluetooth_last_data_at = None;
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
            if completion.wifi_started {
                s.set_wifi_health_state(InterfaceHealthState::Active, "capturing packets");
                s.wifi_last_data_at = Some(Instant::now());
                s.wifi_consecutive_watchdog_restarts = 0;
            } else if completion.wifi_failed {
                s.set_wifi_health_state(
                    InterfaceHealthState::Error,
                    "Wi-Fi capture failed to start",
                );
            } else if !s.settings.interfaces.iter().any(|iface| iface.enabled) {
                s.set_wifi_health_state(InterfaceHealthState::Idle, "Wi-Fi scanning disabled");
                s.wifi_last_data_at = None;
            }
            if completion.bluetooth_started {
                s.set_bluetooth_health_state(
                    InterfaceHealthState::Active,
                    "scanning Bluetooth advertisements",
                );
                s.bluetooth_last_data_at = Some(Instant::now());
                s.bluetooth_consecutive_watchdog_restarts = 0;
            } else if !s.settings.bluetooth_enabled {
                s.set_bluetooth_health_state(
                    InterfaceHealthState::Idle,
                    "Bluetooth scanning disabled",
                );
                s.bluetooth_last_data_at = None;
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

        {
            let mut s = state.borrow_mut();
            if s.maybe_run_scan_watchdog() {
                refresh.status = true;
            }
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

        let active_tab = notebook.current_page().unwrap_or(ACCESS_POINTS_TAB_INDEX);
        let ap_tab_active = active_tab == ACCESS_POINTS_TAB_INDEX;
        let client_tab_active = active_tab == CLIENTS_TAB_INDEX;
        let bluetooth_tab_active = active_tab == BLUETOOTH_TAB_INDEX;
        let channel_tab_active = active_tab == CHANNEL_USAGE_TAB_INDEX;

        {
            let s = state.borrow();
            let table_viewport_width_px = if is_small_display() {
                680
            } else {
                (window.width().max(MIN_WINDOW_WIDTH) - 52).max(320)
            };
            let ap_row_width_px = table_row_width_px_for_layout(&s.settings.ap_table_layout)
                .max(AP_TABLE_MIN_WIDTH_PX);
            let client_row_width_px = table_row_width_px_for_layout(&s.settings.client_table_layout)
                .max(CLIENT_TABLE_MIN_WIDTH_PX);
            let bluetooth_row_width_px =
                table_row_width_px_for_layout(&s.settings.bluetooth_table_layout)
                    .max(BLUETOOTH_TABLE_MIN_WIDTH_PX);
            ap_header_scrolled.set_max_content_width(table_viewport_width_px);
            ap_scrolled.set_max_content_width(table_viewport_width_px);
            ap_header_scrolled.set_min_content_width(table_viewport_width_px);
            ap_scrolled.set_min_content_width(table_viewport_width_px);
            ap_header_scrolled.set_size_request(table_viewport_width_px, -1);
            ap_scrolled.set_size_request(table_viewport_width_px, -1);
            client_header_scrolled.set_max_content_width(table_viewport_width_px);
            client_scrolled.set_max_content_width(table_viewport_width_px);
            client_header_scrolled.set_min_content_width(table_viewport_width_px);
            client_scrolled.set_min_content_width(table_viewport_width_px);
            client_header_scrolled.set_size_request(table_viewport_width_px, -1);
            client_scrolled.set_size_request(table_viewport_width_px, -1);
            bluetooth_header_scrolled.set_max_content_width(table_viewport_width_px);
            bluetooth_scrolled.set_max_content_width(table_viewport_width_px);
            bluetooth_header_scrolled.set_min_content_width(table_viewport_width_px);
            bluetooth_scrolled.set_min_content_width(table_viewport_width_px);
            bluetooth_header_scrolled.set_size_request(table_viewport_width_px, -1);
            bluetooth_scrolled.set_size_request(table_viewport_width_px, -1);
            ap_list.set_size_request(ap_row_width_px, -1);
            ap_list_canvas.set_size_request(ap_row_width_px, -1);
            ap_header_holder.set_size_request(ap_row_width_px, -1);
            ap_header_holder.set_halign(gtk::Align::Start);
            ap_list.set_halign(gtk::Align::Start);
            ap_list.set_hexpand(false);
            let ap_upper = ap_row_width_px as f64;
            let ap_page = table_viewport_width_px as f64;
            update_horizontal_adjustment_bounds(
                &ap_scroll_adj,
                ap_upper,
                ap_page,
                24.0,
                (ap_page * 0.7).max(48.0),
            );
            client_list.set_size_request(client_row_width_px, -1);
            client_header_holder.set_size_request(client_row_width_px, -1);
            client_header_holder.set_halign(gtk::Align::Start);
            client_list.set_halign(gtk::Align::Start);
            client_list.set_hexpand(false);
            let client_upper = client_row_width_px as f64;
            let client_page = table_viewport_width_px as f64;
            update_horizontal_adjustment_bounds(
                &client_scroll_adj,
                client_upper,
                client_page,
                24.0,
                (client_page * 0.7).max(48.0),
            );
            bluetooth_list.set_size_request(bluetooth_row_width_px, -1);
            bluetooth_header_holder.set_size_request(bluetooth_row_width_px, -1);
            bluetooth_header_holder.set_halign(gtk::Align::Start);
            bluetooth_list.set_halign(gtk::Align::Start);
            bluetooth_list.set_hexpand(false);
            let bluetooth_upper = bluetooth_row_width_px as f64;
            let bluetooth_page = table_viewport_width_px as f64;
            update_horizontal_adjustment_bounds(
                &bluetooth_scroll_adj,
                bluetooth_upper,
                bluetooth_page,
                24.0,
                (bluetooth_page * 0.7).max(48.0),
            );
        }
        let ap_max_scroll = (ap_scroll_adj.upper() - ap_scroll_adj.page_size()).max(0.0);
        ap_scroll_debug_label.set_text(&format!(
            "AP scroll: value={:.1} upper={:.1} page={:.1} max={:.1}",
            ap_scroll_adj.value(),
            ap_scroll_adj.upper(),
            ap_scroll_adj.page_size(),
            ap_max_scroll
        ));
        let client_max_scroll = (client_scroll_adj.upper() - client_scroll_adj.page_size()).max(0.0);
        client_scroll_debug_label.set_text(&format!(
            "Client scroll: value={:.1} upper={:.1} page={:.1} max={:.1}",
            client_scroll_adj.value(),
            client_scroll_adj.upper(),
            client_scroll_adj.page_size(),
            client_max_scroll
        ));
        let bluetooth_max_scroll =
            (bluetooth_scroll_adj.upper() - bluetooth_scroll_adj.page_size()).max(0.0);
        bluetooth_scroll_debug_label.set_text(&format!(
            "Bluetooth scroll: value={:.1} upper={:.1} page={:.1} max={:.1}",
            bluetooth_scroll_adj.value(),
            bluetooth_scroll_adj.upper(),
            bluetooth_scroll_adj.page_size(),
            bluetooth_max_scroll
        ));

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
                        let ap_detail_hadj = ap_detail_scroll.hadjustment();
                        if ap_detail_hadj.value() != 0.0 {
                            ap_detail_hadj.set_value(0.0);
                        }
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

                    let assoc_clients = s
                        .clients
                        .iter()
                        .filter(|client| client_seen_on_ap(client, &ap.bssid))
                        .cloned()
                        .collect::<Vec<_>>();
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
                    let ap_detail_hadj = ap_detail_scroll.hadjustment();
                    if ap_detail_hadj.value() != 0.0 {
                        ap_detail_hadj.set_value(0.0);
                    }
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
                let ap_detail_hadj = ap_detail_scroll.hadjustment();
                if ap_detail_hadj.value() != 0.0 {
                    ap_detail_hadj.set_value(0.0);
                }
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
                    let status = s
                        .bluetooth_enumeration_status
                        .get(&device.mac)
                        .cloned()
                        .or_else(|| bluetooth_enumeration_status_from_device(device));
                    set_bluetooth_enumeration_status_label(
                        &bluetooth_enumeration_status_label,
                        status.as_ref(),
                        Some(&device.mac),
                    );
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
                    set_bluetooth_enumeration_status_label(
                        &bluetooth_enumeration_status_label,
                        None,
                        Some(key),
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
                set_bluetooth_enumeration_status_label(
                    &bluetooth_enumeration_status_label,
                    None,
                    None,
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

        let (
            status_text,
            interface_status_text,
            gps_text,
            wifi_running,
            bluetooth_running,
            scan_transition_in_progress,
        ) = {
            let s = state.borrow();
            (
                s.status_text(),
                s.interface_runtime_status_text(),
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
        interface_health_label.set_text(&interface_status_text);

        gps_status_label.set_text(&gps_text);
        global_gps_status_label.set_text(&gps_text);

        glib::ControlFlow::Continue
    });
}

fn interface_health_state_label(state: InterfaceHealthState) -> &'static str {
    match state {
        InterfaceHealthState::Idle => "Idle",
        InterfaceHealthState::Active => "Active",
        InterfaceHealthState::Stalled => "Stalled",
        InterfaceHealthState::Restarting => "Restarting",
        InterfaceHealthState::Error => "Error",
    }
}

fn format_health_elapsed(last_data_at: Option<Instant>) -> String {
    last_data_at
        .map(|ts| format!("{}s ago", ts.elapsed().as_secs()))
        .unwrap_or_else(|| "n/a".to_string())
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
    let local_ipv4 = intel.local_ipv4_addresses.join(",");
    let local_ipv6 = intel.local_ipv6_addresses.join(",");
    let dhcp_hostnames = intel.dhcp_hostnames.join(",");
    let dhcp_fqdns = intel.dhcp_fqdns.join(",");
    let dhcp_vendor_classes = intel.dhcp_vendor_classes.join(",");
    let dns_names = intel.dns_names.join(",");
    let qos_priorities = intel
        .qos_priorities
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let endpoints = intel
        .remote_endpoints
        .iter()
        .map(|endpoint| {
            format!(
                "{}:{}:{}:{}:{}:{}",
                endpoint.protocol,
                endpoint.ip_address,
                endpoint.port.unwrap_or_default(),
                endpoint.domain.as_deref().unwrap_or(""),
                endpoint.geo_city.as_deref().unwrap_or(""),
                endpoint.packet_count
            )
        })
        .collect::<Vec<_>>()
        .join("|");

    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        local_ipv4,
        local_ipv6,
        dhcp_hostnames,
        dhcp_fqdns,
        dhcp_vendor_classes,
        dns_names,
        endpoints,
        intel.uplink_bytes,
        intel.downlink_bytes,
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
    let mut sorted = clients.to_vec();
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
    let current_count = filtered
        .iter()
        .filter(|client| client.associated_ap.as_deref() == Some(ap_bssid))
        .count();
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
            "ipv4_addresses" => a
                .network_intel
                .local_ipv4_addresses
                .join(",")
                .cmp(&b.network_intel.local_ipv4_addresses.join(",")),
            "ipv6_addresses" => a
                .network_intel
                .local_ipv6_addresses
                .join(",")
                .cmp(&b.network_intel.local_ipv6_addresses.join(",")),
            "dhcp_hostnames" => a
                .network_intel
                .dhcp_hostnames
                .join(",")
                .cmp(&b.network_intel.dhcp_hostnames.join(",")),
            "dns_names" => a
                .network_intel
                .dns_names
                .join(",")
                .cmp(&b.network_intel.dns_names.join(",")),
            "remote_endpoints" => a
                .network_intel
                .remote_endpoints
                .len()
                .cmp(&b.network_intel.remote_endpoints.len()),
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
        value_for(column_id)
            .map(|value| value.to_ascii_lowercase().contains(needle))
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
    let row_width_px = table_row_width_px_for_layout(&settings.ap_table_layout)
        .max(AP_TABLE_MIN_WIDTH_PX);
    set_table_overflow_width(list, row_width_px);
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
        let watchlist_match = ap_watchlist_match(&ap, &settings.watchlists);
        set_row_alert_classes(
            &row,
            watchlist_match
                .as_ref()
                .map(|matched| matched.css_class.as_str()),
            &watchlist_classes,
            ap.handshake_count > 0,
        );
        let line = GtkBox::new(Orientation::Horizontal, 14);
        line.set_hexpand(false);
        line.set_halign(gtk::Align::Start);
        line.set_size_request(row_width_px, -1);
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

fn table_row_width_px_for_layout(layout: &TableLayout) -> i32 {
    let visible_columns = layout.columns.iter().filter(|c| c.visible).collect::<Vec<_>>();
    let content = visible_columns
        .iter()
        .map(|column| column.width_chars.max(6) * TABLE_CHAR_WIDTH_PX)
        .sum::<i32>();
    let gaps = (visible_columns.len().saturating_sub(1) as i32) * 14;
    (content + gaps + 24).max(0)
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
    let row_width_px = table_row_width_px_for_layout(&settings.client_table_layout)
        .max(CLIENT_TABLE_MIN_WIDTH_PX);
    set_table_overflow_width(list, row_width_px);

    for client in filtered
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
    {
        let row = ListBoxRow::new();
        row.set_widget_name(&client.mac);
        attach_row_click_selection(&row, list, selected_key_state.clone());
        let watchlist_match = client_watchlist_match(&client, aps, &settings.watchlists);
        set_row_alert_classes(
            &row,
            watchlist_match
                .as_ref()
                .map(|matched| matched.css_class.as_str()),
            &watchlist_classes,
            false,
        );
        let line = GtkBox::new(Orientation::Horizontal, 14);
        line.set_hexpand(false);
        line.set_halign(gtk::Align::Start);
        line.set_size_request(row_width_px, -1);
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
    let mut latest_enum_status: HashMap<String, (String, bool)> = HashMap::new();
    let mut logs = Vec::new();

    for _ in 0..limit {
        let Ok(event) = receiver.try_recv() else {
            break;
        };
        match event {
            BluetoothEvent::DeviceSeen(device) => {
                latest_devices.insert(device.mac.clone(), device);
            }
            BluetoothEvent::EnumerationStatus {
                mac,
                message,
                is_error,
            } => {
                latest_enum_status.insert(mac, (message, is_error));
            }
            BluetoothEvent::Log(text) => logs.push(text),
        }
    }

    let mut events =
        Vec::with_capacity(logs.len() + latest_enum_status.len() + latest_devices.len());
    events.extend(logs.into_iter().map(BluetoothEvent::Log));
    events.extend(
        latest_enum_status
            .into_iter()
            .map(
                |(mac, (message, is_error))| BluetoothEvent::EnumerationStatus {
                    mac,
                    message,
                    is_error,
                },
            ),
    );
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
    let mut sorted = clients.to_vec();
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
        row.set_widget_name(&client.mac);
        attach_row_click_selection(&row, list, None);
        set_row_alert_classes(&row, None, &no_watchlist_classes, false);
        let line = GtkBox::new(Orientation::Horizontal, 14);
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
    let row_width_px = table_row_width_px_for_layout(&settings.bluetooth_table_layout)
        .max(BLUETOOTH_TABLE_MIN_WIDTH_PX);
    set_table_overflow_width(list, row_width_px);

    for device in filtered
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
    {
        let row = ListBoxRow::new();
        row.set_widget_name(&device.mac);
        attach_row_click_selection(&row, list, selected_key_state.clone());
        let watchlist_match = bluetooth_watchlist_match(&device, watchlists);
        set_row_alert_classes(
            &row,
            watchlist_match
                .as_ref()
                .map(|matched| matched.css_class.as_str()),
            &watchlist_classes,
            false,
        );

        let line = GtkBox::new(Orientation::Horizontal, 14);
        line.set_hexpand(false);
        line.set_halign(gtk::Align::Start);
        line.set_size_request(row_width_px, -1);
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

fn set_table_overflow_width(list: &ListBox, row_width_px: i32) {
    let width = row_width_px.max(0);
    list.set_size_request(width, -1);
    list.set_halign(gtk::Align::Start);
    list.set_hexpand(false);
    if let Some(parent) = list.parent() {
        if let Ok(canvas) = parent.downcast::<GtkBox>() {
            canvas.set_size_request(width, -1);
            canvas.set_halign(gtk::Align::Start);
            canvas.set_hexpand(false);
        }
    }
}

fn update_horizontal_adjustment_bounds(
    adj: &gtk::Adjustment,
    upper: f64,
    page_size: f64,
    step: f64,
    page_inc: f64,
) {
    let upper = upper.max(0.0);
    let page_size = page_size.max(1.0);
    let max_value = (upper - page_size).max(0.0);
    let current = adj.value().clamp(0.0, max_value);
    if (adj.upper() - upper).abs() > 0.5 {
        adj.set_upper(upper);
    }
    if (adj.page_size() - page_size).abs() > 0.5 {
        adj.set_page_size(page_size);
    }
    if (adj.step_increment() - step).abs() > 0.1 {
        adj.set_step_increment(step);
    }
    if (adj.page_increment() - page_inc).abs() > 0.1 {
        adj.set_page_increment(page_inc);
    }
    if (adj.value() - current).abs() > 0.5 {
        adj.set_value(current);
    }
}

fn nudge_adjustment(adj: &gtk::Adjustment, delta: f64) {
    let max_value = (adj.upper() - adj.page_size()).max(0.0);
    let next = (adj.value() + delta).clamp(0.0, max_value);
    adj.set_value(next);
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

fn header_cell(text: String, width_chars: i32) -> Label {
    let label = label_cell(text, width_chars);
    label.set_xalign(0.5);
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
        "ipv4_addresses" => client.network_intel.local_ipv4_addresses.join(", "),
        "ipv6_addresses" => client.network_intel.local_ipv6_addresses.join(", "),
        "dhcp_hostnames" => client.network_intel.dhcp_hostnames.join(", "),
        "dns_names" => client.network_intel.dns_names.join(", "),
        "remote_endpoints" => client
            .network_intel
            .remote_endpoints
            .iter()
            .take(4)
            .map(|endpoint| {
                let port = endpoint
                    .port
                    .map(|value| format!(":{value}"))
                    .unwrap_or_default();
                format!("{} {}{}", endpoint.protocol, endpoint.ip_address, port)
            })
            .collect::<Vec<_>>()
            .join(" | "),
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

fn bluetooth_enumeration_status_from_device(
    device: &BluetoothDeviceRecord,
) -> Option<BluetoothEnumerationStatus> {
    let active = device.active_enumeration.as_ref()?;
    if let Some(error) = active.last_error.as_ref() {
        return Some(BluetoothEnumerationStatus {
            message: format!("Enumeration failed for {}: {}", device.mac, error),
            is_error: true,
        });
    }

    let service_count = active.services.len();
    let characteristic_count = active.characteristics.len();
    if service_count == 0 {
        return Some(BluetoothEnumerationStatus {
            message: format!("Enumeration returned no services for {}", device.mac),
            is_error: true,
        });
    }

    Some(BluetoothEnumerationStatus {
        message: format!(
            "Enumeration complete for {} (services: {}, characteristics: {})",
            device.mac, service_count, characteristic_count
        ),
        is_error: false,
    })
}

fn set_bluetooth_enumeration_status_label(
    label: &Label,
    status: Option<&BluetoothEnumerationStatus>,
    selected_mac: Option<&str>,
) {
    let text = match (selected_mac, status) {
        (None, _) => "<b>Enumeration status:</b> select a Bluetooth device".to_string(),
        (Some(_), None) => "<b>Enumeration status:</b> idle".to_string(),
        (Some(_), Some(status)) if status.is_error => format!(
            "<span foreground='red'><b>Enumeration status:</b> {}</span>",
            glib::markup_escape_text(&status.message)
        ),
        (Some(_), Some(status)) => format!(
            "<b>Enumeration status:</b> {}",
            glib::markup_escape_text(&status.message)
        ),
    };
    label.set_markup(&text);
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
    watchlist_class: Option<&str>,
    all_watchlist_classes: &[String],
    handshake: bool,
) {
    for class_name in all_watchlist_classes {
        row.remove_css_class(class_name);
    }
    if let Some(class_name) = watchlist_class {
        row.add_css_class(class_name);
        row.remove_css_class("row-handshake");
    } else if handshake {
        row.add_css_class("row-handshake");
    } else {
        row.remove_css_class("row-handshake");
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

fn format_endpoint_summary(client: &ClientRecord) -> String {
    if client.network_intel.remote_endpoints.is_empty() {
        return "None observed".to_string();
    }

    client
        .network_intel
        .remote_endpoints
        .iter()
        .take(16)
        .map(|endpoint| {
            let port = endpoint
                .port
                .map(|value| format!(":{value}"))
                .unwrap_or_default();
            let domain = endpoint.domain.as_deref().unwrap_or("unresolved");
            let geo = endpoint.geo_city.as_deref().unwrap_or("location unknown");
            format!(
                "{} {}{} | domain={} | city={} | packets={} | first={} | last={}",
                endpoint.protocol,
                endpoint.ip_address,
                port,
                domain,
                geo,
                endpoint.packet_count,
                endpoint.first_seen.format("%H:%M:%S"),
                endpoint.last_seen.format("%H:%M:%S")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    let local_ipv4 = if client.network_intel.local_ipv4_addresses.is_empty() {
        "None observed".to_string()
    } else {
        client.network_intel.local_ipv4_addresses.join(", ")
    };
    let local_ipv6 = if client.network_intel.local_ipv6_addresses.is_empty() {
        "None observed".to_string()
    } else {
        client.network_intel.local_ipv6_addresses.join(", ")
    };
    let dhcp_hostnames = if client.network_intel.dhcp_hostnames.is_empty() {
        "None observed".to_string()
    } else {
        client.network_intel.dhcp_hostnames.join(", ")
    };
    let dhcp_fqdns = if client.network_intel.dhcp_fqdns.is_empty() {
        "None observed".to_string()
    } else {
        client.network_intel.dhcp_fqdns.join(", ")
    };
    let dhcp_vendor_classes = if client.network_intel.dhcp_vendor_classes.is_empty() {
        "None observed".to_string()
    } else {
        client.network_intel.dhcp_vendor_classes.join(", ")
    };
    let dns_names = if client.network_intel.dns_names.is_empty() {
        "None observed".to_string()
    } else {
        client.network_intel.dns_names.join(", ")
    };
    let open_network_endpoints = format_endpoint_summary(client);
    let roam_count = client.seen_access_points.len().saturating_sub(1);
    let associated_ssid =
        associated_ssid_for_client(aps, client).unwrap_or_else(|| "Unknown".to_string());

    format!(
        "Identity\nMAC: {}\nOUI: {}\nRandomized MAC: {}\nDHCP Vendor Class: {}\n\nAssociation\nAssociated AP: {}\nAssociated SSID: {}\nSeen AP Count: {}\nSeen APs: {}\nRoam Count: {}\nProbe Count: {}\nProbes: {}\nFirst Heard: {}\nLast Heard: {}\n\nRadio And Behavior\nBand: {}\nLast Channel: {}\nLast Frequency: {}\nCurrent RSSI: {}\nAverage RSSI: {}\nMinimum RSSI: {}\nMaximum RSSI: {}\nRSSI Samples: {}\nPacket Mix: mgmt={} control={} data={} other={}\nData Transferred: {} bytes\nUplink Bytes: {}\nDownlink Bytes: {}\nRetry Frames: {}\nRetry Rate: {}\nPower Save Observed: {}\nQoS Priorities: {}\nLast Frame: {}\nListen Interval: {}\n\nSecurity\nWPS: {}\nEAPOL Frames: {}\nPMKID Count: {}\nHandshake Network Count: {}\nHandshake Networks: {}\nLast Status Code: {}\nLast Reason Code: {}\n\nOpen Network Metadata\nLocal IPv4 Addresses: {}\nLocal IPv6 Addresses: {}\nDHCP Hostnames: {}\nDHCP FQDNs: {}\nDNS Names: {}\nRemote Endpoints:\n{}\n\nPresence\nObservation Count: {}\nFirst Location: {}\nLast Location: {}\nStrongest Location: {}",
        client.mac,
        client.oui_manufacturer.clone().unwrap_or_else(|| "Unknown".into()),
        bool_text(is_randomized_mac(&client.mac)),
        dhcp_vendor_classes,
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
        local_ipv4,
        local_ipv6,
        dhcp_hostnames,
        dhcp_fqdns,
        dns_names,
        open_network_endpoints,
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

    let uuid_details = format_bluetooth_uuid_metadata(device);

    format!(
        "MFGR IDs: {}\nUUIDs: {}\nUUID Resolver Details:\n{}",
        mfgr, uuids, uuid_details
    )
}

fn format_bluetooth_uuid_metadata(device: &BluetoothDeviceRecord) -> String {
    if device.uuids.is_empty() {
        return "None observed".to_string();
    }
    let metadata = bluetooth::resolve_uuid_metadata_many(&device.uuids);
    if metadata.is_empty() {
        return "No resolver metadata available".to_string();
    }
    metadata
        .into_iter()
        .map(|entry| {
            let name = entry.name.unwrap_or_else(|| "Unknown".to_string());
            let kind = entry.kind.unwrap_or_else(|| "Unknown Type".to_string());
            let short = entry
                .short_form
                .map(|value| format!(" ({value})"))
                .unwrap_or_default();
            format!("- {}{}: {} [{}]", entry.uuid, short, name, kind)
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    window: &ApplicationWindow,
    bluetooth_list: &ListBox,
    state: Rc<RefCell<AppState>>,
    bluetooth_geiger_state: Rc<RefCell<BluetoothGeigerUiState>>,
) {
    let popover = Popover::new();
    popover.set_parent(bluetooth_list);
    let box_ = GtkBox::new(Orientation::Vertical, 4);
    let view_btn = Button::with_label("View Details");
    let locate_btn = Button::with_label("Locate Device");
    let enumerate_btn = Button::with_label("Connect & Enumerate");
    let disconnect_btn = Button::with_label("Disconnect");
    box_.append(&view_btn);
    box_.append(&locate_btn);
    box_.append(&enumerate_btn);
    box_.append(&disconnect_btn);
    popover.set_child(Some(&box_));

    {
        let window = window.clone();
        let state = state.clone();
        let bluetooth_list = bluetooth_list.clone();
        view_btn.connect_clicked(move |_| {
            if let Some(device) = selected_bluetooth(&state, &bluetooth_list) {
                open_bluetooth_details_dialog(&window, &device);
            }
        });
    }

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
            let device_mac = device.mac.clone();
            let (controller, sender) = {
                let s = state.borrow();
                (
                    s.settings.bluetooth_controller.clone(),
                    s.bluetooth_sender.clone(),
                )
            };
            {
                let mut s = state.borrow_mut();
                s.set_bluetooth_enumeration_status(
                    device_mac.clone(),
                    format!("Enumerating {}...", device_mac),
                    false,
                );
                s.push_status(format!(
                    "starting active bluetooth enumeration for {}",
                    device_mac
                ));
            }
            thread::spawn(move || {
                match bluetooth::connect_and_enumerate_device(controller.as_deref(), &device_mac) {
                    Ok(record) => {
                        let status = bluetooth_enumeration_status_from_device(&record).unwrap_or(
                            BluetoothEnumerationStatus {
                                message: format!("Enumeration complete for {}", record.mac),
                                is_error: false,
                            },
                        );
                        let log_line = if status.is_error {
                            format!(
                                "active bluetooth enumeration completed with warning for {}: {}",
                                record.mac, status.message
                            )
                        } else {
                            format!("active bluetooth enumeration completed for {}", record.mac)
                        };
                        let mac = record.mac.clone();
                        let _ = sender.send(BluetoothEvent::DeviceSeen(record));
                        let _ = sender.send(BluetoothEvent::EnumerationStatus {
                            mac,
                            message: status.message,
                            is_error: status.is_error,
                        });
                        let _ = sender.send(BluetoothEvent::Log(log_line));
                    }
                    Err(err) => {
                        let message = format!("Enumeration failed for {}: {}", device_mac, err);
                        let _ = sender.send(BluetoothEvent::EnumerationStatus {
                            mac: device_mac.clone(),
                            message,
                            is_error: true,
                        });
                        let _ = sender.send(BluetoothEvent::Log(format!(
                            "active bluetooth enumeration failed for {}: {err}",
                            device_mac
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

fn open_bluetooth_details_dialog(window: &ApplicationWindow, device: &BluetoothDeviceRecord) {
    let dialog = Dialog::builder()
        .transient_for(window)
        .modal(true)
        .title("Bluetooth Details")
        .default_width(760)
        .default_height(560)
        .build();

    dialog.add_button("Close", ResponseType::Close);
    let area = dialog.content_area();
    let detail_text = format!(
        "Identity\n{}\n\nPassive Broadcast Data\n{}\n\nActive Enumeration Summary\n{}\n\nReadable Attributes\n{}\n\nServices\n{}\n\nCharacteristics\n{}\n\nDescriptors\n{}",
        format_bluetooth_identity_section(device),
        format_bluetooth_passive_section(device),
        format_bluetooth_active_summary(device),
        format_bluetooth_readable_attributes(device),
        format_bluetooth_services(device),
        format_bluetooth_characteristics(device),
        format_bluetooth_descriptors(device)
    );
    let label = Label::new(Some(&detail_text));
    label.set_xalign(0.0);
    label.set_yalign(0.0);
    label.set_wrap(true);
    label.set_selectable(true);
    let scroll = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&label)
        .build();
    area.append(&scroll);
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

fn populate_bluetooth_controller_combo(combo: &ComboBoxText, preferred: Option<&str>) {
    combo.remove_all();
    combo.append(Some("default"), "Default Controller");

    let mut known_ids = HashSet::new();
    for ctrl in bluetooth::list_controllers().unwrap_or_default() {
        known_ids.insert(ctrl.id.clone());
        combo.append(
            Some(&ctrl.id),
            &format!(
                "{}{} ({}){}",
                ctrl.id,
                ctrl.adapter
                    .as_deref()
                    .map(|adapter| format!(" [{}]", adapter))
                    .unwrap_or_default(),
                if ctrl.name.is_empty() {
                    "unnamed"
                } else {
                    ctrl.name.as_str()
                },
                if ctrl.is_default { " [default]" } else { "" }
            ),
        );
    }

    let preferred = preferred
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_uppercase());

    if let Some(controller_id) = preferred {
        if !known_ids.contains(&controller_id) {
            combo.append(
                Some(&controller_id),
                &format!("{controller_id} [saved controller]"),
            );
        }
        if !combo.set_active_id(Some(&controller_id)) {
            combo.set_active_id(Some("default"));
        }
    } else {
        combo.set_active_id(Some("default"));
    }
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
                        enabled: true,
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

fn fallback_frequency_mhz_for_channel(channel: u16) -> Option<u32> {
    match channel {
        1..=13 => Some(2407 + (channel as u32 * 5)),
        14 => Some(2484),
        15..=177 => Some(5000 + (channel as u32 * 5)),
        178..=233 => Some(5950 + (channel as u32 * 5)),
        _ => None,
    }
}

fn channel_frequency_mhz(channel: &capture::SupportedChannel) -> Option<u32> {
    channel
        .frequency_mhz
        .or_else(|| fallback_frequency_mhz_for_channel(channel.channel))
}

fn channel_capability_band(channel: &capture::SupportedChannel) -> SpectrumBand {
    let by_freq = SpectrumBand::from_frequency_mhz(channel_frequency_mhz(channel));
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
    if channel.channel == 14 || channel_frequency_mhz(channel) == Some(2484) {
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
    filtered.sort_by_key(|c| (channel_frequency_mhz(c).unwrap_or(0), c.channel, !c.enabled));
    filtered.dedup_by(|a, b| {
        a.channel == b.channel
            && channel_frequency_mhz(a) == channel_frequency_mhz(b)
            && a.enabled == b.enabled
    });
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
    const CAP_COL_CHANNEL: i32 = 12;
    const CAP_COL_FREQ: i32 = 14;
    const CAP_COL_BAND: i32 = 12;
    const CAP_COL_WIDTHS: i32 = 44;

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
    header.append(&header_cell("Channel".to_string(), CAP_COL_CHANNEL));
    header.append(&header_cell("Freq MHz".to_string(), CAP_COL_FREQ));
    header.append(&header_cell("Band".to_string(), CAP_COL_BAND));
    header.append(&header_cell(
        "Bandwidth / Modes".to_string(),
        CAP_COL_WIDTHS,
    ));
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
            row.append(&label_cell(ch.channel.to_string(), CAP_COL_CHANNEL));
            row.append(&label_cell(
                channel_frequency_mhz(ch)
                    .map(|f| f.to_string())
                    .unwrap_or_default(),
                CAP_COL_FREQ,
            ));
            row.append(&label_cell(channel_capability_band_label(ch), CAP_COL_BAND));
            row.append(&label_cell(
                if ch.enabled {
                    widths.clone()
                } else {
                    format!("{widths} | disabled")
                },
                CAP_COL_WIDTHS,
            ));
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
    wifi_enabled: bool,
    bluetooth_enabled: bool,
    bluetooth_controller: Option<String>,
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
        enabled: wifi_enabled,
    }];
    s.settings.bluetooth_enabled = bluetooth_enabled;
    s.settings.bluetooth_controller = bluetooth_controller;
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
    let root_scrolled = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&root)
        .build();
    settings_window.set_child(Some(&root_scrolled));

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

    let dwell_entry = Entry::new();
    dwell_entry.set_placeholder_text(Some("Dwell ms (200 = 5 ch/sec)"));
    dwell_entry.set_text("200");

    let channels_entry = Entry::new();
    channels_entry.set_placeholder_text(Some("1,6,11,36,40,44,48"));
    channels_entry.set_visible(false);

    let select_all_channels_btn = Button::with_label("Select All");
    let clear_channels_btn = Button::with_label("Clear");
    let show_channels_btn = Button::with_label("Show Device Channels");
    let channel_checks = Rc::new(RefCell::new(Vec::<(u16, CheckButton)>::new()));
    let hop_channel_mode_controls =
        Rc::new(RefCell::new(HashMap::<u16, Vec<(String, ToggleButton)>>::new()));
    let hop_channel_mode_overrides =
        Rc::new(RefCell::new(BTreeMap::<u16, Vec<String>>::new()));
    let channels_list = GtkBox::new(Orientation::Vertical, 4);
    let channels_scrolled = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .min_content_height(220)
        .child(&channels_list)
        .build();

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
    lock_ht_combo.set_visible(false);
    let lock_ht_buttons = Rc::new(RefCell::new(Vec::<(String, ToggleButton)>::new()));
    let lock_ht_button_box = GtkBox::new(Orientation::Horizontal, 6);
    let hop_ht_combo = ComboBoxText::new();
    hop_ht_combo.append(Some("HT20"), "HT20");
    hop_ht_combo.append(Some("HT40+"), "HT40+");
    hop_ht_combo.append(Some("HT40-"), "HT40-");
    hop_ht_combo.set_active_id(Some("HT20"));
    hop_ht_combo.set_visible(false);
    let hop_ht_buttons = Rc::new(RefCell::new(Vec::<(String, ToggleButton)>::new()));
    let hop_ht_button_box = GtkBox::new(Orientation::Horizontal, 6);

    let wifi_scan_check = CheckButton::with_label("Scan Wi-Fi");
    let bluetooth_scan_check = CheckButton::with_label("Scan Bluetooth");
    let bluetooth_controller_combo = ComboBoxText::new();

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

    let channels_row = GtkBox::new(Orientation::Horizontal, 8);
    let channels_label = Label::new(Some("Channel Selector"));
    channels_label.set_xalign(0.0);
    channels_label.set_width_chars(18);
    channels_row.append(&channels_label);
    channels_row.append(&select_all_channels_btn);
    channels_row.append(&clear_channels_btn);
    channels_row.append(&show_channels_btn);
    let channels_table_row = GtkBox::new(Orientation::Vertical, 6);
    channels_table_row.append(&channels_row);
    channels_table_row.append(&channels_scrolled);

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
    let ht_label = Label::new(Some("Bandwidth"));
    ht_label.set_xalign(0.0);
    ht_label.set_width_chars(18);
    ht_row.append(&ht_label);
    ht_row.append(&lock_ht_button_box);

    let wifi_row = GtkBox::new(Orientation::Horizontal, 8);
    let wifi_label = Label::new(Some("Wi-Fi"));
    wifi_label.set_xalign(0.0);
    wifi_label.set_width_chars(18);
    wifi_row.append(&wifi_label);
    wifi_row.append(&wifi_scan_check);

    let bluetooth_row = GtkBox::new(Orientation::Horizontal, 8);
    let bluetooth_label = Label::new(Some("Bluetooth"));
    bluetooth_label.set_xalign(0.0);
    bluetooth_label.set_width_chars(18);
    bluetooth_row.append(&bluetooth_label);
    bluetooth_row.append(&bluetooth_scan_check);

    let bluetooth_controller_row = GtkBox::new(Orientation::Horizontal, 8);
    let bluetooth_controller_label = Label::new(Some("Bluetooth Radio"));
    bluetooth_controller_label.set_xalign(0.0);
    bluetooth_controller_label.set_width_chars(18);
    bluetooth_controller_row.append(&bluetooth_controller_label);
    bluetooth_controller_row.append(&bluetooth_controller_combo);

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
    root.append(&channels_table_row);
    root.append(&dwell_row);
    root.append(&band_row);
    root.append(&lock_row);
    root.append(&ht_row);
    root.append(&wifi_row);
    root.append(&bluetooth_row);
    root.append(&bluetooth_controller_row);
    root.append(&output_toggle_row);
    root.append(&output_dir_row);
    root.append(&action_row);

    let sync_channels_entry_from_checks = Rc::new(RefCell::new(None::<Box<dyn Fn()>>));
    {
        let channel_checks = channel_checks.clone();
        let channels_entry = channels_entry.clone();
        let sync = sync_channels_entry_from_checks.clone();
        *sync.borrow_mut() = Some(Box::new(move || {
            let mut selected = channel_checks
                .borrow()
                .iter()
                .filter_map(|(ch, cb)| if cb.is_active() { Some(*ch) } else { None })
                .collect::<Vec<_>>();
            selected.sort_unstable();
            channels_entry.set_text(
                &selected
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }));
    }

    let rebuild_lock_ht_buttons = Rc::new(RefCell::new(None::<Box<dyn Fn(Vec<String>)>>));
    {
        let lock_ht_combo = lock_ht_combo.clone();
        let lock_ht_button_box = lock_ht_button_box.clone();
        let lock_ht_buttons = lock_ht_buttons.clone();
        let rebuild = rebuild_lock_ht_buttons.clone();
        *rebuild.borrow_mut() = Some(Box::new(move |choices: Vec<String>| {
            while let Some(child) = lock_ht_button_box.first_child() {
                lock_ht_button_box.remove(&child);
            }
            lock_ht_buttons.borrow_mut().clear();

            let current = lock_ht_combo
                .active_id()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "HT20".to_string());
            let mut modes = choices;
            if !modes.iter().any(|m| m == "HT20") {
                modes.insert(0, "HT20".to_string());
            }
            modes.sort();
            modes.dedup();

            for mode in modes {
                let button = ToggleButton::with_label(&format!("[{}]", mode));
                lock_ht_button_box.append(&button);
                lock_ht_buttons
                    .borrow_mut()
                    .push((mode.clone(), button.clone()));
            }

            let buttons = lock_ht_buttons.borrow().clone();
            for (mode, button) in buttons.iter() {
                let peers = lock_ht_buttons.borrow().clone();
                let mode = mode.clone();
                let lock_ht_combo = lock_ht_combo.clone();
                button.connect_toggled(move |btn| {
                    if btn.is_active() {
                        for (_, peer) in peers.iter() {
                            if peer.as_ptr() != btn.as_ptr() {
                                peer.set_active(false);
                            }
                        }
                        lock_ht_combo.set_active_id(Some(&mode));
                    } else if !peers.iter().any(|(_, peer)| peer.is_active()) {
                        btn.set_active(true);
                    }
                });
            }

            if !lock_ht_combo.set_active_id(Some(&current)) {
                lock_ht_combo.set_active_id(Some("HT20"));
            }
            let active = lock_ht_combo
                .active_id()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "HT20".to_string());
            for (mode, button) in lock_ht_buttons.borrow().iter() {
                button.set_active(mode == &active);
            }
        }));
    }

    let rebuild_hop_ht_buttons = Rc::new(RefCell::new(None::<Box<dyn Fn(Vec<String>)>>));
    {
        let hop_ht_combo = hop_ht_combo.clone();
        let hop_ht_button_box = hop_ht_button_box.clone();
        let hop_ht_buttons = hop_ht_buttons.clone();
        let rebuild = rebuild_hop_ht_buttons.clone();
        *rebuild.borrow_mut() = Some(Box::new(move |choices: Vec<String>| {
            while let Some(child) = hop_ht_button_box.first_child() {
                hop_ht_button_box.remove(&child);
            }
            hop_ht_buttons.borrow_mut().clear();

            let current = hop_ht_combo
                .active_id()
                .map(|v| v.to_string())
                .unwrap_or_else(default_hop_ht_mode);
            let mut modes = choices;
            if !modes.iter().any(|m| m == "HT20") {
                modes.insert(0, "HT20".to_string());
            }
            modes.sort();
            modes.dedup();

            for mode in modes {
                let button = ToggleButton::with_label(&format!("[{}]", mode));
                hop_ht_button_box.append(&button);
                hop_ht_buttons
                    .borrow_mut()
                    .push((mode.clone(), button.clone()));
            }

            let buttons = hop_ht_buttons.borrow().clone();
            for (mode, button) in buttons.iter() {
                let peers = hop_ht_buttons.borrow().clone();
                let mode = mode.clone();
                let hop_ht_combo = hop_ht_combo.clone();
                button.connect_toggled(move |btn| {
                    if btn.is_active() {
                        for (_, peer) in peers.iter() {
                            if peer.as_ptr() != btn.as_ptr() {
                                peer.set_active(false);
                            }
                        }
                        hop_ht_combo.set_active_id(Some(&mode));
                    } else if !peers.iter().any(|(_, peer)| peer.is_active()) {
                        btn.set_active(true);
                    }
                });
            }

            if !hop_ht_combo.set_active_id(Some(&current)) {
                hop_ht_combo.set_active_id(Some("HT20"));
            }
            let active = hop_ht_combo
                .active_id()
                .map(|v| v.to_string())
                .unwrap_or_else(default_hop_ht_mode);
            for (mode, button) in hop_ht_buttons.borrow().iter() {
                button.set_active(mode == &active);
            }
        }));
    }

    let rebuild_channel_table = Rc::new(RefCell::new(
        None::<Box<dyn Fn(Vec<capture::SupportedChannel>, Vec<String>)>>,
    ));
    {
        const SEL_COL_USE: i32 = 8;
        const SEL_COL_CHANNEL: i32 = 12;
        const SEL_COL_FREQ: i32 = 14;
        const SEL_COL_BAND: i32 = 12;
        const SEL_COL_MODE: i32 = 20;

        let channels_list = channels_list.clone();
        let channel_checks = channel_checks.clone();
        let hop_channel_mode_controls = hop_channel_mode_controls.clone();
        let hop_channel_mode_overrides = hop_channel_mode_overrides.clone();
        let channels_entry = channels_entry.clone();
        let lock_channel_entry = lock_channel_entry.clone();
        let hop_ht_combo = hop_ht_combo.clone();
        let sync_channels_entry_from_checks = sync_channels_entry_from_checks.clone();
        let rebuild = rebuild_channel_table.clone();
        *rebuild.borrow_mut() = Some(Box::new(
            move |channels: Vec<capture::SupportedChannel>, ht_choices: Vec<String>| {
                while let Some(child) = channels_list.first_child() {
                    channels_list.remove(&child);
                }
                channel_checks.borrow_mut().clear();
                hop_channel_mode_controls.borrow_mut().clear();

                let selected_seed = channels_entry
                    .text()
                    .split(',')
                    .filter_map(|v| v.trim().parse::<u16>().ok())
                    .collect::<HashSet<_>>();

                let header = GtkBox::new(Orientation::Horizontal, 10);
                header.append(&header_cell("Use".to_string(), SEL_COL_USE));
                header.append(&header_cell("Channel".to_string(), SEL_COL_CHANNEL));
                header.append(&header_cell("Freq MHz".to_string(), SEL_COL_FREQ));
                header.append(&header_cell("Band".to_string(), SEL_COL_BAND));
                header.append(&header_cell("Bandwidth".to_string(), SEL_COL_MODE));
                channels_list.append(&header);

                if channels.is_empty() {
                    let empty = Label::new(Some("No channel capability data available."));
                    empty.set_xalign(0.0);
                    channels_list.append(&empty);
                    return;
                }

                let mut default_lock_channel: Option<u16> = None;
                for ch in channels {
                    let row = GtkBox::new(Orientation::Horizontal, 10);
                    let check = CheckButton::new();
                    let should_default_select = if selected_seed.is_empty() {
                        ch.enabled
                    } else {
                        selected_seed.contains(&ch.channel)
                    };
                    check.set_active(should_default_select);
                    check.set_sensitive(ch.enabled);
                    check.set_halign(gtk::Align::Center);
                    let check_cell = GtkBox::new(Orientation::Horizontal, 0);
                    check_cell.set_halign(gtk::Align::Center);
                    check_cell.set_size_request(SEL_COL_USE * TABLE_CHAR_WIDTH_PX, -1);
                    check_cell.append(&check);
                    row.append(&check_cell);
                    row.append(&label_cell(ch.channel.to_string(), SEL_COL_CHANNEL));
                    row.append(&label_cell(
                        channel_frequency_mhz(&ch)
                            .map(|f| f.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        SEL_COL_FREQ,
                    ));
                    row.append(&label_cell(
                        channel_capability_band_label(&ch),
                        SEL_COL_BAND,
                    ));
                    let mode_box = GtkBox::new(Orientation::Horizontal, 4);
                    let default_modes = hop_channel_mode_overrides
                        .borrow()
                        .get(&ch.channel)
                        .cloned()
                        .unwrap_or_else(|| {
                            vec![
                                hop_ht_combo
                                    .active_id()
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(default_hop_ht_mode),
                            ]
                        });
                    let mut row_mode_buttons = Vec::<(String, ToggleButton)>::new();
                    for mode in &ht_choices {
                        let button = ToggleButton::with_label(&format!("[{}]", mode));
                        button.set_sensitive(ch.enabled);
                        button.set_active(default_modes.contains(mode));
                        mode_box.append(&button);
                        row_mode_buttons.push((mode.clone(), button.clone()));
                    }
                    if row_mode_buttons.iter().all(|(_, button)| !button.is_active()) {
                        if let Some((_, first)) = row_mode_buttons.first() {
                            first.set_active(true);
                        }
                    }
                    for (_mode, button) in row_mode_buttons.iter() {
                        let peers = row_mode_buttons.clone();
                        let hop_channel_mode_overrides = hop_channel_mode_overrides.clone();
                        let channel = ch.channel;
                        button.connect_toggled(move |_| {
                            let mut selected = peers
                                .iter()
                                .filter_map(|(candidate_mode, candidate_button)| {
                                    if candidate_button.is_active() {
                                        Some(candidate_mode.clone())
                                    } else {
                                        None
                                    }
                                })
                                .collect::<Vec<_>>();
                            if selected.is_empty() {
                                if let Some((_, first)) = peers.first() {
                                    first.set_active(true);
                                }
                                selected = peers
                                    .iter()
                                    .filter_map(|(candidate_mode, candidate_button)| {
                                        if candidate_button.is_active() {
                                            Some(candidate_mode.clone())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect::<Vec<_>>();
                            }
                            selected.sort();
                            selected.dedup();
                            hop_channel_mode_overrides
                                .borrow_mut()
                                .insert(channel, selected);
                        });
                    }
                    let mut initial_selected = row_mode_buttons
                        .iter()
                        .filter_map(|(mode, button)| if button.is_active() { Some(mode.clone()) } else { None })
                        .collect::<Vec<_>>();
                    initial_selected.sort();
                    initial_selected.dedup();
                    hop_channel_mode_overrides
                        .borrow_mut()
                        .insert(ch.channel, initial_selected);
                    let mode_cell = GtkBox::new(Orientation::Horizontal, 0);
                    mode_cell.set_size_request(SEL_COL_MODE * TABLE_CHAR_WIDTH_PX, -1);
                    mode_cell.append(&mode_box);
                    row.append(&mode_cell);

                    let sync = sync_channels_entry_from_checks.clone();
                    check.connect_toggled(move |_| {
                        if let Some(cb) = sync.borrow().as_ref() {
                            cb();
                        }
                    });

                    if should_default_select && default_lock_channel.is_none() {
                        default_lock_channel = Some(ch.channel);
                    }
                    hop_channel_mode_controls
                        .borrow_mut()
                        .insert(ch.channel, row_mode_buttons);
                    channel_checks.borrow_mut().push((ch.channel, check));
                    channels_list.append(&row);
                }

                if let Some(cb) = sync_channels_entry_from_checks.borrow().as_ref() {
                    cb();
                }
                if lock_channel_entry.text().trim().is_empty() {
                    if let Some(ch) = default_lock_channel {
                        lock_channel_entry.set_text(&ch.to_string());
                    }
                }
            },
        ));
    }

    let apply_interface_capability = Rc::new(RefCell::new(None::<Box<dyn Fn()>>));
    {
        let caps = capabilities_rc.clone();
        let interface_combo = interface_combo.clone();
        let channels_entry = channels_entry.clone();
        let interface_status = interface_status.clone();
        let lock_ht_combo = lock_ht_combo.clone();
        let band_combo = band_combo.clone();
        let rebuild_channel_table = rebuild_channel_table.clone();
        let rebuild_lock_ht_buttons = rebuild_lock_ht_buttons.clone();
        let hop_ht_combo = hop_ht_combo.clone();
        let rebuild_hop_ht_buttons = rebuild_hop_ht_buttons.clone();
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
                        .filter(|ch| ch.enabled)
                        .map(|c| c.channel.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    channels_entry.set_text(&default_channels);
                }

                let discovered_enabled = cap.channels.iter().filter(|ch| ch.enabled).count();
                let discovered_disabled = cap.channels.iter().filter(|ch| !ch.enabled).count();

                interface_status.set_text(&format!(
                    "Selected {} | monitor mode: {} | channels: {} enabled / {} disabled | modes: {}",
                    cap.interface_name,
                    if cap.monitor_capable { "yes" } else { "no" },
                    discovered_enabled,
                    discovered_disabled,
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
                let current_hop_ht = hop_ht_combo
                    .active_id()
                    .map(|v| v.to_string())
                    .unwrap_or_else(default_hop_ht_mode);
                let ht_choices = lock_ht_mode_choices_from_capability(&cap.ht_modes);
                lock_ht_combo.remove_all();
                hop_ht_combo.remove_all();
                for mode in &ht_choices {
                    lock_ht_combo.append(Some(mode), mode);
                    hop_ht_combo.append(Some(mode), mode);
                }
                if ht_choices.iter().any(|m| m == &current_ht) {
                    lock_ht_combo.set_active_id(Some(&current_ht));
                } else {
                    lock_ht_combo.set_active_id(Some("HT20"));
                }
                if ht_choices.iter().any(|m| m == &current_hop_ht) {
                    hop_ht_combo.set_active_id(Some(&current_hop_ht));
                } else {
                    hop_ht_combo.set_active_id(Some("HT20"));
                }
                if let Some(render_ht) = rebuild_lock_ht_buttons.borrow().as_ref() {
                    render_ht(ht_choices.clone());
                }
                if let Some(render_ht) = rebuild_hop_ht_buttons.borrow().as_ref() {
                    render_ht(ht_choices.clone());
                }
                if let Some(render_channels) = rebuild_channel_table.borrow().as_ref() {
                    render_channels(cap.channels.clone(), ht_choices);
                }
            } else {
                interface_status.set_text("No interface capability data available.");
            }
        }));
    }

    let update_mode_visibility = Rc::new(RefCell::new(None::<Box<dyn Fn()>>));
    {
        let mode_combo = mode_combo.clone();
        let mode_row = mode_row.clone();
        let channels_table_row = channels_table_row.clone();
        let dwell_row = dwell_row.clone();
        let band_row = band_row.clone();
        let lock_row = lock_row.clone();
        let ht_row = ht_row.clone();
        let wifi_scan_check = wifi_scan_check.clone();
        let update_mode = update_mode_visibility.clone();
        *update_mode.borrow_mut() = Some(Box::new(move || {
            let wifi_enabled = wifi_scan_check.is_active();
            mode_row.set_visible(wifi_enabled);
            if !wifi_enabled {
                channels_table_row.set_visible(false);
                dwell_row.set_visible(false);
                band_row.set_visible(false);
                lock_row.set_visible(false);
                ht_row.set_visible(false);
                return;
            }
            let mode = mode_combo
                .active_id()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "hop_specific".to_string());
            match mode.as_str() {
                "hop_band" => {
                    channels_table_row.set_visible(false);
                    dwell_row.set_visible(false);
                    band_row.set_visible(true);
                    lock_row.set_visible(false);
                    ht_row.set_visible(false);
                }
                "locked" => {
                    channels_table_row.set_visible(false);
                    dwell_row.set_visible(false);
                    band_row.set_visible(false);
                    lock_row.set_visible(true);
                    ht_row.set_visible(true);
                }
                _ => {
                    channels_table_row.set_visible(true);
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
    }

    {
        let channel_checks = channel_checks.clone();
        let sync = sync_channels_entry_from_checks.clone();
        select_all_channels_btn.connect_clicked(move |_| {
            for (_, check) in channel_checks.borrow().iter() {
                if check.is_sensitive() {
                    check.set_active(true);
                }
            }
            if let Some(cb) = sync.borrow().as_ref() {
                cb();
            }
        });
    }

    {
        let channel_checks = channel_checks.clone();
        let sync = sync_channels_entry_from_checks.clone();
        clear_channels_btn.connect_clicked(move |_| {
            for (_, check) in channel_checks.borrow().iter() {
                check.set_active(false);
            }
            if let Some(cb) = sync.borrow().as_ref() {
                cb();
            }
        });
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

    {
        let update_mode_visibility = update_mode_visibility.clone();
        wifi_scan_check.connect_toggled(move |_| {
            if let Some(cb) = update_mode_visibility.borrow().as_ref() {
                cb();
            }
        });
    }

    {
        let bluetooth_controller_combo = bluetooth_controller_combo.clone();
        bluetooth_scan_check.connect_toggled(move |check| {
            bluetooth_controller_combo.set_sensitive(check.is_active());
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

    {
        let hop_channel_mode_controls = hop_channel_mode_controls.clone();
        let hop_channel_mode_overrides = hop_channel_mode_overrides.clone();
        hop_ht_combo.connect_changed(move |combo| {
            let Some(selected) = combo.active_id().map(|v| v.to_string()) else {
                return;
            };
            for (channel, controls) in hop_channel_mode_controls.borrow().iter() {
                let mut applied = false;
                for (mode, button) in controls.iter() {
                    if button.is_sensitive() {
                        let make_active = mode == &selected;
                        button.set_active(make_active);
                        if make_active {
                            applied = true;
                        }
                    }
                }
                if applied {
                    hop_channel_mode_overrides
                        .borrow_mut()
                        .insert(*channel, vec![selected.clone()]);
                }
            }
        });
    }

    let current_interface = {
        let s = state.borrow();
        (
            s.settings.interfaces.first().cloned(),
            s.settings.interfaces.iter().any(|iface| iface.enabled),
            s.settings.bluetooth_enabled,
            s.settings.bluetooth_controller.clone(),
            s.settings.output_to_files,
            s.settings.output_root.clone(),
        )
    };

    if let Some(iface) = &current_interface.0 {
        if !iface.interface_name.is_empty() {
            interface_combo.set_active_id(Some(&iface.interface_name));
        }
        match &iface.channel_mode {
            ChannelSelectionMode::HopAll {
                channels,
                dwell_ms,
                ht_mode,
                channel_ht_modes,
            } => {
                mode_combo.set_active_id(Some("hop_specific"));
                channels_entry.set_text(
                    &channels
                        .iter()
                        .map(|c| c.to_string())
                        .collect::<Vec<_>>()
                        .join(","),
                );
                dwell_entry.set_text(&dwell_ms.to_string());
                hop_ht_combo.set_active_id(Some(ht_mode));
                *hop_channel_mode_overrides.borrow_mut() = channel_ht_modes.clone();
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

    wifi_scan_check.set_active(current_interface.1);
    bluetooth_scan_check.set_active(current_interface.2);
    populate_bluetooth_controller_combo(
        &bluetooth_controller_combo,
        current_interface.3.as_deref(),
    );
    bluetooth_controller_combo.set_sensitive(current_interface.2);
    output_to_files_check.set_active(current_interface.4);
    output_dir_entry.set_text(&current_interface.5.display().to_string());
    output_dir_entry.set_sensitive(current_interface.4);
    browse_output_btn.set_sensitive(current_interface.4);

    if let Some(cb) = apply_interface_capability.borrow().as_ref() {
        cb();
    }
    if let Some(cb) = update_mode_visibility.borrow().as_ref() {
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
        let channels_entry = channels_entry.clone();
        let dwell_entry = dwell_entry.clone();
        let band_combo = band_combo.clone();
        let lock_channel_entry = lock_channel_entry.clone();
        let lock_ht_combo = lock_ht_combo.clone();
        let hop_ht_combo = hop_ht_combo.clone();
        let hop_channel_mode_controls = hop_channel_mode_controls.clone();
        let hop_channel_mode_overrides = hop_channel_mode_overrides.clone();
        let wifi_scan_check = wifi_scan_check.clone();
        let bluetooth_scan_check = bluetooth_scan_check.clone();
        let bluetooth_controller_combo = bluetooth_controller_combo.clone();
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
            let hop_ht_mode = hop_ht_combo
                .active_id()
                .map(|v| v.to_string())
                .unwrap_or_else(default_hop_ht_mode);
            let wifi_enabled = wifi_scan_check.is_active();
            let bluetooth_enabled = bluetooth_scan_check.is_active();
            let bluetooth_controller = if !bluetooth_enabled {
                None
            } else {
                match bluetooth_controller_combo.active_id().as_deref() {
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
                            enabled: true,
                        })
                        .collect()
                });
            let selectable_channel_details = all_channel_details
                .iter()
                .filter(|ch| ch.enabled)
                .cloned()
                .collect::<Vec<_>>();
            let all_channels = selectable_channel_details
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
                    let band_channels =
                        filter_channels_for_band(&selectable_channel_details, &band);
                    if band_channels.is_empty() {
                        let mut s = state.borrow_mut();
                        s.push_status(format!(
                            "selected band {} has no enabled channels on {}",
                            band.label(),
                            iface_name
                        ));
                    }
                    ChannelSelectionMode::HopBand {
                        band,
                        channels: band_channels,
                        dwell_ms,
                    }
                }
                _ => ChannelSelectionMode::HopAll {
                    channels: {
                        if sanitized_parsed_channels.is_empty() {
                            all_channels.clone()
                        } else {
                            sanitized_parsed_channels.clone()
                        }
                    },
                    dwell_ms,
                    ht_mode: hop_ht_mode.clone(),
                    channel_ht_modes: {
                        let selected_channels = if sanitized_parsed_channels.is_empty() {
                            all_channels.clone()
                        } else {
                            sanitized_parsed_channels.clone()
                        };
                        let mut per_channel = BTreeMap::new();
                        for channel in selected_channels {
                            let mut modes = hop_channel_mode_controls
                                .borrow()
                                .get(&channel)
                                .map(|controls| {
                                    controls
                                        .iter()
                                        .filter_map(|(mode, button)| {
                                            if button.is_active() {
                                                Some(mode.clone())
                                            } else {
                                                None
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                })
                                .or_else(|| {
                                    hop_channel_mode_overrides.borrow().get(&channel).cloned()
                                })
                                .unwrap_or_else(|| vec![hop_ht_mode.clone()]);
                            modes.sort();
                            modes.dedup();
                            if modes.is_empty() {
                                modes.push(hop_ht_mode.clone());
                            }
                            per_channel.insert(channel, modes);
                        }
                        per_channel
                    },
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
                s.push_status(format!(
                    "preparing scan setup on {} (Wi-Fi: {}, Bluetooth: {})",
                    iface_name,
                    if wifi_enabled { "on" } else { "off" },
                    if bluetooth_enabled { "on" } else { "off" }
                ));
            }

            settings_window.close();
            apply_interface_selection(
                state.clone(),
                iface_name,
                mode,
                wifi_enabled,
                bluetooth_enabled,
                bluetooth_controller,
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
    let default_geoip_path = settings_snapshot.geoip_city_db_path.clone();
    let default_oui_path = settings_snapshot.oui_source_path.clone();
    let show_status_bar_check = CheckButton::with_label("Status Pane");
    show_status_bar_check.set_active(settings_snapshot.show_status_bar);
    let show_detail_pane_check = CheckButton::with_label("Details Pane");
    show_detail_pane_check.set_active(settings_snapshot.show_detail_pane);
    let show_device_pane_check = CheckButton::with_label("Device Pane");
    show_device_pane_check.set_active(settings_snapshot.show_device_pane);
    let dark_mode_check = CheckButton::with_label("Dark Mode");
    dark_mode_check.set_active(settings_snapshot.dark_mode);

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
    view_page.append(&dark_mode_check);

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
    populate_bluetooth_controller_combo(
        &bluetooth_controller_combo,
        settings_snapshot.bluetooth_controller.as_deref(),
    );
    bluetooth_controller_combo.set_sensitive(settings_snapshot.bluetooth_enabled);

    let bluetooth_timeout_entry = Entry::new();
    bluetooth_timeout_entry.set_text(&settings_snapshot.bluetooth_scan_timeout_secs.to_string());
    let bluetooth_pause_entry = Entry::new();
    bluetooth_pause_entry.set_text(&settings_snapshot.bluetooth_scan_pause_ms.to_string());

    for (label_text, widget) in [
        (
            "Bluetooth Radio",
            bluetooth_controller_combo.upcast_ref::<gtk::Widget>(),
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
        let bluetooth_controller_combo = bluetooth_controller_combo.clone();
        bluetooth_enabled_check.connect_toggled(move |check| {
            bluetooth_controller_combo.set_sensitive(check.is_active());
        });
    }

    let data_sources_page = page(&stack, "data_sources", "Data Sources");
    data_sources_page.append(&section_heading("Data Sources"));
    let geoip_row = GtkBox::new(Orientation::Horizontal, 8);
    let geoip_label = Label::new(Some("GeoIP Lookup File"));
    geoip_label.set_width_chars(24);
    geoip_label.set_xalign(0.0);
    let geoip_entry = Entry::new();
    geoip_entry.set_hexpand(true);
    geoip_entry.set_text(&settings_snapshot.geoip_city_db_path.display().to_string());
    let geoip_browse_btn = Button::with_label("Browse");
    geoip_row.append(&geoip_label);
    geoip_row.append(&geoip_entry);
    geoip_row.append(&geoip_browse_btn);
    data_sources_page.append(&geoip_row);

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
        let geoip_entry = geoip_entry.clone();
        geoip_browse_btn.connect_clicked(move |_| {
            let current = geoip_entry.text().to_string();
            let initial = if current.trim().is_empty() {
                PathBuf::from(".")
            } else {
                PathBuf::from(current)
            };
            let geoip_entry = geoip_entry.clone();
            choose_file_path(
                &dialog,
                "Select GeoIP Lookup File",
                initial,
                move |selected| {
                    if let Some(path) = selected {
                        geoip_entry.set_text(&path.display().to_string());
                    }
                },
            );
        });
    }

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
        let default_geoip_path = default_geoip_path.clone();
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

                let bluetooth_enabled = bluetooth_enabled_check.is_active();
                let bluetooth_controller = if !bluetooth_enabled {
                    None
                } else {
                    match bluetooth_controller_combo.active_id().as_deref() {
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

                let geoip_path_text = geoip_entry.text().to_string();
                let geoip_path = if geoip_path_text.trim().is_empty() {
                    default_geoip_path.clone()
                } else {
                    PathBuf::from(geoip_path_text.trim())
                };
                let oui_path_text = oui_entry.text().to_string();
                let oui_path = if oui_path_text.trim().is_empty() {
                    default_oui_path.clone()
                } else {
                    PathBuf::from(oui_path_text.trim())
                };

                let mut full_restart_needed = false;
                let mut bluetooth_restart_needed = false;
                let mut applied_messages = Vec::new();
                let mut dark_mode_changed = false;

                let view_changed = {
                    let mut s = state.borrow_mut();
                    let view_changed = s.settings.show_status_bar
                        != show_status_bar_check.is_active()
                        || s.settings.show_detail_pane != show_detail_pane_check.is_active()
                        || s.settings.show_device_pane != show_device_pane_check.is_active();
                    if s.settings.dark_mode != dark_mode_check.is_active() {
                        s.settings.dark_mode = dark_mode_check.is_active();
                        dark_mode_changed = true;
                        applied_messages.push(format!(
                            "dark mode {}",
                            if s.settings.dark_mode {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        ));
                    }

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
                        || s.settings.bluetooth_controller != bluetooth_controller
                        || s.settings.bluetooth_scan_timeout_secs != bluetooth_timeout
                        || s.settings.bluetooth_scan_pause_ms != bluetooth_pause;
                    if bluetooth_changed {
                        s.settings.bluetooth_enabled = bluetooth_enabled;
                        s.settings.bluetooth_controller = bluetooth_controller;
                        s.settings.bluetooth_scan_timeout_secs = bluetooth_timeout;
                        s.settings.bluetooth_scan_pause_ms = bluetooth_pause;
                        bluetooth_restart_needed = s.bluetooth_runtime.is_some();
                        applied_messages.push("bluetooth settings applied".to_string());
                    }

                    if s.settings.geoip_city_db_path != geoip_path {
                        s.settings.geoip_city_db_path = geoip_path.clone();
                        full_restart_needed = s.capture_runtime.is_some();
                        applied_messages
                            .push(format!("GeoIP lookup file set to {}", geoip_path.display()));
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
                if dark_mode_changed {
                    apply_dark_mode_preference(dark_mode_check.is_active());
                }
                state.borrow_mut().save_settings_to_disk();
                if view_changed || dark_mode_changed {
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
                        sync_view_menu_action_state(
                            &app,
                            "settings_dark_mode",
                            dark_mode_check.is_active(),
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
                "{}{} ({}){}",
                ctrl.id,
                ctrl.adapter
                    .as_deref()
                    .map(|adapter| format!(" [{}]", adapter))
                    .unwrap_or_default(),
                if ctrl.name.is_empty() {
                    "unnamed"
                } else {
                    ctrl.name.as_str()
                },
                if ctrl.is_default { " [default]" } else { "" }
            ),
        );
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
        scan_timeout_entry.set_text(&s.settings.bluetooth_scan_timeout_secs.to_string());
        scan_pause_entry.set_text(&s.settings.bluetooth_scan_pause_ms.to_string());
    }

    area.append(&Label::new(Some("Bluetooth Radio")));
    area.append(&controller_combo);
    area.append(&Label::new(Some("Scan Timeout Seconds")));
    area.append(&scan_timeout_entry);
    area.append(&Label::new(Some("Scan Pause Milliseconds")));
    area.append(&scan_pause_entry);

    {
        let state = state.clone();
        dialog.connect_response(move |d, resp| {
            if resp == ResponseType::Apply {
                let controller = match controller_combo.active_id().as_deref() {
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
                s.settings.bluetooth_controller = controller;
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
            channels: cap
                .channels
                .into_iter()
                .filter(|c| c.enabled)
                .map(|c| c.channel)
                .collect(),
            dwell_ms: 200,
            ht_mode: default_hop_ht_mode(),
            channel_ht_modes: BTreeMap::new(),
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
    for value in &incoming.local_ipv4_addresses {
        if !existing.network_intel.local_ipv4_addresses.contains(value) {
            existing
                .network_intel
                .local_ipv4_addresses
                .push(value.clone());
        }
    }
    for value in &incoming.local_ipv6_addresses {
        if !existing.network_intel.local_ipv6_addresses.contains(value) {
            existing
                .network_intel
                .local_ipv6_addresses
                .push(value.clone());
        }
    }
    for value in &incoming.dhcp_hostnames {
        if !existing.network_intel.dhcp_hostnames.contains(value) {
            existing.network_intel.dhcp_hostnames.push(value.clone());
        }
    }
    for value in &incoming.dhcp_fqdns {
        if !existing.network_intel.dhcp_fqdns.contains(value) {
            existing.network_intel.dhcp_fqdns.push(value.clone());
        }
    }
    for value in &incoming.dhcp_vendor_classes {
        if !existing.network_intel.dhcp_vendor_classes.contains(value) {
            existing
                .network_intel
                .dhcp_vendor_classes
                .push(value.clone());
        }
    }
    for value in &incoming.dns_names {
        if !existing.network_intel.dns_names.contains(value) {
            existing.network_intel.dns_names.push(value.clone());
        }
    }
    for endpoint in &incoming.remote_endpoints {
        if let Some(current) =
            existing
                .network_intel
                .remote_endpoints
                .iter_mut()
                .find(|candidate| {
                    candidate.ip_address == endpoint.ip_address
                        && candidate.port == endpoint.port
                        && candidate.protocol == endpoint.protocol
                })
        {
            current.first_seen = current.first_seen.min(endpoint.first_seen);
            current.last_seen = current.last_seen.max(endpoint.last_seen);
            current.packet_count = current.packet_count.max(endpoint.packet_count);
            if current.domain.is_none() && endpoint.domain.is_some() {
                current.domain = endpoint.domain.clone();
            }
            if current.geo_city.is_none() && endpoint.geo_city.is_some() {
                current.geo_city = endpoint.geo_city.clone();
            }
        } else {
            existing
                .network_intel
                .remote_endpoints
                .push(endpoint.clone());
        }
    }
    existing.network_intel.remote_endpoints.sort_by(|a, b| {
        b.last_seen
            .cmp(&a.last_seen)
            .then_with(|| b.packet_count.cmp(&a.packet_count))
    });
    if existing.network_intel.remote_endpoints.len() > 64 {
        existing.network_intel.remote_endpoints.truncate(64);
    }
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
    use crate::model::{ClientEndpointRecord, ClientNetworkIntel};

    #[test]
    fn client_detail_text_includes_network_intel_sections() {
        let now = Utc::now();
        let mut client = ClientRecord::new("AA:BB:CC:DD:EE:FF", now);
        client.oui_manufacturer = Some("Example Vendor".to_string());
        client.associated_ap = Some("11:22:33:44:55:66".to_string());
        client.rssi_dbm = Some(-52);
        client.probes = vec!["ExampleWiFi".to_string()];
        client.seen_access_points = vec!["11:22:33:44:55:66".to_string()];
        client.handshake_networks = vec!["11:22:33:44:55:66".to_string()];
        client.network_intel = ClientNetworkIntel {
            local_ipv4_addresses: vec!["192.168.1.25".to_string()],
            local_ipv6_addresses: vec!["fe80::1234".to_string()],
            dhcp_hostnames: vec!["phone".to_string()],
            dhcp_fqdns: vec!["phone.lan".to_string()],
            dhcp_vendor_classes: vec!["android-dhcp-13".to_string()],
            dns_names: vec!["example.com".to_string()],
            remote_endpoints: vec![ClientEndpointRecord {
                ip_address: "93.184.216.34".to_string(),
                protocol: "TCP".to_string(),
                port: Some(443),
                domain: Some("example.com".to_string()),
                geo_city: Some("Los Angeles, US".to_string()),
                first_seen: now,
                last_seen: now,
                packet_count: 4,
            }],
            uplink_bytes: 512,
            downlink_bytes: 1024,
            retry_frame_count: 2,
            power_save_observed: true,
            qos_priorities: vec![0, 5],
            eapol_frame_count: 1,
            pmkid_count: 1,
            last_frame_type: Some(2),
            last_frame_subtype: Some(8),
            last_channel: Some(6),
            last_frequency_mhz: Some(2437),
            band: SpectrumBand::Ghz2_4,
            last_reason_code: Some(7),
            last_status_code: Some(0),
            listen_interval: Some(10),
            ..ClientNetworkIntel::default()
        };

        let rendered = format_client_detail_text(&client, &[]);
        assert!(rendered.contains("Open Network Metadata"));
        assert!(rendered.contains("192.168.1.25"));
        assert!(rendered.contains("example.com"));
        assert!(rendered.contains("93.184.216.34:443"));
        assert!(rendered.contains("Radio And Behavior"));
        assert!(rendered.contains("Security"));
    }

    #[test]
    fn client_detail_signature_changes_when_endpoint_content_changes() {
        let now = Utc::now();
        let mut client = ClientRecord::new("AA:BB:CC:DD:EE:FF", now);
        client.network_intel.remote_endpoints = vec![ClientEndpointRecord {
            ip_address: "93.184.216.34".to_string(),
            protocol: "TCP".to_string(),
            port: Some(443),
            domain: Some("example.com".to_string()),
            geo_city: Some("Los Angeles, US".to_string()),
            first_seen: now,
            last_seen: now,
            packet_count: 1,
        }];

        let before = client_detail_signature(&client);
        client.network_intel.remote_endpoints[0].packet_count = 2;
        let after_packets = client_detail_signature(&client);
        assert_ne!(before, after_packets);

        client.network_intel.remote_endpoints[0].domain = Some("www.example.com".to_string());
        let after_domain = client_detail_signature(&client);
        assert_ne!(after_packets, after_domain);
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
}

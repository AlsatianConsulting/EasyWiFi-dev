use crate::bluetooth::{self, BluetoothEvent, BluetoothRuntime, BluetoothScanConfig};
use crate::capture::{self, CaptureConfig, CaptureEvent, CaptureRuntime};
use crate::model::{AccessPointRecord, BluetoothDeviceRecord, ChannelUsagePoint, ClientRecord};
use crate::settings::{
    AppSettings, ChannelSelectionMode, InterfaceSettings, WatchlistDeviceType, WatchlistEntry,
};
use anyhow::{Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct RuntimeHandles {
    capture: Option<CaptureRuntime>,
    bluetooth: Option<BluetoothRuntime>,
}

struct WebState {
    settings: AppSettings,
    runtime: RuntimeHandles,
    wifi_interface_restore_types: HashMap<String, String>,
    access_points: HashMap<String, AccessPointRecord>,
    clients: HashMap<String, ClientRecord>,
    bluetooth_devices: HashMap<String, BluetoothDeviceRecord>,
    channel_usage: Vec<ChannelUsagePoint>,
    bt_enumeration_status: HashMap<String, BtEnumerationStatus>,
    logs: Vec<String>,
}

#[derive(Clone, Serialize)]
struct BtEnumerationStatus {
    message: String,
    is_error: bool,
}

#[derive(Serialize)]
struct StateResponse {
    scanning_wifi: bool,
    scanning_bluetooth: bool,
    access_points: Vec<AccessPointRecord>,
    clients: Vec<ClientRecord>,
    bluetooth_devices: Vec<BluetoothDeviceRecord>,
    bt_enumeration_status: HashMap<String, BtEnumerationStatus>,
    channel_usage: Vec<ChannelUsagePoint>,
    logs: Vec<String>,
}

#[derive(Serialize)]
struct WebMetaResponse {
    interfaces: Vec<WebInterfaceInfo>,
    selected_interface: Option<String>,
    bluetooth_controllers: Vec<WebBluetoothControllerInfo>,
    selected_bluetooth_controller: Option<String>,
    watchlist_entries: Vec<WebWatchlistEntry>,
}

#[derive(Serialize)]
struct WebInterfaceInfo {
    name: String,
    if_type: String,
}

#[derive(Serialize)]
struct WebBluetoothControllerInfo {
    id: String,
    name: String,
    is_default: bool,
}

#[derive(Serialize)]
struct WebWatchlistEntry {
    index: usize,
    label: String,
    device_type: String,
    name: String,
    mac: String,
}

#[derive(Deserialize)]
struct InterfaceSelectRequest {
    interface_name: String,
}

#[derive(Deserialize)]
struct WatchlistAddRequest {
    label: String,
    description: Option<String>,
    name: Option<String>,
    mac_or_bssid: Option<String>,
    oui: Option<String>,
    device_type: Option<String>,
}

#[derive(Deserialize)]
struct WatchlistDeleteRequest {
    index: usize,
}

#[derive(Deserialize)]
struct ApLockRequest {
    bssid: String,
}

#[derive(Deserialize)]
struct BluetoothEnumerateRequest {
    mac: String,
    confirm_active: Option<bool>,
}

#[derive(Deserialize)]
struct ScanStartCustomRequest {
    wifi_enabled: bool,
    bluetooth_enabled: bool,
}

#[derive(Serialize)]
struct InterfaceCapabilitiesResponse {
    interface_name: String,
    channels: Vec<u16>,
    ht_modes: Vec<String>,
}

#[derive(Serialize)]
struct ScanSetupResponse {
    wifi_enabled: bool,
    bluetooth_enabled: bool,
    selected_interface: Option<String>,
    mode: String,
    locked_channel: Option<u16>,
    locked_ht_mode: Option<String>,
    hop_channels: Vec<u16>,
    hop_dwell_ms: u64,
    hop_ht_mode: String,
    channel_ht_modes: BTreeMap<u16, Vec<String>>,
    wifi_band: String,
    wifi_bandwidths: Vec<String>,
    wifi_export_enabled: bool,
    wifi_export_dir: String,
    bluetooth_export_enabled: bool,
    bluetooth_export_dir: String,
    bluetooth_controller: Option<String>,
}

#[derive(Deserialize)]
struct ScanSetupRequest {
    wifi_enabled: bool,
    bluetooth_enabled: bool,
    selected_interface: Option<String>,
    mode: String,
    locked_channel: Option<u16>,
    locked_ht_mode: Option<String>,
    hop_channels: Option<Vec<u16>>,
    hop_dwell_ms: Option<u64>,
    hop_ht_mode: Option<String>,
    channel_ht_modes: Option<BTreeMap<u16, Vec<String>>>,
    wifi_band: Option<String>,
    wifi_bandwidths: Option<Vec<String>>,
    wifi_export_enabled: bool,
    wifi_export_dir: String,
    bluetooth_export_enabled: bool,
    bluetooth_export_dir: String,
    bluetooth_controller: Option<String>,
}

pub fn run() -> Result<()> {
    let settings = AppSettings::load_from_disk().unwrap_or_default();
    let state = Arc::new(Mutex::new(WebState {
        settings,
        runtime: RuntimeHandles::default(),
        wifi_interface_restore_types: HashMap::new(),
        access_points: HashMap::new(),
        clients: HashMap::new(),
        bluetooth_devices: HashMap::new(),
        channel_usage: Vec::new(),
        bt_enumeration_status: HashMap::new(),
        logs: vec!["web ui initialized".to_string()],
    }));

    let (capture_tx, capture_rx) = unbounded::<CaptureEvent>();
    let (bluetooth_tx, bluetooth_rx) = unbounded::<BluetoothEvent>();
    spawn_event_pump(state.clone(), capture_rx, bluetooth_rx);

    let addr = "127.0.0.1:8787";
    let listener = TcpListener::bind(addr).with_context(|| format!("failed to bind web UI on {addr}"))?;
    eprintln!("EasyWiFi Web UI listening on http://{addr}");

    let url = format!("http://{addr}");
    if let Err(err) = launch_browser(&url) {
        eprintln!("failed to launch browser for web UI: {err}");
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = state.clone();
                let capture_tx = capture_tx.clone();
                let bluetooth_tx = bluetooth_tx.clone();
                thread::spawn(move || {
                    let _ = handle_client(stream, state, capture_tx, bluetooth_tx);
                });
            }
            Err(err) => {
                eprintln!("web ui accept error: {err}");
            }
        }
    }
    Ok(())
}

fn spawn_event_pump(
    state: Arc<Mutex<WebState>>,
    capture_rx: Receiver<CaptureEvent>,
    bluetooth_rx: Receiver<BluetoothEvent>,
) {
    thread::spawn(move || loop {
        let mut changed = false;
        while let Ok(event) = capture_rx.try_recv() {
            changed = true;
            if let Ok(mut s) = state.lock() {
                apply_capture_event(&mut s, event);
            }
        }
        while let Ok(event) = bluetooth_rx.try_recv() {
            changed = true;
            if let Ok(mut s) = state.lock() {
                apply_bluetooth_event(&mut s, event);
            }
        }
        if !changed {
            thread::sleep(Duration::from_millis(75));
        }
    });
}

fn apply_capture_event(state: &mut WebState, event: CaptureEvent) {
    match event {
        CaptureEvent::AccessPointSeen(ap) => {
            state.access_points.insert(ap.bssid.clone(), ap);
        }
        CaptureEvent::ClientSeen(client) => {
            state.clients.insert(client.mac.clone(), client);
        }
        CaptureEvent::ChannelUsage(usage) => {
            state.channel_usage.push(usage);
            if state.channel_usage.len() > 800 {
                let trim = state.channel_usage.len() - 800;
                state.channel_usage.drain(0..trim);
            }
        }
        CaptureEvent::Observation { .. } => {}
        CaptureEvent::HandshakeSeen(hs) => {
            if let Some(ap) = state.access_points.get_mut(&hs.bssid) {
                ap.handshake_count = ap.handshake_count.saturating_add(1);
            }
        }
        CaptureEvent::Log(line) => push_log(state, line),
    }
}

fn apply_bluetooth_event(state: &mut WebState, event: BluetoothEvent) {
    match event {
        BluetoothEvent::DeviceSeen(device) => {
            state.bluetooth_devices.insert(device.mac.clone(), device);
        }
        BluetoothEvent::EnumerationStatus { mac, message, is_error } => {
            state.bt_enumeration_status.insert(
                mac.clone(),
                BtEnumerationStatus {
                    message: message.clone(),
                    is_error,
                },
            );
            let status = if is_error { "error" } else { "ok" };
            push_log(state, format!("bt enum {status} {mac}: {message}"));
        }
        BluetoothEvent::Log(line) => push_log(state, line),
    }
}

fn push_log(state: &mut WebState, line: String) {
    state.logs.push(line);
    if state.logs.len() > 120 {
        let trim = state.logs.len() - 120;
        state.logs.drain(0..trim);
    }
}

fn handle_client(
    mut stream: TcpStream,
    state: Arc<Mutex<WebState>>,
    capture_tx: Sender<CaptureEvent>,
    bluetooth_tx: Sender<BluetoothEvent>,
) -> Result<()> {
    let req_bytes = read_http_request(&mut stream)?;
    if req_bytes.is_empty() {
        return Ok(());
    }
    let req = String::from_utf8_lossy(&req_bytes);
    let mut lines = req.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let raw_path = parts.next().unwrap_or("/");
    let (path, query) = split_request_path(raw_path);
    let body = request_body_bytes(&req_bytes);

    if method == "GET" && path == "/api/health" {
        return respond_json(&mut stream, "{\"status\":\"ok\",\"ui\":\"web\"}");
    }

    if method == "GET" && path == "/api/state" {
        let payload = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
            let mut aps = s.access_points.values().cloned().collect::<Vec<_>>();
            aps.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
            let mut clients = s.clients.values().cloned().collect::<Vec<_>>();
            clients.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
            let mut bt = s.bluetooth_devices.values().cloned().collect::<Vec<_>>();
            bt.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));

            StateResponse {
                scanning_wifi: s.runtime.capture.is_some(),
                scanning_bluetooth: s.runtime.bluetooth.is_some(),
                access_points: aps,
                clients,
                bluetooth_devices: bt,
                bt_enumeration_status: s.bt_enumeration_status.clone(),
                channel_usage: s.channel_usage.clone(),
                logs: s.logs.clone(),
            }
        };
        let body = serde_json::to_string(&payload).context("failed to serialize state")?;
        return respond_json(&mut stream, &body);
    }

    if method == "GET" && path == "/api/meta" {
        let payload = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
            let selected_interface = s
                .settings
                .interfaces
                .iter()
                .find(|iface| iface.enabled)
                .map(|iface| iface.interface_name.clone());
            let interfaces = capture::list_interfaces()
                .unwrap_or_default()
                .into_iter()
                .map(|iface| WebInterfaceInfo {
                    name: iface.name,
                    if_type: iface.if_type,
                })
                .collect::<Vec<_>>();
            let watchlist_entries = s
                .settings
                .watchlists
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| WebWatchlistEntry {
                    index,
                    label: entry.label.clone(),
                    device_type: match entry.device_type {
                        WatchlistDeviceType::Wifi => "wifi".to_string(),
                        WatchlistDeviceType::Bluetooth => "bluetooth".to_string(),
                    },
                    name: entry.name.clone(),
                    mac: entry.mac.clone(),
                })
                .collect::<Vec<_>>();
            let bluetooth_controllers = bluetooth::list_controllers()
                .unwrap_or_default()
                .into_iter()
                .map(|ctrl| WebBluetoothControllerInfo {
                    id: ctrl.id,
                    name: ctrl.name,
                    is_default: ctrl.is_default,
                })
                .collect::<Vec<_>>();
            WebMetaResponse {
                interfaces,
                selected_interface,
                bluetooth_controllers,
                selected_bluetooth_controller: s.settings.bluetooth_controller.clone(),
                watchlist_entries,
            }
        };
        let body = serde_json::to_string(&payload).context("failed to serialize meta payload")?;
        return respond_json(&mut stream, &body);
    }

    if method == "GET" && path == "/api/interface/capabilities" {
        let requested = query_param(query.as_deref(), "name").unwrap_or_default();
        let interface_name = if requested.trim().is_empty() {
            let s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
            s.settings
                .interfaces
                .iter()
                .find(|iface| iface.enabled)
                .map(|iface| iface.interface_name.clone())
                .unwrap_or_default()
        } else {
            requested
        };
        if interface_name.is_empty() {
            return respond_status(&mut stream, 400, "Bad Request", "text/plain", b"missing interface");
        }

        let channels = actionable_channels_for_interface(&interface_name);
        let ht_modes = actionable_ht_modes_for_interface(&interface_name);
        let payload = InterfaceCapabilitiesResponse {
            interface_name,
            channels,
            ht_modes,
        };
        let body = serde_json::to_string(&payload).context("failed to serialize capabilities")?;
        return respond_json(&mut stream, &body);
    }

    if method == "GET" && path == "/api/scan/setup" {
        let s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
        let selected_interface = s
            .settings
            .interfaces
            .iter()
            .find(|iface| iface.enabled)
            .map(|iface| iface.interface_name.clone());
        let mut mode = "hop_specific".to_string();
        let mut locked_channel = None;
        let mut locked_ht_mode = None;
        let mut hop_channels = vec![1, 6, 11];
        let mut hop_dwell_ms = 200_u64;
        let mut hop_ht_mode = "HT20".to_string();
        let mut channel_ht_modes = BTreeMap::<u16, Vec<String>>::new();
        let mut wifi_band = "all".to_string();
        let mut wifi_bandwidths = vec!["HT20".to_string()];
        if let Some(iface) = s.settings.interfaces.iter().find(|iface| iface.enabled) {
            let supported_ht_modes = actionable_ht_modes_for_interface(&iface.interface_name);
            if !supported_ht_modes.is_empty() {
                wifi_bandwidths = supported_ht_modes;
            }
            match &iface.channel_mode {
                ChannelSelectionMode::Locked { channel, ht_mode } => {
                    mode = "locked".to_string();
                    locked_channel = Some(*channel);
                    locked_ht_mode = Some(ht_mode.clone());
                    wifi_bandwidths = vec![ht_mode.clone()];
                }
                ChannelSelectionMode::HopAll {
                    channels,
                    dwell_ms,
                    ht_mode,
                    channel_ht_modes: saved_channel_ht_modes,
                } => {
                    mode = "hop_specific".to_string();
                    hop_channels = channels.clone();
                    hop_dwell_ms = *dwell_ms;
                    hop_ht_mode = ht_mode.clone();
                    channel_ht_modes = saved_channel_ht_modes.clone();
                    if !saved_channel_ht_modes.is_empty() {
                        let mut merged = Vec::<String>::new();
                        for modes in saved_channel_ht_modes.values() {
                            for mode in modes {
                                if !merged.iter().any(|m| m.eq_ignore_ascii_case(mode)) {
                                    merged.push(mode.clone());
                                }
                            }
                        }
                        if !merged.is_empty() {
                            wifi_bandwidths = merged;
                        }
                    }
                }
                ChannelSelectionMode::HopBand { channels, dwell_ms, .. } => {
                    mode = "hop_specific".to_string();
                    hop_channels = channels.clone();
                    hop_dwell_ms = *dwell_ms;
                }
            }
            wifi_band = infer_band_from_channels(&hop_channels, locked_channel);
        }
        let wifi_enabled = s.settings.interfaces.iter().any(|iface| iface.enabled);
        let payload = ScanSetupResponse {
            wifi_enabled,
            bluetooth_enabled: s.settings.bluetooth_enabled,
            selected_interface,
            mode,
            locked_channel,
            locked_ht_mode,
            hop_channels,
            hop_dwell_ms,
            hop_ht_mode,
            channel_ht_modes,
            wifi_band,
            wifi_bandwidths,
            wifi_export_enabled: s.settings.wifi_export_enabled,
            wifi_export_dir: s.settings.wifi_export_dir.to_string_lossy().to_string(),
            bluetooth_export_enabled: s.settings.bluetooth_export_enabled,
            bluetooth_export_dir: s
                .settings
                .bluetooth_export_dir
                .to_string_lossy()
                .to_string(),
            bluetooth_controller: s.settings.bluetooth_controller.clone(),
        };
        let body = serde_json::to_string(&payload).context("failed to serialize scan setup")?;
        return respond_json(&mut stream, &body);
    }

    if method == "POST" && path == "/api/scan/setup" {
        let req = serde_json::from_slice::<ScanSetupRequest>(body)
            .context("invalid /api/scan/setup payload")?;
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;

        let mut selected_interface = req.selected_interface.unwrap_or_default().trim().to_string();
        if selected_interface.is_empty() {
            if let Some(existing) = s.settings.interfaces.iter().find(|iface| iface.enabled) {
                selected_interface = existing.interface_name.clone();
            }
        }
        if selected_interface.is_empty() {
            selected_interface = "wlan0".to_string();
        }

        let mode = req.mode.trim().to_lowercase();
        let supported_channels = actionable_channels_for_interface(&selected_interface);
        let supported_modes = actionable_ht_modes_for_interface(&selected_interface);
        let mut wifi_bandwidths = req
            .wifi_bandwidths
            .unwrap_or_default()
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if !supported_modes.is_empty() {
            wifi_bandwidths = wifi_bandwidths
                .into_iter()
                .filter(|mode| supported_modes.contains(mode))
                .collect::<Vec<_>>();
        }
        let requested_primary = req
            .locked_ht_mode
            .clone()
            .or_else(|| req.hop_ht_mode.clone())
            .or_else(|| wifi_bandwidths.first().cloned());
        let mut primary_ht_mode = requested_primary
            .filter(|mode| supported_modes.is_empty() || supported_modes.contains(mode))
            .unwrap_or_else(|| {
                supported_modes
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "HT20".to_string())
            });
        let channel_mode = if mode == "locked" {
            let requested_lock = req.locked_channel.unwrap_or(1);
            let locked_channel = if supported_channels.is_empty()
                || supported_channels.contains(&requested_lock)
            {
                requested_lock
            } else {
                supported_channels.first().copied().unwrap_or(requested_lock)
            };
            if !mode_allowed_for_channel(&primary_ht_mode, locked_channel) {
                let per_channel_modes = modes_for_channel(&supported_modes, locked_channel);
                primary_ht_mode = per_channel_modes
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "HT20".to_string());
            }
            ChannelSelectionMode::Locked {
                channel: locked_channel,
                ht_mode: primary_ht_mode.clone(),
            }
        } else {
            let mut channels = req
                .hop_channels
                .unwrap_or_else(|| vec![1, 6, 11])
                .into_iter()
                .filter(|ch| *ch > 0)
                .collect::<Vec<_>>();
            if let Some(band) = req.wifi_band.as_deref() {
                channels = channels
                    .into_iter()
                    .filter(|ch| match band.trim() {
                        "2.4" => (1..=14).contains(ch),
                        "5" => (32..=177).contains(ch),
                        "6" => *ch > 177,
                        _ => true,
                    })
                    .collect::<Vec<_>>();
            }
            if !supported_channels.is_empty() {
                channels = channels
                    .into_iter()
                    .filter(|ch| supported_channels.contains(ch))
                    .collect::<Vec<_>>();
            }
            if channels.is_empty() {
                channels = if supported_channels.is_empty() {
                    vec![1, 6, 11]
                } else {
                    supported_channels.clone()
                };
            }
            let mut channel_ht_modes = BTreeMap::<u16, Vec<String>>::new();
            let requested_channel_ht_modes = req.channel_ht_modes.unwrap_or_default();
            let all_supported_channel_modes = if wifi_bandwidths.is_empty() {
                supported_modes.clone()
            } else {
                wifi_bandwidths.clone()
            };
            if !requested_channel_ht_modes.is_empty() {
                for channel in &channels {
                    let requested_modes = requested_channel_ht_modes
                        .get(channel)
                        .cloned()
                        .or_else(|| requested_channel_ht_modes.get(&0).cloned())
                        .unwrap_or_default();
                    let filtered_modes = modes_for_channel(&requested_modes, *channel);
                    if !filtered_modes.is_empty() {
                        channel_ht_modes.insert(*channel, filtered_modes);
                    }
                }
            } else if !all_supported_channel_modes.is_empty() {
                for channel in &channels {
                    let per_channel = modes_for_channel(&all_supported_channel_modes, *channel);
                    if !per_channel.is_empty() {
                        channel_ht_modes.insert(*channel, per_channel);
                    }
                }
            }
            ChannelSelectionMode::HopAll {
                channels,
                dwell_ms: req.hop_dwell_ms.unwrap_or(200).max(25),
                ht_mode: primary_ht_mode,
                channel_ht_modes,
            }
        };

        let mut found = false;
        for iface in &mut s.settings.interfaces {
            if iface.interface_name == selected_interface {
                iface.enabled = req.wifi_enabled;
                iface.channel_mode = channel_mode.clone();
                found = true;
            } else {
                iface.enabled = false;
            }
        }
        if !found {
            s.settings.interfaces.push(InterfaceSettings {
                interface_name: selected_interface.clone(),
                monitor_interface_name: None,
                channel_mode,
                enabled: req.wifi_enabled,
            });
        }
        s.settings.bluetooth_enabled = req.bluetooth_enabled;
        s.settings.wifi_export_enabled = req.wifi_export_enabled;
        s.settings.bluetooth_export_enabled = req.bluetooth_export_enabled;
        let wifi_export_dir = req.wifi_export_dir.trim();
        if !wifi_export_dir.is_empty() {
            s.settings.wifi_export_dir = PathBuf::from(wifi_export_dir);
        }
        let bluetooth_export_dir = req.bluetooth_export_dir.trim();
        if !bluetooth_export_dir.is_empty() {
            s.settings.bluetooth_export_dir = PathBuf::from(bluetooth_export_dir);
        }
        s.settings.bluetooth_controller = req
            .bluetooth_controller
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        s.settings.save_to_disk().ok();
        push_log(&mut s, "scan setup updated".to_string());
        return respond_json(&mut stream, "{\"ok\":true}");
    }

    if method == "POST" && path == "/api/scan/start" {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
        start_scans_if_needed(&mut s, &capture_tx, &bluetooth_tx);
        return respond_json(&mut stream, "{\"ok\":true}");
    }

    if method == "POST" && path == "/api/scan/start_custom" {
        let req = serde_json::from_slice::<ScanStartCustomRequest>(body)
            .context("invalid /api/scan/start_custom payload")?;
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
        start_scans_with_selection(
            &mut s,
            &capture_tx,
            &bluetooth_tx,
            req.wifi_enabled,
            req.bluetooth_enabled,
        );
        return respond_json(&mut stream, "{\"ok\":true}");
    }

    if method == "POST" && path == "/api/scan/stop" {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
        stop_wifi_capture(&mut s);
        stop_bluetooth_scan(&mut s);
        clear_live_scan_state(&mut s);
        push_log(&mut s, "Live tables cleared".to_string());
        push_log(&mut s, "Scanning stopped".to_string());
        return respond_json(&mut stream, "{\"ok\":true}");
    }

    if method == "POST" && path == "/api/interface/select" {
        let req = serde_json::from_slice::<InterfaceSelectRequest>(body)
            .context("invalid /api/interface/select payload")?;
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
        let interface_name = req.interface_name.trim().to_string();
        if interface_name.is_empty() {
            return respond_status(&mut stream, 400, "Bad Request", "text/plain", b"missing interface");
        }

        let mut found = false;
        for iface in &mut s.settings.interfaces {
            if iface.interface_name == interface_name {
                iface.enabled = true;
                found = true;
            } else {
                iface.enabled = false;
            }
        }
        if !found {
            s.settings.interfaces.push(InterfaceSettings {
                interface_name: interface_name.clone(),
                monitor_interface_name: None,
                channel_mode: ChannelSelectionMode::default(),
                enabled: true,
            });
            for iface in &mut s.settings.interfaces {
                if iface.interface_name != interface_name {
                    iface.enabled = false;
                }
            }
        }
        s.settings.save_to_disk().ok();
        push_log(
            &mut s,
            format!("selected scan interface set to {}", interface_name),
        );
        if s.runtime.capture.is_some() {
            stop_wifi_capture(&mut s);
            start_scans_if_needed(&mut s, &capture_tx, &bluetooth_tx);
        }
        return respond_json(&mut stream, "{\"ok\":true}");
    }

    if method == "POST" && path == "/api/watchlist/add" {
        let req = serde_json::from_slice::<WatchlistAddRequest>(body)
            .context("invalid /api/watchlist/add payload")?;
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
        let label = req.label.trim().to_string();
        if label.is_empty() {
            return respond_status(
                &mut stream,
                400,
                "Bad Request",
                "text/plain",
                b"watchlist label is required",
            );
        }
        let mut entry = WatchlistEntry {
            label,
            device_type: match req.device_type.as_deref() {
                Some("bluetooth") => WatchlistDeviceType::Bluetooth,
                _ => WatchlistDeviceType::Wifi,
            },
            mac: req
                .mac_or_bssid
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_uppercase(),
            name: req
                .name
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_string(),
            color_hex: "#f59e0b".to_string(),
        };

        let oui = req.oui.unwrap_or_default().trim().to_uppercase();
        if !oui.is_empty() {
            entry.mac = oui;
        }
        if let Some(description) = req.description {
            let d = description.trim();
            if !d.is_empty() {
                entry.label = format!("{} - {}", entry.label, d);
            }
        }
        s.settings.watchlists.entries.push(entry);
        s.settings.save_to_disk().ok();
        push_log(&mut s, "watchlist entry added".to_string());
        return respond_json(&mut stream, "{\"ok\":true}");
    }

    if method == "POST" && path == "/api/watchlist/delete" {
        let req = serde_json::from_slice::<WatchlistDeleteRequest>(body)
            .context("invalid /api/watchlist/delete payload")?;
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
        if req.index < s.settings.watchlists.entries.len() {
            s.settings.watchlists.entries.remove(req.index);
            s.settings.save_to_disk().ok();
            push_log(&mut s, "watchlist entry removed".to_string());
            return respond_json(&mut stream, "{\"ok\":true}");
        }
        return respond_status(
            &mut stream,
            400,
            "Bad Request",
            "text/plain",
            b"watchlist index out of range",
        );
    }

    if method == "POST" && path == "/api/ap/lock" {
        let req =
            serde_json::from_slice::<ApLockRequest>(body).context("invalid /api/ap/lock payload")?;
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
        let bssid = req.bssid.trim();
        if bssid.is_empty() {
            return respond_status(&mut stream, 400, "Bad Request", "text/plain", b"missing bssid");
        }
        let Some(ap) = s.access_points.get(bssid).cloned() else {
            return respond_status(&mut stream, 404, "Not Found", "text/plain", b"ap not found");
        };
        let channel = ap.channel.unwrap_or(1);
        let mut locked = false;
        for iface in &mut s.settings.interfaces {
            if iface.enabled && !locked {
                iface.channel_mode = ChannelSelectionMode::Locked {
                    channel,
                    ht_mode: "HT20".to_string(),
                };
                locked = true;
            }
        }
        if !locked {
            if let Some(first) = s.settings.interfaces.first_mut() {
                first.enabled = true;
                first.channel_mode = ChannelSelectionMode::Locked {
                    channel,
                    ht_mode: "HT20".to_string(),
                };
                locked = true;
            }
        }
        if !locked {
            return respond_status(
                &mut stream,
                400,
                "Bad Request",
                "text/plain",
                b"no interface configured",
            );
        }
        s.settings.save_to_disk().ok();
        push_log(
            &mut s,
            format!("locked selected interface to AP {} channel {}", bssid, channel),
        );
        if s.runtime.capture.is_some() {
            stop_wifi_capture(&mut s);
            start_scans_if_needed(&mut s, &capture_tx, &bluetooth_tx);
        }
        return respond_json(&mut stream, "{\"ok\":true}");
    }

    if method == "POST" && path == "/api/bluetooth/enumerate" {
        let req = serde_json::from_slice::<BluetoothEnumerateRequest>(body)
            .context("invalid /api/bluetooth/enumerate payload")?;
        if !req.confirm_active.unwrap_or(false) {
            return respond_status(
                &mut stream,
                400,
                "Bad Request",
                "text/plain",
                b"active bluetooth enumeration requires explicit confirmation",
            );
        }
        let mac = req.mac.trim().to_uppercase();
        if mac.is_empty() {
            return respond_status(&mut stream, 400, "Bad Request", "text/plain", b"missing mac");
        }

        let controller = {
            let mut s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
            s.bt_enumeration_status.insert(
                mac.clone(),
                BtEnumerationStatus {
                    message: "enumeration running".to_string(),
                    is_error: false,
                },
            );
            s.settings.bluetooth_controller.clone()
        };

        let state_for_task = state.clone();
        let mac_for_task = mac.clone();
        thread::spawn(move || {
            let result = bluetooth::connect_and_enumerate_device(controller.as_deref(), &mac_for_task);
            if let Ok(mut s) = state_for_task.lock() {
                match result {
                    Ok(record) => {
                        s.bluetooth_devices.insert(record.mac.clone(), record);
                        s.bt_enumeration_status.insert(
                            mac_for_task.clone(),
                            BtEnumerationStatus {
                                message: "enumeration completed".to_string(),
                                is_error: false,
                            },
                        );
                        push_log(
                            &mut s,
                            format!("active bluetooth enumeration completed for {}", mac_for_task),
                        );
                    }
                    Err(err) => {
                        let msg = err.to_string();
                        s.bt_enumeration_status.insert(
                            mac_for_task.clone(),
                            BtEnumerationStatus {
                                message: msg.clone(),
                                is_error: true,
                            },
                        );
                        push_log(
                            &mut s,
                            format!(
                                "active bluetooth enumeration failed for {}: {}",
                                mac_for_task, msg
                            ),
                        );
                    }
                }
            }
        });

        let status = BtEnumerationStatus {
            message: "enumeration running".to_string(),
            is_error: false,
        };
        let body = serde_json::to_string(&status).context("failed to serialize bt status")?;
        return respond_json(&mut stream, &body);
    }

    if method != "GET" {
        return respond_status(
            &mut stream,
            405,
            "Method Not Allowed",
            "text/plain",
            b"method not allowed",
        );
    }

    let file_path = map_path(path);
    if !file_path.exists() {
        return respond_status(&mut stream, 404, "Not Found", "text/plain", b"not found");
    }
    let bytes = fs::read(&file_path)
        .with_context(|| format!("failed reading {}", file_path.display()))?;
    let mime = mime_for(&file_path);
    respond_status(&mut stream, 200, "OK", mime, &bytes)
}

fn start_scans_if_needed(
    state: &mut WebState,
    capture_tx: &Sender<CaptureEvent>,
    bluetooth_tx: &Sender<BluetoothEvent>,
) {
    let wifi_enabled = state.settings.interfaces.iter().any(|i| i.enabled);
    start_scans_with_selection(
        state,
        capture_tx,
        bluetooth_tx,
        wifi_enabled,
        state.settings.bluetooth_enabled,
    );
}

fn start_scans_with_selection(
    state: &mut WebState,
    capture_tx: &Sender<CaptureEvent>,
    bluetooth_tx: &Sender<BluetoothEvent>,
    wifi_enabled: bool,
    bluetooth_enabled: bool,
) {
    if !wifi_enabled && !bluetooth_enabled {
        if state.runtime.capture.is_some() {
            stop_wifi_capture(state);
        }
        if state.runtime.bluetooth.is_some() {
            stop_bluetooth_scan(state);
        }
        push_log(
            state,
            "No scan radios enabled. Enable Wi-Fi and/or Bluetooth in Scan Setup.".to_string(),
        );
        return;
    }

    // Starting a new scan should begin from a clean live-state snapshot.
    let starting_fresh = state.runtime.capture.is_none() && state.runtime.bluetooth.is_none();
    if starting_fresh && (wifi_enabled || bluetooth_enabled) {
        clear_live_scan_state(state);
        push_log(state, "cleared previous scan state".to_string());
    }

    if wifi_enabled && state.runtime.capture.is_none() {
        let discovered_ifaces = capture::list_interfaces().unwrap_or_default();
        let discovered_names = discovered_ifaces
            .iter()
            .map(|iface| iface.name.clone())
            .collect::<HashSet<_>>();
        let mut enabled_interfaces = state
            .settings
            .interfaces
            .iter()
            .filter(|i| i.enabled)
            .filter(|i| discovered_names.contains(&i.interface_name))
            .cloned()
            .collect::<Vec<_>>();
        if enabled_interfaces.is_empty() {
            let fallback_name = discovered_ifaces
                .iter()
                .find(|iface| iface.name.starts_with("wlx") || iface.name.starts_with("wlan"))
                .or_else(|| discovered_ifaces.first())
                .map(|iface| iface.name.clone());
            if let Some(fallback_name) = fallback_name {
                push_log(
                    state,
                    format!(
                        "selected Wi-Fi interface unavailable; falling back to {}",
                        fallback_name
                    ),
                );
                for iface in &mut state.settings.interfaces {
                    iface.enabled = iface.interface_name == fallback_name;
                }
                if !state
                    .settings
                    .interfaces
                    .iter()
                    .any(|iface| iface.interface_name == fallback_name)
                {
                    state.settings.interfaces.push(InterfaceSettings {
                        interface_name: fallback_name.clone(),
                        monitor_interface_name: None,
                        channel_mode: ChannelSelectionMode::default(),
                        enabled: true,
                    });
                }
                state.settings.save_to_disk().ok();
                enabled_interfaces = state
                    .settings
                    .interfaces
                    .iter()
                    .filter(|i| i.enabled && i.interface_name == fallback_name)
                    .cloned()
                    .collect::<Vec<_>>();
            }
        }
        let mut prepared_interfaces = Vec::new();
        for iface in enabled_interfaces {
            match capture::prepare_interface_for_capture(iface.clone(), true) {
                Ok(prepared) => {
                    for line in prepared.status_lines {
                        push_log(state, line);
                    }
                    if let Some(original) = prepared.original_type.as_deref() {
                        if !original.eq_ignore_ascii_case("monitor") {
                            state.wifi_interface_restore_types.insert(
                                prepared.interface.interface_name.clone(),
                                original.to_string(),
                            );
                        }
                    }
                    prepared_interfaces.push(prepared.interface);
                }
                Err(err) => {
                    push_log(
                        state,
                        format!("failed to prepare {} for capture: {}", iface.interface_name, err),
                    );
                }
            }
        }
        for prepared in &prepared_interfaces {
            for iface in &mut state.settings.interfaces {
                if iface.interface_name == prepared.interface_name {
                    iface.monitor_interface_name = prepared.monitor_interface_name.clone();
                    iface.channel_mode = prepared.channel_mode.clone();
                }
            }
        }
        if prepared_interfaces.is_empty() {
            push_log(
                state,
                "Wi-Fi scanning not started; no interfaces were successfully prepared".to_string(),
            );
        }
        if !prepared_interfaces.is_empty() {
            let config = CaptureConfig {
                interfaces: prepared_interfaces,
                session_pcap_path: None,
                wifi_packet_header_mode: state.settings.wifi_packet_header_mode,
                wifi_frame_parsing_enabled: state.settings.enable_wifi_frame_parsing,
                geoip_city_db_path: None,
                gps_enabled: false,
                passive_only: false,
            };
            state.runtime.capture = Some(capture::start_capture(config, capture_tx.clone()));
            push_log(state, "Wi-Fi scanning started".to_string());
        }
    } else if !wifi_enabled && state.runtime.capture.is_some() {
        stop_wifi_capture(state);
    }

    if bluetooth_enabled && state.runtime.bluetooth.is_none() {
        let cfg = BluetoothScanConfig {
            controller: state.settings.bluetooth_controller.clone(),
            source: state.settings.bluetooth_scan_source,
            ubertooth_device: state.settings.ubertooth_device.clone(),
            scan_timeout_secs: state.settings.bluetooth_scan_timeout_secs,
            pause_ms: state.settings.bluetooth_scan_pause_ms,
        };
        state.runtime.bluetooth = Some(bluetooth::start_scan(cfg, bluetooth_tx.clone()));
        push_log(state, "Bluetooth scanning started".to_string());
    } else if !bluetooth_enabled && state.runtime.bluetooth.is_some() {
        stop_bluetooth_scan(state);
    }
}

fn stop_wifi_capture(state: &mut WebState) {
    if let Some(runtime) = state.runtime.capture.take() {
        runtime.stop();
    }
    let mut status_lines = Vec::<String>::new();
    for iface in &mut state.settings.interfaces {
        if let Some(restore_type) = state
            .wifi_interface_restore_types
            .remove(&iface.interface_name)
        {
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
        iface.monitor_interface_name = None;
    }
    for line in status_lines {
        push_log(state, line);
    }
    maybe_export_wifi_data(state);
    push_log(state, "Wi-Fi scanning stopped".to_string());
}

fn stop_bluetooth_scan(state: &mut WebState) {
    if let Some(runtime) = state.runtime.bluetooth.take() {
        runtime.stop();
    }
    maybe_export_bluetooth_data(state);
    push_log(state, "Bluetooth scanning stopped".to_string());
}

fn clear_live_scan_state(state: &mut WebState) {
    state.access_points.clear();
    state.clients.clear();
    state.bluetooth_devices.clear();
    state.channel_usage.clear();
    state.bt_enumeration_status.clear();
}

fn maybe_export_wifi_data(state: &mut WebState) {
    if !state.settings.wifi_export_enabled {
        return;
    }
    let dir = state.settings.wifi_export_dir.clone();
    if let Err(err) = fs::create_dir_all(&dir) {
        push_log(
            state,
            format!(
                "wifi export failed: unable to create {}: {}",
                dir.display(),
                err
            ),
        );
        return;
    }
    let ts = epoch_timestamp();
    let path = dir.join(format!("easywifi_wifi_export_{}.json", ts));
    let payload = serde_json::json!({
        "timestamp": ts,
        "access_points": state.access_points.values().cloned().collect::<Vec<_>>(),
        "clients": state.clients.values().cloned().collect::<Vec<_>>(),
        "channel_usage": state.channel_usage,
    });
    match serde_json::to_vec_pretty(&payload)
        .ok()
        .and_then(|bytes| fs::write(&path, bytes).ok())
    {
        Some(()) => push_log(state, format!("wifi export saved to {}", path.display())),
        None => push_log(state, format!("wifi export failed for {}", path.display())),
    }
}

fn maybe_export_bluetooth_data(state: &mut WebState) {
    if !state.settings.bluetooth_export_enabled {
        return;
    }
    let dir = state.settings.bluetooth_export_dir.clone();
    if let Err(err) = fs::create_dir_all(&dir) {
        push_log(
            state,
            format!(
                "bluetooth export failed: unable to create {}: {}",
                dir.display(),
                err
            ),
        );
        return;
    }
    let ts = epoch_timestamp();
    let path = dir.join(format!("easywifi_bluetooth_export_{}.json", ts));
    let payload = serde_json::json!({
        "timestamp": ts,
        "devices": state.bluetooth_devices.values().cloned().collect::<Vec<_>>(),
        "enumeration_status": state.bt_enumeration_status,
    });
    match serde_json::to_vec_pretty(&payload)
        .ok()
        .and_then(|bytes| fs::write(&path, bytes).ok())
    {
        Some(()) => push_log(state, format!("bluetooth export saved to {}", path.display())),
        None => push_log(
            state,
            format!("bluetooth export failed for {}", path.display()),
        ),
    }
}

fn epoch_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_http_request(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut request = Vec::<u8>::with_capacity(8192);
    let mut chunk = [0_u8; 4096];
    let mut header_end: Option<usize> = None;
    let mut expected_body_len = 0usize;

    loop {
        let read = stream.read(&mut chunk).context("failed reading request")?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);

        if header_end.is_none() {
            if let Some(pos) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let end = pos + 4;
                header_end = Some(end);
                expected_body_len = content_length_from_headers(&request[..end]);
                if request.len() >= end + expected_body_len {
                    request.truncate(end + expected_body_len);
                    break;
                }
            }
        } else if let Some(end) = header_end {
            if request.len() >= end + expected_body_len {
                request.truncate(end + expected_body_len);
                break;
            }
        }

        if request.len() > 2_000_000 {
            anyhow::bail!("request too large");
        }
    }

    Ok(request)
}

fn content_length_from_headers(headers: &[u8]) -> usize {
    let text = String::from_utf8_lossy(headers);
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            if let Ok(parsed) = value.trim().parse::<usize>() {
                return parsed;
            }
        }
    }
    0
}

fn request_body_bytes(request: &[u8]) -> &[u8] {
    let Some(header_end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
        return &[];
    };
    &request[(header_end + 4)..]
}

fn split_request_path(raw_path: &str) -> (&str, Option<String>) {
    if let Some((path, query)) = raw_path.split_once('?') {
        return (path, Some(query.to_string()));
    }
    (raw_path, None)
}

fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    for part in query.split('&') {
        let (k, v) = match part.split_once('=') {
            Some(parts) => parts,
            None => (part, ""),
        };
        if k == key {
            return Some(v.to_string());
        }
    }
    None
}

fn infer_band_from_channels(hop_channels: &[u16], locked_channel: Option<u16>) -> String {
    let mut values = hop_channels.to_vec();
    if values.is_empty() {
        if let Some(channel) = locked_channel {
            values.push(channel);
        }
    }
    if values.is_empty() {
        return "all".to_string();
    }
    let has_24 = values.iter().any(|ch| (1..=14).contains(ch));
    let has_5 = values.iter().any(|ch| (32..=177).contains(ch));
    let has_6 = values.iter().any(|ch| *ch > 177);
    match (has_24, has_5, has_6) {
        (true, false, false) => "2.4".to_string(),
        (false, true, false) => "5".to_string(),
        (false, false, true) => "6".to_string(),
        _ => "all".to_string(),
    }
}

fn actionable_ht_modes_for_interface(interface: &str) -> Vec<String> {
    let mut modes = capture::list_supported_ht_modes(interface)
        .unwrap_or_default()
        .into_iter()
        .map(|mode| mode.trim().to_string())
        .filter(|mode| !mode.is_empty())
        .filter(|mode| !mode.contains("device capability"))
        .filter(|mode| matches!(
            mode.as_str(),
            "NOHT"
                | "HT20"
                | "HT40+"
                | "HT40-"
                | "5MHz"
                | "10MHz"
                | "80MHz"
                | "160MHz"
                | "80+80MHz"
        ))
        .collect::<Vec<_>>();
    if modes.is_empty() {
        modes.push("HT20".to_string());
    }
    if !modes.iter().any(|mode| mode == "HT20") {
        modes.push("HT20".to_string());
    }
    modes.sort();
    modes.dedup();
    modes
}

fn actionable_channels_for_interface(interface: &str) -> Vec<u16> {
    let mut channels = capture::list_supported_channel_details(interface)
        .unwrap_or_default()
        .into_iter()
        .filter(|channel| channel.enabled)
        .map(|channel| channel.channel)
        .filter(|channel| *channel > 0)
        .collect::<Vec<_>>();
    channels.sort_unstable();
    channels.dedup();
    channels
}

fn channel_band_kind(channel: u16) -> &'static str {
    if (1..=14).contains(&channel) {
        "2.4"
    } else if (32..=177).contains(&channel) {
        "5"
    } else {
        "6"
    }
}

fn mode_allowed_for_channel(mode: &str, channel: u16) -> bool {
    let normalized = mode.trim().to_ascii_uppercase();
    if normalized.is_empty() || normalized.contains("DEVICE CAPABILITY") {
        return false;
    }

    match channel_band_kind(channel) {
        "2.4" => matches!(
            normalized.as_str(),
            "NOHT" | "HT20" | "HT40+" | "HT40-" | "5MHZ" | "10MHZ"
        ),
        "5" => matches!(
            normalized.as_str(),
            "NOHT"
                | "HT20"
                | "HT40+"
                | "HT40-"
                | "5MHZ"
                | "10MHZ"
                | "80MHZ"
                | "160MHZ"
                | "80+80MHZ"
        ),
        _ => matches!(
            normalized.as_str(),
            "HT20" | "HT40+" | "HT40-" | "80MHZ" | "160MHZ" | "80+80MHZ"
        ),
    }
}

fn modes_for_channel(requested: &[String], channel: u16) -> Vec<String> {
    let mut filtered = requested
        .iter()
        .map(|mode| mode.trim().to_string())
        .filter(|mode| !mode.is_empty())
        .filter(|mode| mode_allowed_for_channel(mode, channel))
        .collect::<Vec<_>>();
    filtered.sort();
    filtered.dedup();
    filtered
}

fn map_path(request_path: &str) -> PathBuf {
    let ui_root = resolve_ui_root();
    let trimmed = request_path.trim_start_matches('/');
    if trimmed.is_empty() {
        let dist_index = ui_root.join("dist").join("index.html");
        if dist_index.exists() {
            return dist_index;
        }
        return ui_root.join("index.html");
    }

    // Block path traversal attempts and keep file serving inside lovableUI.
    let candidate = Path::new(trimmed);
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return ui_root.join("index.html");
    }

    let dist_candidate = ui_root.join("dist").join(trimmed);
    if dist_candidate.exists() {
        return dist_candidate;
    }
    let root_candidate = ui_root.join(trimmed);
    if root_candidate.exists() {
        return root_candidate;
    }

    let dist_index = ui_root.join("dist").join("index.html");
    if dist_index.exists() {
        return dist_index;
    }
    ui_root.join("index.html")
}

fn resolve_ui_root() -> PathBuf {
    if let Ok(custom) = std::env::var("EASYWIFI_UI_ROOT") {
        let root = PathBuf::from(custom);
        if root.join("dist").join("index.html").exists() || root.join("index.html").exists() {
            return root;
        }
    }

    let mut candidates = Vec::new();
    candidates.push(PathBuf::from("lovableUI"));
    candidates.push(PathBuf::from("/home/user/EasyWiFi/lovableUI"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("lovableUI"));
            candidates.push(parent.join("../EasyWiFi/lovableUI"));
            candidates.push(parent.join("../share/easywifi/lovableUI"));
        }
    }

    for root in candidates {
        if root.join("dist").join("index.html").exists() || root.join("index.html").exists() {
            return root;
        }
    }
    PathBuf::from("lovableUI")
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "json" => "application/json; charset=utf-8",
        "map" => "application/json; charset=utf-8",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}

fn launch_browser(url: &str) -> Result<()> {
    let mut attempts: Vec<(String, Vec<String>, bool)> = Vec::new();
    attempts.push((
        "chromium-browser".to_string(),
        vec![
            "--new-window".to_string(),
            "--disable-gpu".to_string(),
            "--disable-extensions".to_string(),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            url.to_string(),
        ],
        true,
    ));
    attempts.push((
        "chromium".to_string(),
        vec![
            "--new-window".to_string(),
            "--disable-gpu".to_string(),
            "--disable-extensions".to_string(),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            url.to_string(),
        ],
        true,
    ));
    attempts.push(("xdg-open".to_string(), vec![url.to_string()], false));

    let mut last_error: Option<anyhow::Error> = None;
    for (bin, args, clear_chromium_flags) in attempts {
        let mut cmd = std::process::Command::new(&bin);
        cmd.args(&args);
        if clear_chromium_flags {
            cmd.env_remove("CHROMIUM_FLAGS");
        }
        match cmd.spawn() {
            Ok(_) => return Ok(()),
            Err(err) => {
                last_error = Some(anyhow::anyhow!("{}: {}", bin, err));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no browser launch method available")))
}

fn respond_json(stream: &mut TcpStream, body: &str) -> Result<()> {
    respond_status(
        stream,
        200,
        "OK",
        "application/json; charset=utf-8",
        body.as_bytes(),
    )
}

fn respond_status(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .context("failed writing header")?;
    stream.write_all(body).context("failed writing body")?;
    Ok(())
}

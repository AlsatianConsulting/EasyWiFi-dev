use crate::bluetooth::{self, BluetoothEvent, BluetoothRuntime, BluetoothScanConfig};
use crate::capture::{self, CaptureConfig, CaptureEvent, CaptureRuntime};
use crate::model::{AccessPointRecord, BluetoothDeviceRecord, ChannelUsagePoint, ClientRecord};
use crate::settings::{
    AppSettings, ChannelSelectionMode, InterfaceSettings, WatchlistDeviceType, WatchlistEntry,
};
use anyhow::{Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Default)]
struct RuntimeHandles {
    capture: Option<CaptureRuntime>,
    bluetooth: Option<BluetoothRuntime>,
}

struct WebState {
    settings: AppSettings,
    runtime: RuntimeHandles,
    access_points: HashMap<String, AccessPointRecord>,
    clients: HashMap<String, ClientRecord>,
    bluetooth_devices: HashMap<String, BluetoothDeviceRecord>,
    channel_usage: Vec<ChannelUsagePoint>,
    logs: Vec<String>,
}

#[derive(Serialize)]
struct StateResponse {
    scanning_wifi: bool,
    scanning_bluetooth: bool,
    access_points: Vec<AccessPointRecord>,
    clients: Vec<ClientRecord>,
    bluetooth_devices: Vec<BluetoothDeviceRecord>,
    channel_usage: Vec<ChannelUsagePoint>,
    logs: Vec<String>,
}

#[derive(Serialize)]
struct WebMetaResponse {
    interfaces: Vec<WebInterfaceInfo>,
    selected_interface: Option<String>,
    watchlist_entries: Vec<WebWatchlistEntry>,
}

#[derive(Serialize)]
struct WebInterfaceInfo {
    name: String,
    if_type: String,
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

pub fn run() -> Result<()> {
    let settings = AppSettings::load_from_disk().unwrap_or_default();
    let state = Arc::new(Mutex::new(WebState {
        settings,
        runtime: RuntimeHandles::default(),
        access_points: HashMap::new(),
        clients: HashMap::new(),
        bluetooth_devices: HashMap::new(),
        channel_usage: Vec::new(),
        logs: vec!["web ui initialized".to_string()],
    }));

    let (capture_tx, capture_rx) = unbounded::<CaptureEvent>();
    let (bluetooth_tx, bluetooth_rx) = unbounded::<BluetoothEvent>();
    spawn_event_pump(state.clone(), capture_rx, bluetooth_rx);

    let addr = "127.0.0.1:8787";
    let listener = TcpListener::bind(addr).with_context(|| format!("failed to bind web UI on {addr}"))?;
    eprintln!("EasyWiFi Web UI listening on http://{addr}");

    let _ = std::process::Command::new("xdg-open")
        .arg(format!("http://{addr}"))
        .spawn();

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
    let mut buf = [0_u8; 8192];
    let read = stream.read(&mut buf).context("failed reading request")?;
    if read == 0 {
        return Ok(());
    }

    let req_bytes = &buf[..read];
    let req = String::from_utf8_lossy(req_bytes);
    let mut lines = req.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/");
    let body = request_body_bytes(req_bytes);

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
            WebMetaResponse {
                interfaces,
                selected_interface,
                watchlist_entries,
            }
        };
        let body = serde_json::to_string(&payload).context("failed to serialize meta payload")?;
        return respond_json(&mut stream, &body);
    }

    if method == "POST" && path == "/api/scan/start" {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
        start_scans_if_needed(&mut s, &capture_tx, &bluetooth_tx);
        return respond_json(&mut stream, "{\"ok\":true}");
    }

    if method == "POST" && path == "/api/scan/stop" {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
        if let Some(runtime) = s.runtime.capture.take() {
            runtime.stop();
        }
        if let Some(runtime) = s.runtime.bluetooth.take() {
            runtime.stop();
        }
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
            if let Some(runtime) = s.runtime.capture.take() {
                runtime.stop();
            }
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
            if let Some(runtime) = s.runtime.capture.take() {
                runtime.stop();
            }
            start_scans_if_needed(&mut s, &capture_tx, &bluetooth_tx);
        }
        return respond_json(&mut stream, "{\"ok\":true}");
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
    if wifi_enabled && state.runtime.capture.is_none() {
        let config = CaptureConfig {
            interfaces: state.settings.interfaces.clone(),
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

    if state.settings.bluetooth_enabled && state.runtime.bluetooth.is_none() {
        let cfg = BluetoothScanConfig {
            controller: state.settings.bluetooth_controller.clone(),
            source: state.settings.bluetooth_scan_source,
            ubertooth_device: state.settings.ubertooth_device.clone(),
            scan_timeout_secs: state.settings.bluetooth_scan_timeout_secs,
            pause_ms: state.settings.bluetooth_scan_pause_ms,
        };
        state.runtime.bluetooth = Some(bluetooth::start_scan(cfg, bluetooth_tx.clone()));
        push_log(state, "Bluetooth scanning started".to_string());
    }
}

fn request_body_bytes(request: &[u8]) -> &[u8] {
    let Some(header_end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
        return &[];
    };
    &request[(header_end + 4)..]
}

fn map_path(request_path: &str) -> PathBuf {
    let trimmed = request_path.trim_start_matches('/');
    if trimmed.is_empty() {
        let dist_index = Path::new("lovableUI").join("dist").join("index.html");
        if dist_index.exists() {
            return dist_index;
        }
        return Path::new("lovableUI").join("index.html");
    }

    // Block path traversal attempts and keep file serving inside lovableUI.
    let candidate = Path::new(trimmed);
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Path::new("lovableUI").join("index.html");
    }

    let dist_candidate = Path::new("lovableUI").join("dist").join(trimmed);
    if dist_candidate.exists() {
        return dist_candidate;
    }
    let root_candidate = Path::new("lovableUI").join(trimmed);
    if root_candidate.exists() {
        return root_candidate;
    }

    let dist_index = Path::new("lovableUI").join("dist").join("index.html");
    if dist_index.exists() {
        return dist_index;
    }
    Path::new("lovableUI").join("index.html")
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

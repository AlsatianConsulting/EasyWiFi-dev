use crate::bluetooth::{self, BluetoothEvent, BluetoothRuntime, BluetoothScanConfig};
use crate::capture::{self, CaptureConfig, CaptureEvent, CaptureRuntime};
use crate::model::{AccessPointRecord, BluetoothDeviceRecord, ChannelUsagePoint, ClientRecord};
use crate::settings::AppSettings;
use anyhow::{Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use serde::Serialize;
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
    let req = String::from_utf8_lossy(&buf[..read]);
    let mut lines = req.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/");

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

    if method == "POST" && path == "/api/scan/start" {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("state mutex poisoned"))?;
        let wifi_enabled = s.settings.interfaces.iter().any(|i| i.enabled);
        if wifi_enabled && s.runtime.capture.is_none() {
            let config = CaptureConfig {
                interfaces: s.settings.interfaces.clone(),
                session_pcap_path: None,
                wifi_packet_header_mode: s.settings.wifi_packet_header_mode,
                wifi_frame_parsing_enabled: s.settings.enable_wifi_frame_parsing,
                geoip_city_db_path: None,
                gps_enabled: false,
                passive_only: false,
            };
            s.runtime.capture = Some(capture::start_capture(config, capture_tx));
            push_log(&mut s, "Wi-Fi scanning started".to_string());
        }

        if s.settings.bluetooth_enabled && s.runtime.bluetooth.is_none() {
            let cfg = BluetoothScanConfig {
                controller: s.settings.bluetooth_controller.clone(),
                source: s.settings.bluetooth_scan_source,
                ubertooth_device: s.settings.ubertooth_device.clone(),
                scan_timeout_secs: s.settings.bluetooth_scan_timeout_secs,
                pause_ms: s.settings.bluetooth_scan_pause_ms,
            };
            s.runtime.bluetooth = Some(bluetooth::start_scan(cfg, bluetooth_tx));
            push_log(&mut s, "Bluetooth scanning started".to_string());
        }
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

fn map_path(request_path: &str) -> PathBuf {
    let base = Path::new("lovableUI");
    match request_path {
        "/" => base.join("index.html"),
        "/index.css" => base.join("index.css"),
        "/app.js" => base.join("app.js"),
        "/favicon.ico" => base.join("favicon.ico"),
        "/placeholder.svg" => base.join("placeholder.svg"),
        _ => base.join("index.html"),
    }
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

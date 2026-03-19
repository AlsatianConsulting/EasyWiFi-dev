use crate::capture::{rssi_to_tone_hz, GeigerUpdate};
use crate::model::{
    BluetoothActiveEnumeration, BluetoothDeviceRecord, BluetoothGattCharacteristicRecord,
    BluetoothGattDescriptorRecord, BluetoothGattServiceRecord, BluetoothReadableAttributeRecord,
};
use crate::settings::BluetoothScanSource;
use anyhow::{Context, Result};
use chrono::Utc;
use crossbeam_channel::Sender;
use rand::Rng;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub const ALL_CONTROLLERS_ID: &str = "all";
pub const ALL_UBERTOOTH_DEVICES_ID: &str = "all";

#[derive(Debug, Clone)]
pub struct BluetoothControllerInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone)]
pub struct UbertoothDeviceInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct BluetoothScanConfig {
    pub controller: Option<String>,
    pub source: BluetoothScanSource,
    pub ubertooth_device: Option<String>,
    pub scan_timeout_secs: u64,
    pub pause_ms: u64,
}

impl Default for BluetoothScanConfig {
    fn default() -> Self {
        Self {
            controller: None,
            source: BluetoothScanSource::Bluez,
            ubertooth_device: None,
            scan_timeout_secs: 4,
            pause_ms: 500,
        }
    }
}

#[derive(Debug, Clone)]
pub enum BluetoothEvent {
    DeviceSeen(BluetoothDeviceRecord),
    Log(String),
}

pub struct BluetoothRuntime {
    stop_flag: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl BluetoothRuntime {
    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Debug, Clone)]
struct ScanHit {
    mac: String,
    name: Option<String>,
    rssi_dbm: Option<i32>,
}

fn bluetooth_source_label(source: &str, id: Option<&str>) -> String {
    match id.map(str::trim).filter(|value| !value.is_empty()) {
        Some(id) => format!("{}:{}", source, id),
        None => source.to_string(),
    }
}

fn bluetooth_selection_is_all(value: Option<&str>, all_token: &str) -> bool {
    matches!(value.map(str::trim), Some(token) if token == all_token)
}

fn resolve_bluez_targets(selection: Option<&str>) -> Vec<Option<String>> {
    if bluetooth_selection_is_all(selection, ALL_CONTROLLERS_ID) {
        let mut targets = list_controllers()
            .unwrap_or_default()
            .into_iter()
            .map(|controller| Some(controller.id))
            .collect::<Vec<_>>();
        if targets.is_empty() {
            targets.push(None);
        }
        return targets;
    }

    match selection.map(str::trim).filter(|value| !value.is_empty()) {
        Some("default") | None => vec![None],
        Some(controller) => vec![Some(controller.to_string())],
    }
}

fn resolve_ubertooth_targets(selection: Option<&str>) -> Vec<Option<String>> {
    if bluetooth_selection_is_all(selection, ALL_UBERTOOTH_DEVICES_ID) {
        let mut targets = list_ubertooth_devices()
            .unwrap_or_default()
            .into_iter()
            .map(|device| {
                if device.id == "default" {
                    None
                } else {
                    Some(device.id)
                }
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            targets.push(None);
        }
        return targets;
    }

    match selection.map(str::trim).filter(|value| !value.is_empty()) {
        Some("default") | None => vec![None],
        Some(device) => vec![Some(device.to_string())],
    }
}

#[derive(Debug, Clone)]
struct SigResolver {
    company_names: HashMap<u16, String>,
    uuid_names: HashMap<String, String>,
}

impl SigResolver {
    fn load_default() -> Self {
        let mut resolver = Self {
            company_names: HashMap::new(),
            uuid_names: HashMap::new(),
        };
        resolver.load_company_kismet_gz(PathBuf::from(
            "/usr/share/kismet/kismet_bluetooth_manuf.txt.gz",
        ));
        resolver.load_uuid_kismet_gz(PathBuf::from(
            "/usr/share/kismet/kismet_bluetooth_ids.txt.gz",
        ));
        resolver.load_company_csv(manifest_asset("bt_company_ids.csv"));
        resolver.load_uuid_csv(manifest_asset("bt_service_uuids.csv"));
        resolver
    }

    fn load_company_csv(&mut self, path: PathBuf) {
        let Ok(raw) = fs::read_to_string(path) else {
            return;
        };
        for line in raw.lines().skip(1) {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.splitn(2, ',');
            let Some(id_raw) = parts.next() else {
                continue;
            };
            let Some(name_raw) = parts.next() else {
                continue;
            };
            let id = parse_company_id(id_raw);
            let name = name_raw.trim().trim_matches('"');
            if let Some(id) = id {
                self.company_names.insert(id, name.to_string());
            }
        }
    }

    fn load_uuid_csv(&mut self, path: PathBuf) {
        let Ok(raw) = fs::read_to_string(path) else {
            return;
        };
        for line in raw.lines().skip(1) {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.splitn(2, ',');
            let Some(uuid_raw) = parts.next() else {
                continue;
            };
            let Some(name_raw) = parts.next() else {
                continue;
            };
            let uuid = normalize_assigned_uuid(uuid_raw);
            let name = name_raw.trim().trim_matches('"');
            if !uuid.is_empty() && !name.is_empty() {
                self.uuid_names.insert(uuid, name.to_string());
            }
        }
    }

    fn load_company_kismet_gz(&mut self, path: PathBuf) {
        let Ok(raw) = read_gzip_text(path) else {
            return;
        };
        for (id_raw, name_raw) in parse_tab_separated_mappings(&raw) {
            let Some(id) = parse_company_id(&id_raw) else {
                continue;
            };
            let name = name_raw.trim();
            if !name.is_empty() {
                self.company_names
                    .entry(id)
                    .or_insert_with(|| name.to_string());
            }
        }
    }

    fn load_uuid_kismet_gz(&mut self, path: PathBuf) {
        let Ok(raw) = read_gzip_text(path) else {
            return;
        };
        for (id_raw, name_raw) in parse_tab_separated_mappings(&raw) {
            let uuid = normalize_assigned_uuid(&id_raw);
            let name = name_raw.trim();
            if !uuid.is_empty() && !name.is_empty() {
                self.uuid_names
                    .entry(uuid)
                    .or_insert_with(|| name.to_string());
            }
        }
    }

    fn company_name(&self, id: u16) -> Option<String> {
        self.company_names.get(&id).cloned()
    }

    fn uuid_name(&self, uuid: &str) -> Option<String> {
        let normalized = normalize_assigned_uuid(uuid);
        self.uuid_names
            .get(&normalized)
            .cloned()
            .or_else(|| fallback_uuid_name(&normalized))
    }
}

pub fn list_controllers() -> Result<Vec<BluetoothControllerInfo>> {
    let output = Command::new("bluetoothctl")
        .arg("list")
        .output()
        .context("failed to run bluetoothctl list")?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    let re = Regex::new(r"Controller\s+([0-9A-Fa-f:]{17})\s+(.+)$").unwrap();
    for raw in String::from_utf8_lossy(&output.stdout).lines() {
        let line = clean_line(raw);
        let Some(caps) = re.captures(&line) else {
            continue;
        };
        let id = normalize_mac(
            caps.get(1)
                .map(|m| m.as_str())
                .unwrap_or_default()
                .to_string(),
        );
        let mut name = caps
            .get(2)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();
        let is_default = name.contains("[default]");
        name = name.replace("[default]", "").trim().to_string();
        out.push(BluetoothControllerInfo {
            id,
            name,
            is_default,
        });
    }

    Ok(out)
}

pub fn list_ubertooth_devices() -> Result<Vec<UbertoothDeviceInfo>> {
    let mut devices = Vec::new();

    let entries = fs::read_dir("/sys/bus/usb/devices").ok();
    if let Some(entries) = entries {
        for entry in entries.flatten() {
            let path = entry.path();
            let vendor = fs::read_to_string(path.join("idVendor"))
                .ok()
                .map(|v| v.trim().to_ascii_lowercase());
            let product = fs::read_to_string(path.join("idProduct"))
                .ok()
                .map(|v| v.trim().to_ascii_lowercase());
            if vendor.as_deref() != Some("1d50") || product.as_deref() != Some("6002") {
                continue;
            }

            let serial = fs::read_to_string(path.join("serial"))
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
            let manufacturer = fs::read_to_string(path.join("manufacturer"))
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
            let product_name = fs::read_to_string(path.join("product"))
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
            let bus_name = path
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("usb-device")
                .to_string();

            let id = serial.clone().unwrap_or_else(|| bus_name.clone());
            let name = format!(
                "{} {}{}",
                manufacturer.unwrap_or_else(|| "Great Scott Gadgets".to_string()),
                product_name.unwrap_or_else(|| "Ubertooth".to_string()),
                serial
                    .as_ref()
                    .map(|s| format!(" ({})", s))
                    .unwrap_or_default()
            );
            devices.push(UbertoothDeviceInfo { id, name });
        }
    }

    devices.sort_by(|a, b| a.id.cmp(&b.id));
    devices.dedup_by(|a, b| a.id == b.id);

    if devices.is_empty() && command_exists("ubertooth-btle") {
        devices.push(UbertoothDeviceInfo {
            id: "default".to_string(),
            name: "Default Ubertooth Device".to_string(),
        });
    }

    Ok(devices)
}

pub fn start_scan(config: BluetoothScanConfig, sender: Sender<BluetoothEvent>) -> BluetoothRuntime {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&stop_flag);
    let handle = thread::spawn(move || {
        run_scan_loop(config, sender, stop);
    });
    BluetoothRuntime {
        stop_flag,
        handle: Some(handle),
    }
}

pub fn start_geiger_mode(
    controller: Option<&str>,
    target_mac: &str,
    sender: Sender<GeigerUpdate>,
    stop_flag: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    let controller = controller.map(str::to_string);
    let target = normalize_mac(target_mac.to_string());
    thread::spawn(move || {
        let mut rng = rand::thread_rng();
        while !stop_flag.load(Ordering::Relaxed) {
            if let Some(ctrl) = controller.as_deref() {
                select_controller(ctrl);
            }

            let hits = run_scan_once_interruptible(2, Some(&stop_flag)).unwrap_or_default();
            let mut emitted = false;
            for hit in hits {
                if hit.mac == target {
                    if let Some(rssi) = hit.rssi_dbm {
                        let tone_hz = rssi_to_tone_hz(rssi);
                        let _ = sender.send(GeigerUpdate {
                            rssi_dbm: rssi,
                            tone_hz,
                        });
                        emitted = true;
                    }
                    break;
                }
            }

            if !emitted {
                let fallback_rssi = rng.gen_range(-95..=-65);
                let _ = sender.send(GeigerUpdate {
                    rssi_dbm: fallback_rssi,
                    tone_hz: rssi_to_tone_hz(fallback_rssi),
                });
            }

            thread::sleep(Duration::from_millis(180));
        }
    })
}

pub fn connect_and_enumerate_device(
    controller: Option<&str>,
    mac: &str,
) -> Result<BluetoothDeviceRecord> {
    let normalized_mac = normalize_mac(mac.to_string());
    if normalized_mac.is_empty() {
        anyhow::bail!("invalid bluetooth device address");
    }

    if let Some(ctrl) = controller {
        select_controller(ctrl);
    }

    let connect_output = Command::new("bluetoothctl")
        .arg("--timeout")
        .arg("10")
        .arg("connect")
        .arg(&normalized_mac)
        .output()
        .with_context(|| format!("failed to run bluetoothctl connect {}", normalized_mac))?;
    let connect_stdout = clean_line(&String::from_utf8_lossy(&connect_output.stdout));
    let connect_stderr = clean_line(&String::from_utf8_lossy(&connect_output.stderr));
    let connect_error = normalize_connect_error(&normalized_mac, &connect_stdout, &connect_stderr);
    let mut record = read_device_state_internal(controller, &normalized_mac, connect_error)?;
    if let Some(active) = record.active_enumeration.as_mut() {
        if active.last_error.is_none()
            && !active.connected
            && !active.services_resolved
            && active.services.is_empty()
            && active.characteristics.is_empty()
        {
            active.last_error = Some(
                "device did not connect or did not expose enumerable GATT services".to_string(),
            );
        }
    }
    Ok(record)
}

pub fn read_device_state(controller: Option<&str>, mac: &str) -> Result<BluetoothDeviceRecord> {
    let normalized_mac = normalize_mac(mac.to_string());
    if normalized_mac.is_empty() {
        anyhow::bail!("invalid bluetooth device address");
    }
    if let Some(ctrl) = controller {
        select_controller(ctrl);
    }
    read_device_state_internal(controller, &normalized_mac, None)
}

pub fn disconnect_device(controller: Option<&str>, mac: &str) -> Result<()> {
    let normalized_mac = normalize_mac(mac.to_string());
    if normalized_mac.is_empty() {
        anyhow::bail!("invalid bluetooth device address");
    }

    if let Some(ctrl) = controller {
        select_controller(ctrl);
    }

    let output = Command::new("bluetoothctl")
        .arg("--timeout")
        .arg("6")
        .arg("disconnect")
        .arg(&normalized_mac)
        .output()
        .with_context(|| format!("failed to run bluetoothctl disconnect {}", normalized_mac))?;

    let stdout = clean_line(&String::from_utf8_lossy(&output.stdout));
    let stderr = clean_line(&String::from_utf8_lossy(&output.stderr));
    let combined = format!("{} {}", stdout.trim(), stderr.trim())
        .trim()
        .to_string();

    if !output.status.success()
        || combined.contains("Failed")
        || combined.contains("not available")
        || combined.contains("No default controller")
    {
        anyhow::bail!(
            "{}",
            if combined.is_empty() {
                format!("bluetooth disconnect failed for {}", normalized_mac)
            } else {
                combined
            }
        );
    }

    Ok(())
}

fn read_device_state_internal(
    controller: Option<&str>,
    normalized_mac: &str,
    active_error: Option<String>,
) -> Result<BluetoothDeviceRecord> {
    let resolver = SigResolver::load_default();
    let now = Utc::now();
    let mut record = query_device_info(controller, normalized_mac, &resolver)
        .unwrap_or_else(|| BluetoothDeviceRecord::new(normalized_mac.to_string(), now));
    record.last_seen = now;

    let adapter_path = resolve_adapter_path(controller)?;
    let device_path = format!(
        "{}/dev_{}",
        adapter_path,
        normalized_mac.replace(':', "_").to_ascii_uppercase()
    );

    let mut active = enumerate_active_details(&resolver, &device_path)?;
    active.last_enumerated = Some(now);
    active.last_error = active_error;
    record.active_enumeration = Some(active);
    Ok(record)
}

fn run_scan_loop(
    config: BluetoothScanConfig,
    sender: Sender<BluetoothEvent>,
    stop_flag: Arc<AtomicBool>,
) {
    let bluez_available = command_exists("bluetoothctl");
    let ubertooth_available = command_exists("ubertooth-btle");
    let wants_bluez = matches!(
        config.source,
        BluetoothScanSource::Bluez | BluetoothScanSource::Both
    );
    let wants_ubertooth = matches!(
        config.source,
        BluetoothScanSource::Ubertooth | BluetoothScanSource::Both
    );
    let use_bluez = wants_bluez && bluez_available;
    let use_ubertooth = wants_ubertooth && ubertooth_available;

    if wants_bluez && !bluez_available {
        let _ = sender.send(BluetoothEvent::Log(
            "bluetoothctl not found; BlueZ scanning disabled".to_string(),
        ));
    }
    if wants_ubertooth && !ubertooth_available {
        let _ = sender.send(BluetoothEvent::Log(
            "ubertooth-btle not found; Ubertooth scanning disabled".to_string(),
        ));
    }

    if !use_bluez && !use_ubertooth {
        let _ = sender.send(BluetoothEvent::Log(
            "no Bluetooth scan backend available; running simulated bluetooth scanner".to_string(),
        ));
        run_simulated_scan(sender, stop_flag);
        return;
    }

    let resolver = SigResolver::load_default();
    let mut cache: HashMap<String, BluetoothDeviceRecord> = HashMap::new();
    let mut last_refresh: HashMap<String, Instant> = HashMap::new();
    let info_refresh_period = Duration::from_secs(35);
    let pause = Duration::from_millis(config.pause_ms.max(100));
    let scan_timeout = config.scan_timeout_secs.clamp(2, 12);
    let bluez_targets = if use_bluez {
        resolve_bluez_targets(config.controller.as_deref())
    } else {
        Vec::new()
    };
    let ubertooth_targets = if use_ubertooth {
        resolve_ubertooth_targets(config.ubertooth_device.as_deref())
    } else {
        Vec::new()
    };

    while !stop_flag.load(Ordering::Relaxed) {
        let mut process_hits = |hits: Vec<ScanHit>,
                                is_ble_only: bool,
                                source_label: &str,
                                info_controller: Option<&str>| {
            for hit in hits {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }

                let now = Utc::now();
                let should_refresh = use_bluez
                    && last_refresh
                        .get(&hit.mac)
                        .map(|t| t.elapsed() >= info_refresh_period)
                        .unwrap_or(true);

                if should_refresh {
                    if let Some(fresh) = query_device_info(info_controller, &hit.mac, &resolver) {
                        cache.insert(hit.mac.clone(), fresh);
                        last_refresh.insert(hit.mac.clone(), Instant::now());
                    }
                }

                let mut record = cache
                    .get(&hit.mac)
                    .cloned()
                    .unwrap_or_else(|| BluetoothDeviceRecord::new(hit.mac.clone(), now));

                if let Some(name) = hit.name.filter(|v| !v.trim().is_empty()) {
                    if name.replace('-', ":").to_ascii_uppercase() != record.mac.as_str() {
                        record.advertised_name = Some(name);
                    }
                }
                if hit.rssi_dbm.is_some() {
                    record.rssi_dbm = hit.rssi_dbm;
                }
                if record.transport == "Unknown" {
                    record.transport = if is_ble_only {
                        "BLE".to_string()
                    } else {
                        "BT/BLE".to_string()
                    };
                }
                if !record
                    .source_adapters
                    .iter()
                    .any(|adapter| adapter == source_label)
                {
                    record.source_adapters.push(source_label.to_string());
                }
                record.last_seen = now;

                cache.insert(hit.mac.clone(), record.clone());
                let _ = sender.send(BluetoothEvent::DeviceSeen(record));
            }
        };

        if use_bluez {
            for controller in &bluez_targets {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                if let Some(controller_id) = controller.as_deref() {
                    select_controller(controller_id);
                }
                match run_scan_once_interruptible(scan_timeout, Some(&stop_flag)) {
                    Ok(hits) => process_hits(
                        hits,
                        false,
                        &bluetooth_source_label("bluez", controller.as_deref()),
                        controller.as_deref(),
                    ),
                    Err(err) => {
                        if stop_flag.load(Ordering::Relaxed) {
                            break;
                        }
                        let _ = sender.send(BluetoothEvent::Log(format!(
                            "BlueZ bluetooth scan failed{}: {err}",
                            controller
                                .as_deref()
                                .map(|id| format!(" on {}", id))
                                .unwrap_or_default()
                        )));
                    }
                }
            }
        }

        if use_ubertooth {
            for device_hint in &ubertooth_targets {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                match run_ubertooth_scan_once_interruptible(
                    scan_timeout,
                    device_hint.as_deref(),
                    Some(&stop_flag),
                ) {
                    Ok((hits, links)) => {
                        process_hits(
                            hits,
                            true,
                            &bluetooth_source_label("ubertooth", device_hint.as_deref()),
                            None,
                        );
                        for link in links {
                            let _ = sender.send(BluetoothEvent::Log(link));
                        }
                    }
                    Err(err) => {
                        if stop_flag.load(Ordering::Relaxed) {
                            break;
                        }
                        let _ = sender.send(BluetoothEvent::Log(format!(
                            "Ubertooth scan failed{}: {err}",
                            device_hint
                                .as_deref()
                                .filter(|value| !value.is_empty())
                                .map(|id| format!(" on {}", id))
                                .unwrap_or_default()
                        )));
                    }
                }
            }
        }

        thread::sleep(pause);
    }
}

fn command_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_simulated_scan(sender: Sender<BluetoothEvent>, stop_flag: Arc<AtomicBool>) {
    let mut rng = rand::thread_rng();
    let mut tick: u64 = 0;
    while !stop_flag.load(Ordering::Relaxed) {
        let now = Utc::now();
        let mac = format!(
            "C0:DE:BE:EF:{:02X}:{:02X}",
            (tick % 255) as u8,
            ((tick / 2) % 255) as u8
        );
        let mut record = BluetoothDeviceRecord::new(mac, now);
        record.transport = if tick % 2 == 0 {
            "BLE".to_string()
        } else {
            "BT".to_string()
        };
        record.source_adapters = vec!["simulated".to_string()];
        record.device_type = Some(if tick % 3 == 0 {
            "Headset".to_string()
        } else {
            "Phone".to_string()
        });
        record.advertised_name = Some(format!("SimBT-{}", tick % 12));
        record.rssi_dbm = Some(rng.gen_range(-90..=-40));
        record.alias = record.advertised_name.clone();
        let _ = sender.send(BluetoothEvent::DeviceSeen(record));
        tick += 1;
        thread::sleep(Duration::from_millis(750));
    }
}

fn run_scan_once_interruptible(
    scan_timeout_secs: u64,
    stop_flag: Option<&Arc<AtomicBool>>,
) -> Result<Vec<ScanHit>> {
    let mut child = Command::new("bluetoothctl")
        .arg("--timeout")
        .arg(scan_timeout_secs.to_string())
        .arg("scan")
        .arg("on")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to run bluetoothctl scan on")?;

    loop {
        if stop_flag
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
        {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Vec::new());
        }

        if let Some(status) = child
            .try_wait()
            .context("failed to poll bluetoothctl scan")?
        {
            let mut stdout = String::new();
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stdout.take() {
                let _ = pipe.read_to_string(&mut stdout);
            }
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            if !status.success() {
                let detail = if stderr.trim().is_empty() {
                    format!("bluetoothctl scan on exited with status {}", status)
                } else {
                    stderr.trim().to_string()
                };
                anyhow::bail!("{}", detail);
            }
            return Ok(parse_scan_output(&stdout));
        }

        thread::sleep(Duration::from_millis(120));
    }
}

fn run_ubertooth_scan_once_interruptible(
    scan_timeout_secs: u64,
    device_hint: Option<&str>,
    stop_flag: Option<&Arc<AtomicBool>>,
) -> Result<(Vec<ScanHit>, Vec<String>)> {
    let mut cmd = Command::new("ubertooth-btle");
    cmd.arg("-f").stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(device_hint) = device_hint.map(str::trim).filter(|v| !v.is_empty()) {
        if device_hint != "default" {
            cmd.args(["-U", device_hint]);
        }
    }

    let mut child = cmd.spawn().context("failed to run ubertooth-btle -f")?;
    let started = Instant::now();
    let timeout = Duration::from_secs(scan_timeout_secs.clamp(2, 12));

    loop {
        if stop_flag
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
            || started.elapsed() >= timeout
        {
            let _ = child.kill();
        }

        if let Some(_status) = child.try_wait().context("failed to poll ubertooth-btle")? {
            let mut stdout = String::new();
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stdout.take() {
                let _ = pipe.read_to_string(&mut stdout);
            }
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }

            let mut combined = String::new();
            if !stdout.is_empty() {
                combined.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(&stderr);
            }

            let hits = parse_ubertooth_output(&combined);
            let links = parse_ubertooth_links(&combined);
            if hits.is_empty() && links.is_empty() && combined.trim().is_empty() {
                anyhow::bail!("ubertooth-btle produced no output");
            }
            return Ok((hits, links));
        }

        thread::sleep(Duration::from_millis(100));
    }
}

fn parse_scan_output(text: &str) -> Vec<ScanHit> {
    let mut hits: HashMap<String, ScanHit> = HashMap::new();
    let device_re = Regex::new(r"Device\s+([0-9A-Fa-f:]{17})\s*(.*)$").unwrap();
    let rssi_re = Regex::new(r"RSSI:\s*(-?\d+)").unwrap();

    for raw in text.lines() {
        let line = clean_line(raw);
        let Some(caps) = device_re.captures(&line) else {
            continue;
        };

        let mac = normalize_mac(
            caps.get(1)
                .map(|m| m.as_str())
                .unwrap_or_default()
                .to_string(),
        );
        if mac.is_empty() {
            continue;
        }
        let rest = caps
            .get(2)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();

        let entry = hits.entry(mac.clone()).or_insert_with(|| ScanHit {
            mac: mac.clone(),
            name: None,
            rssi_dbm: None,
        });

        if let Some(rssi_caps) = rssi_re.captures(&rest) {
            entry.rssi_dbm = rssi_caps
                .get(1)
                .and_then(|m| m.as_str().parse::<i32>().ok());
            continue;
        }

        if rest.starts_with("TxPower:")
            || rest.starts_with("ManufacturerData")
            || rest.is_empty()
            || rest.eq_ignore_ascii_case(&mac.replace(':', "-"))
        {
            continue;
        }

        entry.name = Some(rest);
    }

    hits.into_values().collect()
}

fn parse_ubertooth_output(text: &str) -> Vec<ScanHit> {
    let mac_re = Regex::new(r"(?i)\b([0-9a-f]{2}(?::[0-9a-f]{2}){5})\b").unwrap();
    let rssi_re = Regex::new(r"(?i)(?:rssi\s*[:=]\s*|)(-?\d+)\s*d?bm\b").unwrap();
    let local_name_re =
        Regex::new(r"(?i)(?:name|complete local name)\s*[:=]\s*([A-Za-z0-9 _\-.]+)").unwrap();
    let mut hits: HashMap<String, ScanHit> = HashMap::new();

    for raw in text.lines() {
        let line = clean_line(raw);
        let Some(mac_caps) = mac_re.captures(&line) else {
            continue;
        };
        let mac = normalize_mac(
            mac_caps
                .get(1)
                .map(|m| m.as_str())
                .unwrap_or_default()
                .to_string(),
        );
        if mac.is_empty() {
            continue;
        }

        let entry = hits.entry(mac.clone()).or_insert_with(|| ScanHit {
            mac: mac.clone(),
            name: None,
            rssi_dbm: None,
        });

        if let Some(caps) = rssi_re.captures(&line) {
            if let Some(parsed) = caps.get(1).and_then(|m| m.as_str().parse::<i32>().ok()) {
                entry.rssi_dbm = Some(parsed.clamp(-127, 20));
            }
        }

        if let Some(caps) = local_name_re.captures(&line) {
            if let Some(name) = caps.get(1).map(|m| m.as_str().trim().to_string()) {
                if !name.is_empty() {
                    entry.name = Some(name);
                }
            }
        }
    }

    hits.into_values().collect()
}

fn parse_ubertooth_links(text: &str) -> Vec<String> {
    let init_re = Regex::new(r"(?i)(?:inita|initiator)\s*[:=]\s*([0-9a-f:]{17})").unwrap();
    let adv_re = Regex::new(r"(?i)(?:adva|advertiser)\s*[:=]\s*([0-9a-f:]{17})").unwrap();
    let mut out = Vec::new();

    for raw in text.lines() {
        let line = clean_line(raw);
        let Some(init) = init_re
            .captures(&line)
            .and_then(|caps| caps.get(1).map(|m| normalize_mac(m.as_str().to_string())))
        else {
            continue;
        };
        let Some(adv) = adv_re
            .captures(&line)
            .and_then(|caps| caps.get(1).map(|m| normalize_mac(m.as_str().to_string())))
        else {
            continue;
        };
        if init.is_empty() || adv.is_empty() {
            continue;
        }
        out.push(format!(
            "ubertooth observed BLE link candidate: {} -> {}",
            init, adv
        ));
    }

    out.sort();
    out.dedup();
    out
}

fn query_device_info(
    controller: Option<&str>,
    mac: &str,
    resolver: &SigResolver,
) -> Option<BluetoothDeviceRecord> {
    if let Some(ctrl) = controller {
        select_controller(ctrl);
    }

    let output = Command::new("bluetoothctl")
        .arg("--timeout")
        .arg("6")
        .arg("info")
        .arg(mac)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let now = Utc::now();
    let mut record = BluetoothDeviceRecord::new(mac.to_string(), now);
    let mut mfgr_id_set = HashSet::new();
    let mut uuid_set = HashSet::new();

    for raw in String::from_utf8_lossy(&output.stdout).lines() {
        let line = clean_line(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("Device ") {
            if let Some(open) = rest.rfind('(') {
                if let Some(close) = rest.rfind(')') {
                    if close > open + 1 {
                        let ty = rest[open + 1..close].trim().to_lowercase();
                        record.address_type = Some(ty.clone());
                        record.transport = if ty == "random" {
                            "BLE".to_string()
                        } else {
                            "BT/BLE".to_string()
                        };
                    }
                }
            }
            continue;
        }

        if let Some(value) = trimmed.strip_prefix("Name:") {
            record.advertised_name = Some(value.trim().to_string());
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("Alias:") {
            record.alias = Some(value.trim().to_string());
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("Icon:") {
            record.device_type = Some(icon_to_device_type(value.trim()));
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("Class:") {
            record.class_of_device = Some(value.trim().to_string());
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("RSSI:") {
            record.rssi_dbm = value.trim().parse::<i32>().ok();
            continue;
        }

        if let Some(value) = trimmed.strip_prefix("UUID:") {
            let raw = value.trim();
            if let Some((name, uuid)) = parse_uuid_line(raw) {
                let uuid_norm = normalize_assigned_uuid(&uuid);
                if uuid_set.insert(uuid_norm.clone()) {
                    record.uuids.push(uuid_norm.clone());
                }

                let label = if !name.is_empty() {
                    name
                } else {
                    resolver
                        .uuid_name(&uuid_norm)
                        .unwrap_or_else(|| "Unknown UUID".to_string())
                };

                if !record.uuid_names.contains(&label) {
                    record.uuid_names.push(label);
                }
            }
            continue;
        }

        if let Some(value) = trimmed.strip_prefix("ManufacturerData Key:") {
            let raw = value.trim();
            if let Some(id) = parse_company_id(raw) {
                let id_label = format!("0x{:04X}", id);
                if mfgr_id_set.insert(id_label.clone()) {
                    record.mfgr_ids.push(id_label);
                }
                if let Some(name) = resolver.company_name(id) {
                    if !record.mfgr_names.contains(&name) {
                        record.mfgr_names.push(name);
                    }
                }
            }
            continue;
        }
    }

    if record.device_type.is_none() {
        record.device_type = record
            .class_of_device
            .as_ref()
            .map(|_| "Classified Device".into());
    }

    Some(record)
}

fn normalize_connect_error(mac: &str, stdout: &str, stderr: &str) -> Option<String> {
    let combined = format!("{} {}", stdout.trim(), stderr.trim())
        .trim()
        .to_string();
    if combined.is_empty() {
        return None;
    }

    if combined.contains("Connection successful")
        || combined.contains("Successful connected")
        || combined.contains("already connected")
        || combined.contains("org.bluez.Error.AlreadyConnected")
    {
        return None;
    }

    if combined.contains("Failed") || combined.contains("not available") {
        return Some(format!("bluetooth connect {} failed: {}", mac, combined));
    }

    None
}

fn resolve_adapter_path(controller: Option<&str>) -> Result<String> {
    let output = Command::new("busctl")
        .args(["--system", "--list", "tree", "org.bluez"])
        .output()
        .context("failed to list BlueZ object tree")?;
    if !output.status.success() {
        anyhow::bail!(
            "{}",
            String::from_utf8_lossy(&output.stderr).trim().to_string()
        );
    }

    let paths = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with("/org/bluez/hci") && line.split('/').count() == 4 && !line.is_empty()
        })
        .map(str::to_string)
        .collect::<Vec<_>>();

    if paths.is_empty() {
        anyhow::bail!("no BlueZ adapters found");
    }

    if let Some(controller) = controller {
        for path in &paths {
            if busctl_get_string_property(path, "org.bluez.Adapter1", "Address")
                .ok()
                .flatten()
                .as_deref()
                == Some(controller)
            {
                return Ok(path.clone());
            }
        }
        anyhow::bail!(
            "selected bluetooth controller {} is not available",
            controller
        );
    }

    Ok(paths[0].clone())
}

fn enumerate_active_details(
    resolver: &SigResolver,
    device_path: &str,
) -> Result<BluetoothActiveEnumeration> {
    let connected = busctl_get_bool_property(device_path, "org.bluez.Device1", "Connected")
        .ok()
        .flatten()
        .unwrap_or(false);
    let services_resolved = if connected {
        wait_for_services_resolved(device_path)
    } else {
        busctl_get_bool_property(device_path, "org.bluez.Device1", "ServicesResolved")
            .ok()
            .flatten()
            .unwrap_or(false)
    };
    let mut out = BluetoothActiveEnumeration {
        connected,
        paired: busctl_get_bool_property(device_path, "org.bluez.Device1", "Paired")
            .ok()
            .flatten()
            .unwrap_or(false),
        trusted: busctl_get_bool_property(device_path, "org.bluez.Device1", "Trusted")
            .ok()
            .flatten()
            .unwrap_or(false),
        blocked: busctl_get_bool_property(device_path, "org.bluez.Device1", "Blocked")
            .ok()
            .flatten()
            .unwrap_or(false),
        services_resolved,
        tx_power_dbm: busctl_get_i32_property(device_path, "org.bluez.Device1", "TxPower")
            .ok()
            .flatten(),
        battery_percent: busctl_get_u8_property(device_path, "org.bluez.Battery1", "Percentage")
            .ok()
            .flatten(),
        appearance_code: busctl_get_u16_property(device_path, "org.bluez.Device1", "Appearance")
            .ok()
            .flatten(),
        appearance_name: None,
        icon: busctl_get_string_property(device_path, "org.bluez.Device1", "Icon")
            .ok()
            .flatten(),
        modalias: busctl_get_string_property(device_path, "org.bluez.Device1", "Modalias")
            .ok()
            .flatten(),
        ..BluetoothActiveEnumeration::default()
    };
    out.appearance_name = out
        .appearance_code
        .map(|code| appearance_name(code).to_string());

    let output = Command::new("busctl")
        .args(["--system", "--list", "tree", "org.bluez"])
        .output()
        .with_context(|| format!("failed to enumerate BlueZ object tree for {}", device_path))?;
    if !output.status.success() {
        anyhow::bail!(
            "{}",
            String::from_utf8_lossy(&output.stderr).trim().to_string()
        );
    }

    let mut service_name_by_path: HashMap<String, (String, Option<String>)> = HashMap::new();
    let mut service_name_by_uuid: HashMap<String, String> = HashMap::new();
    let mut characteristic_name_by_path: HashMap<String, (String, Option<String>)> = HashMap::new();

    for path in String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with(device_path))
    {
        if path.contains("/service") && !path.contains("/char") {
            let uuid = busctl_get_string_property(path, "org.bluez.GattService1", "UUID")
                .ok()
                .flatten()
                .map(|value| normalize_assigned_uuid(&value))
                .unwrap_or_default();
            if uuid.is_empty() {
                continue;
            }
            let primary = busctl_get_bool_property(path, "org.bluez.GattService1", "Primary")
                .ok()
                .flatten()
                .unwrap_or(false);
            let name = resolver.uuid_name(&uuid);
            if let Some(label) = &name {
                service_name_by_uuid.insert(uuid.clone(), label.clone());
            }
            service_name_by_path.insert(path.to_string(), (uuid.clone(), name.clone()));
            out.services.push(BluetoothGattServiceRecord {
                path: path.to_string(),
                uuid: uuid.clone(),
                name,
                primary,
            });
        }
    }

    for path in String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with(device_path))
    {
        if !path.contains("/char") {
            continue;
        }
        let uuid = busctl_get_string_property(path, "org.bluez.GattCharacteristic1", "UUID")
            .ok()
            .flatten()
            .map(|value| normalize_assigned_uuid(&value))
            .unwrap_or_default();
        if uuid.is_empty() {
            continue;
        }
        let service_path =
            busctl_get_string_property(path, "org.bluez.GattCharacteristic1", "Service")
                .ok()
                .flatten();
        let service_uuid = service_path.as_ref().and_then(|candidate| {
            service_name_by_path
                .get(candidate)
                .map(|entry| entry.0.clone())
        });
        let service_name = service_path
            .as_ref()
            .and_then(|candidate| service_name_by_path.get(candidate))
            .and_then(|entry| entry.1.clone())
            .or_else(|| {
                service_uuid
                    .as_ref()
                    .and_then(|candidate| service_name_by_uuid.get(candidate).cloned())
            });
        let flags =
            busctl_get_string_array_property(path, "org.bluez.GattCharacteristic1", "Flags")
                .ok()
                .flatten()
                .unwrap_or_default();
        let name = resolver.uuid_name(&uuid);
        if flags.iter().any(|flag| flag == "read") && should_read_characteristic_value(&uuid) {
            if let Ok(Some(value)) = read_gatt_characteristic_value(path, &uuid) {
                out.readable_attributes
                    .push(BluetoothReadableAttributeRecord {
                        uuid: uuid.clone(),
                        name: name.clone(),
                        value,
                    });
            }
        }

        characteristic_name_by_path.insert(path.to_string(), (uuid.clone(), name.clone()));

        out.characteristics.push(BluetoothGattCharacteristicRecord {
            path: path.to_string(),
            uuid: uuid.clone(),
            name,
            service_uuid,
            service_name,
            flags,
        });
    }

    for path in String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with(device_path))
    {
        if !path.contains("/desc") {
            continue;
        }
        let uuid = busctl_get_string_property(path, "org.bluez.GattDescriptor1", "UUID")
            .ok()
            .flatten()
            .map(|value| normalize_assigned_uuid(&value))
            .unwrap_or_default();
        if uuid.is_empty() {
            continue;
        }
        let characteristic_path =
            busctl_get_string_property(path, "org.bluez.GattDescriptor1", "Characteristic")
                .ok()
                .flatten();
        let characteristic_uuid = characteristic_path.as_ref().and_then(|candidate| {
            characteristic_name_by_path
                .get(candidate)
                .map(|entry| entry.0.clone())
        });
        let characteristic_name = characteristic_path
            .as_ref()
            .and_then(|candidate| characteristic_name_by_path.get(candidate))
            .and_then(|entry| entry.1.clone())
            .or_else(|| {
                characteristic_uuid
                    .as_ref()
                    .and_then(|candidate| resolver.uuid_name(candidate))
            });
        let value = if should_read_descriptor_value(&uuid) {
            read_gatt_descriptor_value(path, &uuid).ok().flatten()
        } else {
            None
        };
        out.descriptors.push(BluetoothGattDescriptorRecord {
            path: path.to_string(),
            uuid: uuid.clone(),
            name: resolver.uuid_name(&uuid),
            characteristic_uuid,
            characteristic_name,
            value,
        });
    }

    out.services.sort_by(|a, b| {
        a.primary
            .cmp(&b.primary)
            .reverse()
            .then_with(|| {
                a.name
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.name.as_deref().unwrap_or(""))
            })
            .then_with(|| a.uuid.cmp(&b.uuid))
    });
    out.characteristics.sort_by(|a, b| {
        a.service_name
            .as_deref()
            .unwrap_or("")
            .cmp(b.service_name.as_deref().unwrap_or(""))
            .then_with(|| {
                a.name
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.name.as_deref().unwrap_or(""))
            })
            .then_with(|| a.uuid.cmp(&b.uuid))
    });
    out.readable_attributes.sort_by(|a, b| {
        a.name
            .as_deref()
            .unwrap_or("")
            .cmp(b.name.as_deref().unwrap_or(""))
            .then_with(|| a.uuid.cmp(&b.uuid))
    });
    out.descriptors.sort_by(|a, b| {
        a.characteristic_name
            .as_deref()
            .unwrap_or("")
            .cmp(b.characteristic_name.as_deref().unwrap_or(""))
            .then_with(|| {
                a.name
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.name.as_deref().unwrap_or(""))
            })
            .then_with(|| a.uuid.cmp(&b.uuid))
    });

    Ok(out)
}

fn wait_for_services_resolved(device_path: &str) -> bool {
    let started = Instant::now();
    while started.elapsed() <= Duration::from_secs(8) {
        if busctl_get_bool_property(device_path, "org.bluez.Device1", "ServicesResolved")
            .ok()
            .flatten()
            .unwrap_or(false)
        {
            return true;
        }
        thread::sleep(Duration::from_millis(250));
    }
    false
}

fn gdbus_get_property_raw(object_path: &str, interface: &str, property: &str) -> Result<String> {
    let output = Command::new("gdbus")
        .args([
            "--system",
            "call",
            "--dest",
            "org.bluez",
            "--object-path",
            object_path,
            "--method",
            "org.freedesktop.DBus.Properties.Get",
            interface,
            property,
        ])
        .output()
        .with_context(|| {
            format!(
                "failed to query BlueZ property {} {} {}",
                object_path, interface, property
            )
        })?;

    if !output.status.success() {
        anyhow::bail!(
            "{}",
            String::from_utf8_lossy(&output.stderr).trim().to_string()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn busctl_get_string_property(
    object_path: &str,
    interface: &str,
    property: &str,
) -> Result<Option<String>> {
    Ok(gdbus_get_property_raw(object_path, interface, property)
        .ok()
        .as_deref()
        .and_then(parse_gvariant_string))
}

fn busctl_get_bool_property(
    object_path: &str,
    interface: &str,
    property: &str,
) -> Result<Option<bool>> {
    Ok(gdbus_get_property_raw(object_path, interface, property)
        .ok()
        .as_deref()
        .and_then(parse_gvariant_bool))
}

fn busctl_get_i32_property(
    object_path: &str,
    interface: &str,
    property: &str,
) -> Result<Option<i32>> {
    Ok(gdbus_get_property_raw(object_path, interface, property)
        .ok()
        .as_deref()
        .and_then(parse_gvariant_i64)
        .and_then(|value| i32::try_from(value).ok()))
}

fn busctl_get_u16_property(
    object_path: &str,
    interface: &str,
    property: &str,
) -> Result<Option<u16>> {
    Ok(gdbus_get_property_raw(object_path, interface, property)
        .ok()
        .as_deref()
        .and_then(parse_gvariant_i64)
        .and_then(|value| u16::try_from(value).ok()))
}

fn busctl_get_u8_property(
    object_path: &str,
    interface: &str,
    property: &str,
) -> Result<Option<u8>> {
    Ok(gdbus_get_property_raw(object_path, interface, property)
        .ok()
        .as_deref()
        .and_then(parse_gvariant_i64)
        .and_then(|value| u8::try_from(value).ok()))
}

fn busctl_get_string_array_property(
    object_path: &str,
    interface: &str,
    property: &str,
) -> Result<Option<Vec<String>>> {
    Ok(gdbus_get_property_raw(object_path, interface, property)
        .ok()
        .as_deref()
        .map(parse_gvariant_string_array))
}

fn parse_gvariant_string(raw: &str) -> Option<String> {
    let re = Regex::new(r"'([^']*)'").ok()?;
    re.captures(raw)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
}

fn parse_gvariant_bool(raw: &str) -> Option<bool> {
    if raw.contains("<true>") {
        Some(true)
    } else if raw.contains("<false>") {
        Some(false)
    } else {
        None
    }
}

fn parse_gvariant_i64(raw: &str) -> Option<i64> {
    let re = Regex::new(r"<(?:[A-Za-z0-9_]+\s+)?(-?(?:0x[0-9A-Fa-f]+|\d+))>").ok()?;
    let text = re
        .captures(raw)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))?;
    if let Some(hex) = text.strip_prefix("0x") {
        i64::from_str_radix(hex, 16).ok()
    } else {
        text.parse::<i64>().ok()
    }
}

fn parse_gvariant_string_array(raw: &str) -> Vec<String> {
    let re = Regex::new(r"'([^']*)'").unwrap();
    re.captures_iter(raw)
        .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

fn read_gzip_text(path: PathBuf) -> Result<String> {
    let output = Command::new("gzip")
        .arg("-cd")
        .arg(&path)
        .output()
        .with_context(|| format!("failed to read gzip text {}", path.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "{}",
            String::from_utf8_lossy(&output.stderr).trim().to_string()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_tab_separated_mappings(raw: &str) -> Vec<(String, String)> {
    raw.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let mut parts = line.splitn(2, '\t');
            let key = parts.next()?.trim();
            let value = parts.next()?.trim();
            if key.is_empty() || value.is_empty() {
                None
            } else {
                Some((key.to_string(), value.to_string()))
            }
        })
        .collect()
}

fn should_read_characteristic_value(uuid: &str) -> bool {
    matches!(
        normalize_assigned_uuid(uuid).as_str(),
        "00002a00-0000-1000-8000-00805f9b34fb"
            | "00002a01-0000-1000-8000-00805f9b34fb"
            | "00002a02-0000-1000-8000-00805f9b34fb"
            | "00002a03-0000-1000-8000-00805f9b34fb"
            | "00002a04-0000-1000-8000-00805f9b34fb"
            | "00002a05-0000-1000-8000-00805f9b34fb"
            | "00002a06-0000-1000-8000-00805f9b34fb"
            | "00002a07-0000-1000-8000-00805f9b34fb"
            | "00002a08-0000-1000-8000-00805f9b34fb"
            | "00002a09-0000-1000-8000-00805f9b34fb"
            | "00002a0a-0000-1000-8000-00805f9b34fb"
            | "00002a0d-0000-1000-8000-00805f9b34fb"
            | "00002a0e-0000-1000-8000-00805f9b34fb"
            | "00002a0f-0000-1000-8000-00805f9b34fb"
            | "00002a14-0000-1000-8000-00805f9b34fb"
            | "00002a19-0000-1000-8000-00805f9b34fb"
            | "00002a1d-0000-1000-8000-00805f9b34fb"
            | "00002a21-0000-1000-8000-00805f9b34fb"
            | "00002a23-0000-1000-8000-00805f9b34fb"
            | "00002a24-0000-1000-8000-00805f9b34fb"
            | "00002a25-0000-1000-8000-00805f9b34fb"
            | "00002a26-0000-1000-8000-00805f9b34fb"
            | "00002a27-0000-1000-8000-00805f9b34fb"
            | "00002a28-0000-1000-8000-00805f9b34fb"
            | "00002a29-0000-1000-8000-00805f9b34fb"
            | "00002a2b-0000-1000-8000-00805f9b34fb"
            | "00002a31-0000-1000-8000-00805f9b34fb"
            | "00002a38-0000-1000-8000-00805f9b34fb"
            | "00002a4a-0000-1000-8000-00805f9b34fb"
            | "00002a4b-0000-1000-8000-00805f9b34fb"
            | "00002a4d-0000-1000-8000-00805f9b34fb"
            | "00002a4e-0000-1000-8000-00805f9b34fb"
            | "00002a50-0000-1000-8000-00805f9b34fb"
    )
}

fn should_read_descriptor_value(uuid: &str) -> bool {
    matches!(
        normalize_assigned_uuid(uuid).as_str(),
        "00002900-0000-1000-8000-00805f9b34fb"
            | "00002901-0000-1000-8000-00805f9b34fb"
            | "00002902-0000-1000-8000-00805f9b34fb"
            | "00002904-0000-1000-8000-00805f9b34fb"
            | "00002906-0000-1000-8000-00805f9b34fb"
            | "00002907-0000-1000-8000-00805f9b34fb"
            | "00002908-0000-1000-8000-00805f9b34fb"
    )
}

fn read_gatt_characteristic_value(path: &str, uuid: &str) -> Result<Option<String>> {
    let output = Command::new("gdbus")
        .args([
            "call",
            "--system",
            "--dest",
            "org.bluez",
            "--object-path",
            path,
            "--method",
            "org.bluez.GattCharacteristic1.ReadValue",
            "{}",
        ])
        .output()
        .with_context(|| format!("failed to read Bluetooth characteristic {}", path))?;
    if !output.status.success() {
        return Ok(None);
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let bytes = parse_gdbus_byte_array(&raw);
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(decode_characteristic_value(uuid, &bytes)))
}

fn read_gatt_descriptor_value(path: &str, uuid: &str) -> Result<Option<String>> {
    let output = Command::new("gdbus")
        .args([
            "call",
            "--system",
            "--dest",
            "org.bluez",
            "--object-path",
            path,
            "--method",
            "org.bluez.GattDescriptor1.ReadValue",
            "{}",
        ])
        .output()
        .with_context(|| format!("failed to read Bluetooth descriptor {}", path))?;
    if !output.status.success() {
        return Ok(None);
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let bytes = parse_gdbus_byte_array(&raw);
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(decode_descriptor_value(uuid, &bytes)))
}

fn parse_gdbus_byte_array(raw: &str) -> Vec<u8> {
    let re = Regex::new(r"0x([0-9A-Fa-f]{2})").unwrap();
    re.captures_iter(raw)
        .filter_map(|caps| {
            caps.get(1)
                .and_then(|m| u8::from_str_radix(m.as_str(), 16).ok())
        })
        .collect()
}

fn decode_characteristic_value(uuid: &str, bytes: &[u8]) -> String {
    match normalize_assigned_uuid(uuid).as_str() {
        "00002a02-0000-1000-8000-00805f9b34fb" => bytes
            .first()
            .map(|value| {
                if value & 0x01 == 0x01 {
                    "Peripheral privacy enabled".to_string()
                } else {
                    "Peripheral privacy disabled".to_string()
                }
            })
            .unwrap_or_else(|| format_hex_bytes(bytes)),
        "00002a03-0000-1000-8000-00805f9b34fb" => format_mac_like(bytes),
        "00002a04-0000-1000-8000-00805f9b34fb" => decode_peripheral_connection_params(bytes),
        "00002a05-0000-1000-8000-00805f9b34fb" => decode_service_changed(bytes),
        "00002a06-0000-1000-8000-00805f9b34fb" => decode_alert_level(bytes),
        "00002a07-0000-1000-8000-00805f9b34fb" => bytes
            .first()
            .map(|value| format!("{} dBm", *value as i8))
            .unwrap_or_else(|| format_hex_bytes(bytes)),
        "00002a08-0000-1000-8000-00805f9b34fb" => decode_date_time(bytes),
        "00002a09-0000-1000-8000-00805f9b34fb" => decode_day_of_week(bytes),
        "00002a0a-0000-1000-8000-00805f9b34fb" => decode_day_date_time(bytes),
        "00002a0d-0000-1000-8000-00805f9b34fb" => decode_dst_offset(bytes),
        "00002a0e-0000-1000-8000-00805f9b34fb" => decode_time_zone(bytes),
        "00002a0f-0000-1000-8000-00805f9b34fb" => decode_local_time_information(bytes),
        "00002a14-0000-1000-8000-00805f9b34fb" => decode_reference_time_information(bytes),
        "00002a19-0000-1000-8000-00805f9b34fb" => bytes
            .first()
            .map(|value| format!("{value}%"))
            .unwrap_or_else(|| format_hex_bytes(bytes)),
        "00002a01-0000-1000-8000-00805f9b34fb" => {
            if bytes.len() >= 2 {
                let code = u16::from_le_bytes([bytes[0], bytes[1]]);
                format!("{} (0x{:04X})", appearance_name(code), code)
            } else {
                format_hex_bytes(bytes)
            }
        }
        "00002a1d-0000-1000-8000-00805f9b34fb" => decode_temperature_type(bytes),
        "00002a21-0000-1000-8000-00805f9b34fb" => decode_measurement_interval(bytes),
        "00002a23-0000-1000-8000-00805f9b34fb" => decode_system_id(bytes),
        "00002a00-0000-1000-8000-00805f9b34fb"
        | "00002a24-0000-1000-8000-00805f9b34fb"
        | "00002a25-0000-1000-8000-00805f9b34fb"
        | "00002a26-0000-1000-8000-00805f9b34fb"
        | "00002a27-0000-1000-8000-00805f9b34fb"
        | "00002a28-0000-1000-8000-00805f9b34fb"
        | "00002a29-0000-1000-8000-00805f9b34fb" => {
            String::from_utf8(bytes.to_vec()).unwrap_or_else(|_| format_hex_bytes(bytes))
        }
        "00002a2b-0000-1000-8000-00805f9b34fb" => decode_current_time(bytes),
        "00002a31-0000-1000-8000-00805f9b34fb" => decode_scan_refresh(bytes),
        "00002a38-0000-1000-8000-00805f9b34fb" => decode_body_sensor_location(bytes),
        "00002a4a-0000-1000-8000-00805f9b34fb" => decode_hid_information(bytes),
        "00002a4b-0000-1000-8000-00805f9b34fb" => {
            format!("HID Report Map ({} bytes)", bytes.len())
        }
        "00002a4d-0000-1000-8000-00805f9b34fb" => {
            format!(
                "HID Report ({} bytes): {}",
                bytes.len(),
                format_hex_bytes(bytes)
            )
        }
        "00002a4e-0000-1000-8000-00805f9b34fb" => decode_protocol_mode(bytes),
        "00002a50-0000-1000-8000-00805f9b34fb" => decode_pnp_id(bytes),
        _ => format_hex_bytes(bytes),
    }
}

fn decode_descriptor_value(uuid: &str, bytes: &[u8]) -> String {
    match normalize_assigned_uuid(uuid).as_str() {
        "00002900-0000-1000-8000-00805f9b34fb" => decode_characteristic_extended_properties(bytes),
        "00002901-0000-1000-8000-00805f9b34fb" => {
            String::from_utf8(bytes.to_vec()).unwrap_or_else(|_| format_hex_bytes(bytes))
        }
        "00002902-0000-1000-8000-00805f9b34fb" => decode_cccd(bytes),
        "00002904-0000-1000-8000-00805f9b34fb" => decode_presentation_format(bytes),
        "00002906-0000-1000-8000-00805f9b34fb" => decode_valid_range(bytes),
        "00002907-0000-1000-8000-00805f9b34fb" => decode_external_report_reference(bytes),
        "00002908-0000-1000-8000-00805f9b34fb" => decode_report_reference(bytes),
        _ => format_hex_bytes(bytes),
    }
}

fn decode_characteristic_extended_properties(bytes: &[u8]) -> String {
    if bytes.len() < 2 {
        return format_hex_bytes(bytes);
    }
    let flags = u16::from_le_bytes([bytes[0], bytes[1]]);
    let labels = [
        (flags & 0x0001 != 0, "reliable write"),
        (flags & 0x0002 != 0, "writable auxiliaries"),
    ]
    .into_iter()
    .filter_map(|(on, label)| on.then_some(label))
    .collect::<Vec<_>>();
    if labels.is_empty() {
        format!("none (0x{flags:04X})")
    } else {
        format!("{} (0x{flags:04X})", labels.join(", "))
    }
}

fn decode_cccd(bytes: &[u8]) -> String {
    if bytes.len() < 2 {
        return format_hex_bytes(bytes);
    }
    let flags = u16::from_le_bytes([bytes[0], bytes[1]]);
    let labels = [
        (flags & 0x0001 != 0, "notifications"),
        (flags & 0x0002 != 0, "indications"),
    ]
    .into_iter()
    .filter_map(|(on, label)| on.then_some(label))
    .collect::<Vec<_>>();
    if labels.is_empty() {
        format!("disabled (0x{flags:04X})")
    } else {
        format!("{} enabled (0x{flags:04X})", labels.join(", "))
    }
}

fn decode_presentation_format(bytes: &[u8]) -> String {
    if bytes.len() < 7 {
        return format_hex_bytes(bytes);
    }
    let format_code = bytes[0];
    let exponent = bytes[1] as i8;
    let unit = u16::from_le_bytes([bytes[2], bytes[3]]);
    let namespace = bytes[4];
    let description = u16::from_le_bytes([bytes[5], bytes[6]]);
    format!(
        "format 0x{format_code:02X}, exponent {exponent}, unit 0x{unit:04X}, namespace 0x{namespace:02X}, description 0x{description:04X}"
    )
}

fn decode_valid_range(bytes: &[u8]) -> String {
    if bytes.len() == 2 {
        return format!("min {}, max {}", bytes[0], bytes[1]);
    }
    if bytes.len() == 4 {
        let min = u16::from_le_bytes([bytes[0], bytes[1]]);
        let max = u16::from_le_bytes([bytes[2], bytes[3]]);
        return format!("min {}, max {}", min, max);
    }
    if bytes.len() == 8 {
        let min = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let max = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        return format!("min {}, max {}", min, max);
    }
    format_hex_bytes(bytes)
}

fn decode_external_report_reference(bytes: &[u8]) -> String {
    if bytes.len() == 2 {
        let short = u16::from_le_bytes([bytes[0], bytes[1]]);
        return normalize_assigned_uuid(&format!("{short:04x}"));
    }
    if bytes.len() == 16 {
        return format_uuid_from_le_bytes(bytes).unwrap_or_else(|| format_hex_bytes(bytes));
    }
    format_hex_bytes(bytes)
}

fn format_uuid_from_le_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.len() != 16 {
        return None;
    }
    let mut reversed = [0u8; 16];
    for (idx, value) in bytes.iter().rev().enumerate() {
        reversed[idx] = *value;
    }
    Some(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        reversed[0],
        reversed[1],
        reversed[2],
        reversed[3],
        reversed[4],
        reversed[5],
        reversed[6],
        reversed[7],
        reversed[8],
        reversed[9],
        reversed[10],
        reversed[11],
        reversed[12],
        reversed[13],
        reversed[14],
        reversed[15]
    ))
}

fn decode_report_reference(bytes: &[u8]) -> String {
    if bytes.len() < 2 {
        return format_hex_bytes(bytes);
    }
    let report_id = bytes[0];
    let report_type = match bytes[1] {
        1 => "Input Report",
        2 => "Output Report",
        3 => "Feature Report",
        _ => "Unknown Report Type",
    };
    format!("report ID {}, {}", report_id, report_type)
}

fn decode_peripheral_connection_params(bytes: &[u8]) -> String {
    if bytes.len() < 8 {
        return format_hex_bytes(bytes);
    }
    let min_interval = u16::from_le_bytes([bytes[0], bytes[1]]) as f32 * 1.25;
    let max_interval = u16::from_le_bytes([bytes[2], bytes[3]]) as f32 * 1.25;
    let latency = u16::from_le_bytes([bytes[4], bytes[5]]);
    let timeout_ms = u16::from_le_bytes([bytes[6], bytes[7]]) as u32 * 10;
    format!(
        "min {:.2} ms, max {:.2} ms, latency {}, supervision timeout {} ms",
        min_interval, max_interval, latency, timeout_ms
    )
}

fn decode_service_changed(bytes: &[u8]) -> String {
    if bytes.len() < 4 {
        return format_hex_bytes(bytes);
    }
    let start = u16::from_le_bytes([bytes[0], bytes[1]]);
    let end = u16::from_le_bytes([bytes[2], bytes[3]]);
    format!("handles 0x{start:04X} to 0x{end:04X}")
}

fn decode_alert_level(bytes: &[u8]) -> String {
    match bytes.first().copied() {
        Some(0) => "No Alert".to_string(),
        Some(1) => "Mild Alert".to_string(),
        Some(2) => "High Alert".to_string(),
        Some(_) => format_hex_bytes(bytes),
        None => format_hex_bytes(bytes),
    }
}

fn decode_date_time(bytes: &[u8]) -> String {
    if bytes.len() < 7 {
        return format_hex_bytes(bytes);
    }
    let year = u16::from_le_bytes([bytes[0], bytes[1]]);
    let month = bytes[2];
    let day = bytes[3];
    let hour = bytes[4];
    let minute = bytes[5];
    let second = bytes[6];
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

fn decode_day_of_week(bytes: &[u8]) -> String {
    match bytes.first().copied() {
        Some(1) => "Monday".to_string(),
        Some(2) => "Tuesday".to_string(),
        Some(3) => "Wednesday".to_string(),
        Some(4) => "Thursday".to_string(),
        Some(5) => "Friday".to_string(),
        Some(6) => "Saturday".to_string(),
        Some(7) => "Sunday".to_string(),
        Some(_) => format_hex_bytes(bytes),
        None => format_hex_bytes(bytes),
    }
}

fn decode_day_date_time(bytes: &[u8]) -> String {
    if bytes.len() < 8 {
        return format_hex_bytes(bytes);
    }
    format!(
        "{}, {}",
        decode_day_of_week(&bytes[7..8]),
        decode_date_time(bytes)
    )
}

fn decode_dst_offset(bytes: &[u8]) -> String {
    match bytes.first().copied() {
        Some(0) => "Standard Time".to_string(),
        Some(2) => "Half-hour Daylight Time".to_string(),
        Some(4) => "Daylight Time".to_string(),
        Some(8) => "Double Daylight Time".to_string(),
        Some(255) => "DST Offset Unknown".to_string(),
        Some(value) => format!("Reserved ({value})"),
        None => format_hex_bytes(bytes),
    }
}

fn decode_time_zone(bytes: &[u8]) -> String {
    match bytes.first().copied() {
        Some(128) => "Time Zone Unknown".to_string(),
        Some(value) => format!("UTC{:+.2}", (value as i8) as f32 / 4.0),
        None => format_hex_bytes(bytes),
    }
}

fn decode_local_time_information(bytes: &[u8]) -> String {
    if bytes.len() < 2 {
        return format_hex_bytes(bytes);
    }
    format!(
        "{}, {}",
        decode_time_zone(&bytes[0..1]),
        decode_dst_offset(&bytes[1..2])
    )
}

fn decode_reference_time_information(bytes: &[u8]) -> String {
    if bytes.len() < 4 {
        return format_hex_bytes(bytes);
    }
    let source = match bytes[0] {
        0 => "Unknown",
        1 => "Network Time Protocol",
        2 => "GPS",
        3 => "Radio Time Signal",
        4 => "Manual",
        5 => "Atomic Clock",
        6 => "Cellular Network",
        _ => "Reserved",
    };
    let accuracy = if bytes[1] == 253 {
        "Accuracy Out of Range".to_string()
    } else if bytes[1] == 254 {
        "Accuracy Unknown".to_string()
    } else {
        format!("{} eighths of a second", bytes[1])
    };
    let days = if bytes[2] == 255 {
        "unknown".to_string()
    } else {
        format!("{}", bytes[2])
    };
    let hours = if bytes[3] == 255 {
        "unknown".to_string()
    } else {
        format!("{}", bytes[3])
    };
    format!(
        "source {}, accuracy {}, {} day(s) since update, {} hour(s) since update",
        source, accuracy, days, hours
    )
}

fn decode_temperature_type(bytes: &[u8]) -> String {
    match bytes.first().copied() {
        Some(1) => "Armpit".to_string(),
        Some(2) => "Body".to_string(),
        Some(3) => "Ear".to_string(),
        Some(4) => "Finger".to_string(),
        Some(5) => "Gastro-intestinal Tract".to_string(),
        Some(6) => "Mouth".to_string(),
        Some(7) => "Rectum".to_string(),
        Some(8) => "Toe".to_string(),
        Some(9) => "Tympanum".to_string(),
        Some(_) => format_hex_bytes(bytes),
        None => format_hex_bytes(bytes),
    }
}

fn decode_measurement_interval(bytes: &[u8]) -> String {
    if bytes.len() < 2 {
        return format_hex_bytes(bytes);
    }
    let seconds = u16::from_le_bytes([bytes[0], bytes[1]]);
    format!("{seconds} second(s)")
}

fn decode_system_id(bytes: &[u8]) -> String {
    if bytes.len() < 8 {
        return format_hex_bytes(bytes);
    }
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]
    )
}

fn decode_current_time(bytes: &[u8]) -> String {
    if bytes.len() < 10 {
        return format_hex_bytes(bytes);
    }
    let fractions = (bytes[8] as f32) / 256.0;
    let adjust_reason = decode_adjust_reason(bytes[9]);
    format!(
        "{} ({}, fractions {:.3}s)",
        decode_day_date_time(bytes),
        adjust_reason,
        fractions
    )
}

fn decode_scan_refresh(bytes: &[u8]) -> String {
    match bytes.first().copied() {
        Some(0) => "No Refresh Required".to_string(),
        Some(1) => "Server Requires Refresh".to_string(),
        Some(_) => format_hex_bytes(bytes),
        None => format_hex_bytes(bytes),
    }
}

fn decode_body_sensor_location(bytes: &[u8]) -> String {
    match bytes.first().copied() {
        Some(0) => "Other".to_string(),
        Some(1) => "Chest".to_string(),
        Some(2) => "Wrist".to_string(),
        Some(3) => "Finger".to_string(),
        Some(4) => "Hand".to_string(),
        Some(5) => "Ear Lobe".to_string(),
        Some(6) => "Foot".to_string(),
        Some(_) => format_hex_bytes(bytes),
        None => format_hex_bytes(bytes),
    }
}

fn decode_hid_information(bytes: &[u8]) -> String {
    if bytes.len() < 4 {
        return format_hex_bytes(bytes);
    }
    let version = u16::from_le_bytes([bytes[0], bytes[1]]);
    let country_code = bytes[2];
    let flags = bytes[3];
    let flags_text = [
        (flags & 0x01 != 0, "remote wake"),
        (flags & 0x02 != 0, "normally connectable"),
    ]
    .into_iter()
    .filter_map(|(on, label)| on.then_some(label))
    .collect::<Vec<_>>();
    format!(
        "HID {:x}.{:02x}, country code {}, flags {}",
        version >> 8,
        version & 0x00FF,
        country_code,
        if flags_text.is_empty() {
            format!("0x{flags:02X}")
        } else {
            format!("{} (0x{flags:02X})", flags_text.join(", "))
        }
    )
}

fn decode_protocol_mode(bytes: &[u8]) -> String {
    match bytes.first().copied() {
        Some(0) => "Boot Protocol".to_string(),
        Some(1) => "Report Protocol".to_string(),
        Some(_) => format_hex_bytes(bytes),
        None => format_hex_bytes(bytes),
    }
}

fn decode_pnp_id(bytes: &[u8]) -> String {
    if bytes.len() < 7 {
        return format_hex_bytes(bytes);
    }
    let source = match bytes[0] {
        1 => "Bluetooth SIG",
        2 => "USB Implementers Forum",
        _ => "Unknown Vendor ID Source",
    };
    let vendor_id = u16::from_le_bytes([bytes[1], bytes[2]]);
    let product_id = u16::from_le_bytes([bytes[3], bytes[4]]);
    let product_version = u16::from_le_bytes([bytes[5], bytes[6]]);
    format!(
        "{source}, vendor 0x{vendor_id:04X}, product 0x{product_id:04X}, version 0x{product_version:04X}"
    )
}

fn decode_adjust_reason(value: u8) -> String {
    let flags = [
        (value & 0x01 != 0, "manual time update"),
        (value & 0x02 != 0, "external reference"),
        (value & 0x04 != 0, "time zone change"),
        (value & 0x08 != 0, "DST change"),
    ]
    .into_iter()
    .filter_map(|(on, label)| on.then_some(label))
    .collect::<Vec<_>>();
    if flags.is_empty() {
        "no adjustment flags".to_string()
    } else {
        flags.join(", ")
    }
}

fn format_mac_like(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "Unknown".to_string();
    }
    bytes
        .iter()
        .map(|value| format!("{value:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn format_hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|value| format!("{:02X}", value))
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_uuid_line(raw: &str) -> Option<(String, String)> {
    let open = raw.rfind('(')?;
    let close = raw.rfind(')')?;
    if close <= open + 1 {
        return None;
    }
    let name = raw[..open].trim().to_string();
    let uuid = raw[open + 1..close].trim().to_string();
    if uuid.is_empty() {
        None
    } else {
        Some((name, uuid))
    }
}

fn icon_to_device_type(icon: &str) -> String {
    match icon {
        "audio-headset" => "Headset",
        "audio-card" => "Audio Device",
        "audio-input-microphone" => "Microphone",
        "input-keyboard" => "Keyboard",
        "input-mouse" => "Mouse",
        "input-gaming" => "Game Controller",
        "phone" => "Phone",
        "computer" => "Computer",
        "camera-video" => "Camera",
        "multimedia-player" => "Media Player",
        _ => icon,
    }
    .to_string()
}

fn select_controller(controller: &str) {
    let _ = Command::new("bluetoothctl")
        .arg("select")
        .arg(controller)
        .output();
}

fn normalize_mac(value: String) -> String {
    value
        .split(':')
        .filter(|part| !part.trim().is_empty())
        .map(|part| format!("{:0>2}", part.to_ascii_uppercase()))
        .collect::<Vec<_>>()
        .join(":")
}

fn clean_line(raw: &str) -> String {
    let text = strip_ansi(raw);
    text.chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect::<String>()
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut iter = input.chars().peekable();
    while let Some(ch) = iter.next() {
        if ch == '\u{1b}' {
            // CSI sequence
            while let Some(next) = iter.next() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
            continue;
        }
        if ch == '\u{1}' || ch == '\u{2}' {
            continue;
        }
        out.push(ch);
    }
    out
}

fn parse_company_id(raw: &str) -> Option<u16> {
    let text = raw.trim().trim_start_matches("0x");
    u16::from_str_radix(text, 16).ok()
}

fn fallback_uuid_name(uuid: &str) -> Option<String> {
    let short = short_assigned_number(uuid)?;
    let name = match short.as_str() {
        "2A00" => "Device Name",
        "2A01" => "Appearance",
        "2A02" => "Peripheral Privacy Flag",
        "2A03" => "Reconnection Address",
        "2A04" => "Peripheral Preferred Connection Parameters",
        "2A05" => "Service Changed",
        "2A06" => "Alert Level",
        "2A07" => "Tx Power Level",
        "2A08" => "Date Time",
        "2A09" => "Day of Week",
        "2A0A" => "Day Date Time",
        "2A0D" => "DST Offset",
        "2A0E" => "Time Zone",
        "2A0F" => "Local Time Information",
        "2A14" => "Reference Time Information",
        "2A19" => "Battery Level",
        "2A1D" => "Temperature Type",
        "2A21" => "Measurement Interval",
        "2A23" => "System ID",
        "2A24" => "Model Number String",
        "2A25" => "Serial Number String",
        "2A26" => "Firmware Revision String",
        "2A27" => "Hardware Revision String",
        "2A28" => "Software Revision String",
        "2A29" => "Manufacturer Name String",
        "2A2A" => "IEEE 11073-20601 Regulatory Certification Data List",
        "2A2B" => "Current Time",
        "2A31" => "Scan Refresh",
        "2A38" => "Body Sensor Location",
        "2A4A" => "HID Information",
        "2A4B" => "Report Map",
        "2A4D" => "Report",
        "2A4E" => "Protocol Mode",
        "2A50" => "PnP ID",
        "2900" => "Characteristic Extended Properties",
        "2901" => "Characteristic User Description",
        "2902" => "Client Characteristic Configuration",
        "2903" => "Server Characteristic Configuration",
        "2904" => "Characteristic Presentation Format",
        "2905" => "Characteristic Aggregate Format",
        "2906" => "Valid Range",
        "2907" => "External Report Reference",
        "2908" => "Report Reference",
        "2909" => "Number of Digitals",
        "290A" => "Value Trigger Setting",
        "290B" => "Environmental Sensing Configuration",
        "290C" => "Environmental Sensing Measurement",
        "290D" => "Environmental Sensing Trigger Setting",
        "290E" => "Time Trigger Setting",
        _ => return None,
    };
    Some(name.to_string())
}

fn appearance_name(code: u16) -> &'static str {
    match code >> 6 {
        0 => "Unknown",
        1 => "Phone",
        2 => "Computer",
        3 => "Watch",
        4 => "Clock",
        5 => "Display",
        6 => "Remote Control",
        7 => "Eye-glasses",
        8 => "Tag",
        9 => "Keyring",
        10 => "Media Player",
        11 => "Barcode Scanner",
        12 => "Thermometer",
        13 => "Heart Rate Sensor",
        14 => "Blood Pressure",
        15 => "Human Interface Device",
        16 => "Glucose Meter",
        17 => "Running/Walking Sensor",
        18 => "Cycling",
        49 => "Pulse Oximeter",
        50 => "Weight Scale",
        51 => "Personal Mobility Device",
        52 => "Continuous Glucose Monitor",
        53 => "Insulin Pump",
        54 => "Medication Delivery",
        81 => "Outdoor Sports Activity",
        _ => "Assigned Number",
    }
}

fn normalize_assigned_uuid(value: &str) -> String {
    let v = value.trim().to_ascii_lowercase();
    if v.len() == 4 {
        format!("0000{}-0000-1000-8000-00805f9b34fb", v)
    } else if v.len() == 8 && !v.contains('-') {
        format!("{v}-0000-1000-8000-00805f9b34fb")
    } else {
        v
    }
}

fn short_assigned_number(uuid: &str) -> Option<String> {
    let normalized = normalize_assigned_uuid(uuid);
    let prefix = normalized.split('-').next()?.to_ascii_uppercase();
    if prefix.len() == 8 && prefix.starts_with("0000") {
        Some(prefix[4..].to_string())
    } else {
        Some(prefix)
    }
}

fn manifest_asset(name: &str) -> PathBuf {
    let candidates = [
        PathBuf::from("/usr/share/wirelessexplorer/assets").join(name),
        PathBuf::from("/usr/share/WirelessExplorer/assets").join(name),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(name),
    ];
    candidates
        .into_iter()
        .find(|path| path.exists())
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join(name)
        })
}

#[cfg(test)]
mod tests {
    use super::{
        appearance_name, decode_characteristic_value, decode_descriptor_value, fallback_uuid_name,
        normalize_assigned_uuid, parse_tab_separated_mappings, resolve_bluez_targets,
        resolve_ubertooth_targets,
    };

    #[test]
    fn resolves_common_characteristic_uuid_names() {
        assert_eq!(
            fallback_uuid_name(&normalize_assigned_uuid("2A19")).as_deref(),
            Some("Battery Level")
        );
        assert_eq!(
            fallback_uuid_name(&normalize_assigned_uuid("2A29")).as_deref(),
            Some("Manufacturer Name String")
        );
        assert_eq!(
            fallback_uuid_name(&normalize_assigned_uuid("2902")).as_deref(),
            Some("Client Characteristic Configuration")
        );
        assert_eq!(
            fallback_uuid_name(&normalize_assigned_uuid("2A4A")).as_deref(),
            Some("HID Information")
        );
    }

    #[test]
    fn resolves_appearance_categories() {
        assert_eq!(appearance_name(0x0080), "Computer");
        assert_eq!(appearance_name(0x00C0), "Watch");
    }

    #[test]
    fn parses_tab_separated_assigned_number_lines() {
        let rows = parse_tab_separated_mappings(
            "2A19\tBattery Level\n2902\tClient Characteristic Configuration\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "2A19");
        assert_eq!(rows[1].1, "Client Characteristic Configuration");
    }

    #[test]
    fn decodes_common_standard_characteristics() {
        assert_eq!(
            decode_characteristic_value("2A04", &[0x06, 0x00, 0x0C, 0x00, 0x00, 0x00, 0xF4, 0x01]),
            "min 7.50 ms, max 15.00 ms, latency 0, supervision timeout 5000 ms"
        );
        assert_eq!(
            decode_characteristic_value("2A50", &[0x01, 0x4C, 0x00, 0x34, 0x12, 0x02, 0x01]),
            "Bluetooth SIG, vendor 0x004C, product 0x1234, version 0x0102"
        );
        assert_eq!(
            decode_characteristic_value("2A4E", &[0x01]),
            "Report Protocol"
        );
    }

    #[test]
    fn decodes_common_descriptors() {
        assert_eq!(
            decode_descriptor_value("2900", &[0x03, 0x00]),
            "reliable write, writable auxiliaries (0x0003)"
        );
        assert_eq!(
            decode_descriptor_value("2902", &[0x03, 0x00]),
            "notifications, indications enabled (0x0003)"
        );
        assert_eq!(
            decode_descriptor_value("2908", &[0x01, 0x03]),
            "report ID 1, Feature Report"
        );
        assert_eq!(
            decode_descriptor_value("2907", &[0x4B, 0x2A]),
            "00002a4b-0000-1000-8000-00805f9b34fb"
        );
    }

    #[test]
    fn default_bluez_target_is_single_default_controller() {
        assert_eq!(resolve_bluez_targets(None), vec![None]);
        assert_eq!(resolve_bluez_targets(Some("default")), vec![None]);
    }

    #[test]
    fn explicit_bluez_target_is_preserved() {
        assert_eq!(
            resolve_bluez_targets(Some("AA:BB:CC:DD:EE:FF")),
            vec![Some("AA:BB:CC:DD:EE:FF".to_string())]
        );
    }

    #[test]
    fn default_ubertooth_target_is_single_default_device() {
        assert_eq!(resolve_ubertooth_targets(None), vec![None]);
        assert_eq!(resolve_ubertooth_targets(Some("default")), vec![None]);
    }

    #[test]
    fn explicit_ubertooth_target_is_preserved() {
        assert_eq!(
            resolve_ubertooth_targets(Some("usb-1-2")),
            vec![Some("usb-1-2".to_string())]
        );
    }
}

use crate::bluetooth::{self, BluetoothEvent, BluetoothScanConfig};
use crate::capture;
use crate::model::BluetoothDeviceRecord;
use crate::settings::{ChannelSelectionMode, InterfaceSettings, WifiPacketHeaderMode};
use anyhow::{bail, Context, Result};
use crossbeam_channel::unbounded;
use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiTestOptions {
    pub interfaces: Vec<String>,
    pub channels: Vec<u16>,
    pub duration_secs: u64,
    pub ht_mode: String,
    pub packet_header_mode: WifiPacketHeaderMode,
    pub max_networks_per_channel: usize,
}

impl Default for WifiTestOptions {
    fn default() -> Self {
        Self {
            interfaces: Vec::new(),
            channels: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            duration_secs: 6,
            ht_mode: "HT20".to_string(),
            packet_header_mode: WifiPacketHeaderMode::Radiotap,
            max_networks_per_channel: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BeaconObservation {
    bssid: String,
    ssid: Option<String>,
    rssi_dbm: Option<i32>,
    channel: Option<u16>,
    frame_count: u32,
    saw_beacon: bool,
}

pub fn run_wifi_cli(args: &[String]) -> Result<()> {
    let options = parse_wifi_test_args(args)?;
    run_wifi_test(&options)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BluetoothTestOptions {
    pub controller: Option<String>,
    pub source: crate::settings::BluetoothScanSource,
    pub ubertooth_device: Option<String>,
    pub duration_secs: u64,
    pub scan_timeout_secs: u64,
    pub pause_ms: u64,
    pub max_devices: usize,
}

impl Default for BluetoothTestOptions {
    fn default() -> Self {
        Self {
            controller: None,
            source: crate::settings::BluetoothScanSource::Bluez,
            ubertooth_device: None,
            duration_secs: 12,
            scan_timeout_secs: 4,
            pause_ms: 500,
            max_devices: 100,
        }
    }
}

pub fn run_bluetooth_cli(args: &[String]) -> Result<()> {
    let options = parse_bluetooth_test_args(args)?;
    run_bluetooth_test(&options)
}

fn parse_wifi_test_args(args: &[String]) -> Result<WifiTestOptions> {
    let mut options = WifiTestOptions::default();
    let mut idx = 0usize;

    while idx < args.len() {
        match args[idx].as_str() {
            "--interface" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("missing value for --interface"))?;
                options.interfaces.extend(parse_interface_list(value)?);
            }
            "--channels" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("missing value for --channels"))?;
                options.channels = parse_channels(value)?;
            }
            "--duration-secs" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("missing value for --duration-secs"))?;
                options.duration_secs = value
                    .parse::<u64>()
                    .context("invalid value for --duration-secs")?
                    .max(1);
            }
            "--ht-mode" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("missing value for --ht-mode"))?;
                options.ht_mode = value.clone();
            }
            "--packet-headers" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("missing value for --packet-headers"))?;
                options.packet_header_mode = parse_wifi_packet_header_mode(value)?;
            }
            "--max-networks" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("missing value for --max-networks"))?;
                options.max_networks_per_channel = value
                    .parse::<usize>()
                    .context("invalid value for --max-networks")?
                    .max(1);
            }
            "--help" | "-h" => {
                print_wifi_test_usage();
                std::process::exit(0);
            }
            other => {
                bail!("unknown option for --test-wifi: {}", other);
            }
        }
        idx += 1;
    }

    options.interfaces.sort();
    options
        .interfaces
        .dedup_by(|left, right| left.eq_ignore_ascii_case(right));

    if options.interfaces.is_empty() {
        bail!("at least one --interface is required for --test-wifi");
    }
    if options.channels.is_empty() {
        bail!("--channels resolved to an empty set");
    }

    Ok(options)
}

pub fn print_wifi_test_usage() {
    println!("EasyWiFi non-interactive Wi-Fi test mode");
    println!();
    println!("Usage:");
    println!("  easywifi --test-wifi --interface <iface>[,<iface>...] [options]");
    println!("  easywifi --test-wifi --interface <iface1> --interface <iface2> [options]");
    println!();
    println!("Options:");
    println!("  --channels <csv>        Channel list, default: 1-11");
    println!("  --duration-secs <n>     Per-channel capture duration, default: 6");
    println!("  --ht-mode <mode>        HT mode for channel set, default: HT20");
    println!("  --packet-headers <mode> radiotap|ppi (default: radiotap)");
    println!("  --max-networks <n>      Max APs shown per channel, default: 50");
}

pub fn print_bluetooth_test_usage() {
    println!("EasyWiFi non-interactive Bluetooth test mode");
    println!();
    println!("Usage:");
    println!("  easywifi --test-bluetooth [options]");
    println!();
    println!("Options:");
    println!("  --controller <mac|all>  BlueZ controller MAC or all controllers");
    println!("  --source <name>         bluez|ubertooth|both (default: bluez)");
    println!("  --ubertooth-device <id|all> Ubertooth serial/id or all devices");
    println!("  --duration-secs <n>     Total scan duration, default: 12");
    println!("  --scan-timeout-secs <n> Per scan pass timeout, default: 4");
    println!("  --pause-ms <n>          Pause between scan passes, default: 500");
    println!("  --max-devices <n>       Max devices shown, default: 100");
}

fn parse_bluetooth_test_args(args: &[String]) -> Result<BluetoothTestOptions> {
    let mut options = BluetoothTestOptions::default();
    let mut idx = 0usize;

    while idx < args.len() {
        match args[idx].as_str() {
            "--controller" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("missing value for --controller"))?;
                options.controller = Some(value.clone());
            }
            "--source" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("missing value for --source"))?;
                options.source = parse_bluetooth_source(value)?;
            }
            "--ubertooth-device" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("missing value for --ubertooth-device"))?;
                options.ubertooth_device = Some(value.clone());
            }
            "--duration-secs" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("missing value for --duration-secs"))?;
                options.duration_secs = value
                    .parse::<u64>()
                    .context("invalid value for --duration-secs")?
                    .max(1);
            }
            "--scan-timeout-secs" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("missing value for --scan-timeout-secs"))?;
                options.scan_timeout_secs = value
                    .parse::<u64>()
                    .context("invalid value for --scan-timeout-secs")?
                    .max(1);
            }
            "--pause-ms" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("missing value for --pause-ms"))?;
                options.pause_ms = value
                    .parse::<u64>()
                    .context("invalid value for --pause-ms")?;
            }
            "--max-devices" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("missing value for --max-devices"))?;
                options.max_devices = value
                    .parse::<usize>()
                    .context("invalid value for --max-devices")?
                    .max(1);
            }
            "--help" | "-h" => {
                print_bluetooth_test_usage();
                std::process::exit(0);
            }
            other => {
                bail!("unknown option for --test-bluetooth: {}", other);
            }
        }
        idx += 1;
    }

    if matches!(
        options.source,
        crate::settings::BluetoothScanSource::Ubertooth
            | crate::settings::BluetoothScanSource::Both
    ) && !command_exists("ubertooth-btle")
    {
        bail!(
            "ubertooth-btle is required for --source {}",
            bluetooth_source_label(options.source)
        );
    }

    Ok(options)
}

fn parse_bluetooth_source(value: &str) -> Result<crate::settings::BluetoothScanSource> {
    match value.trim().to_ascii_lowercase().as_str() {
        "bluez" => Ok(crate::settings::BluetoothScanSource::Bluez),
        "ubertooth" => Ok(crate::settings::BluetoothScanSource::Ubertooth),
        "both" => Ok(crate::settings::BluetoothScanSource::Both),
        _ => bail!(
            "invalid --source `{}` (expected bluez|ubertooth|both)",
            value
        ),
    }
}

fn bluetooth_source_label(source: crate::settings::BluetoothScanSource) -> &'static str {
    match source {
        crate::settings::BluetoothScanSource::Bluez => "bluez",
        crate::settings::BluetoothScanSource::Ubertooth => "ubertooth",
        crate::settings::BluetoothScanSource::Both => "both",
    }
}

fn run_bluetooth_test(options: &BluetoothTestOptions) -> Result<()> {
    println!("EasyWiFi Bluetooth test mode");
    println!("source: {}", bluetooth_source_label(options.source));
    println!(
        "controller: {}",
        options.controller.as_deref().unwrap_or("(default)")
    );
    println!(
        "ubertooth device: {}",
        options.ubertooth_device.as_deref().unwrap_or("(default)")
    );
    println!("duration: {}s", options.duration_secs);
    println!("scan timeout: {}s", options.scan_timeout_secs);
    println!("pause: {} ms", options.pause_ms);
    println!();

    match bluetooth::list_controllers() {
        Ok(controllers) if !controllers.is_empty() => {
            println!("bluetooth controllers ({}):", controllers.len());
            for controller in controllers {
                let suffix = if controller.is_default {
                    " [default]"
                } else {
                    ""
                };
                let adapter = controller
                    .adapter
                    .as_deref()
                    .map(|value| format!(" [{}]", value))
                    .unwrap_or_default();
                println!(
                    "  {}{}  {}{}",
                    controller.id, adapter, controller.name, suffix
                );
            }
            println!();
        }
        Ok(_) => {
            println!("bluetooth controllers: none reported by bluetoothctl");
            println!();
        }
        Err(err) => {
            println!("bluetooth controller discovery failed: {}", err);
            println!();
        }
    }

    if matches!(
        options.source,
        crate::settings::BluetoothScanSource::Ubertooth
            | crate::settings::BluetoothScanSource::Both
    ) {
        match bluetooth::list_ubertooth_devices() {
            Ok(devices) if !devices.is_empty() => {
                println!("ubertooth devices ({}):", devices.len());
                for device in devices {
                    println!("  {}  {}", device.id, device.name);
                }
                println!();
            }
            Ok(_) => {
                println!("ubertooth devices: none detected");
                println!();
            }
            Err(err) => {
                println!("ubertooth discovery failed: {}", err);
                println!();
            }
        }
    }

    let config = BluetoothScanConfig {
        controller: options.controller.clone(),
        source: options.source,
        ubertooth_device: options.ubertooth_device.clone(),
        scan_timeout_secs: options.scan_timeout_secs,
        pause_ms: options.pause_ms,
    };

    let (sender, receiver) = unbounded();
    let runtime = bluetooth::start_scan(config, sender);
    let started = Instant::now();
    let mut devices = BTreeMap::<String, BluetoothDeviceRecord>::new();
    let mut log_count = 0usize;

    while started.elapsed() < Duration::from_secs(options.duration_secs) {
        let remaining =
            Duration::from_secs(options.duration_secs).saturating_sub(started.elapsed());
        let timeout = std::cmp::min(remaining, Duration::from_millis(350));
        if timeout.is_zero() {
            break;
        }
        match receiver.recv_timeout(timeout) {
            Ok(BluetoothEvent::DeviceSeen(record)) => {
                devices
                    .entry(record.mac.clone())
                    .and_modify(|existing| merge_bluetooth_record(existing, &record))
                    .or_insert(record);
            }
            Ok(BluetoothEvent::Log(message)) => {
                log_count += 1;
                if log_count <= 20 {
                    println!("[scan] {}", message);
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    runtime.stop();

    let mut observed = devices.into_values().collect::<Vec<_>>();
    observed.sort_by(|a, b| {
        b.rssi_dbm
            .unwrap_or(i32::MIN)
            .cmp(&a.rssi_dbm.unwrap_or(i32::MIN))
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.mac.cmp(&b.mac))
    });

    println!();
    println!("observed bluetooth devices: {}", observed.len());
    for device in observed.iter().take(options.max_devices) {
        let name = device
            .advertised_name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or(device.alias.as_deref())
            .unwrap_or("<unknown>");
        let rssi = device
            .rssi_dbm
            .map(|value| format!("{} dBm", value))
            .unwrap_or_else(|| "unknown".to_string());
        let device_type = device
            .device_type
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Unknown");
        let oui = device
            .oui_manufacturer
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Unknown");
        println!(
            "{}  {:<10}  {:<22}  {:<28}  first={} last={}",
            device.mac,
            rssi,
            truncate_text(device_type, 22),
            truncate_text(&format!("{} ({})", name, oui), 28),
            device.first_seen.format("%H:%M:%S"),
            device.last_seen.format("%H:%M:%S")
        );
    }
    if observed.len() > options.max_devices {
        println!("... {} more", observed.len() - options.max_devices);
    }

    Ok(())
}

fn merge_bluetooth_record(existing: &mut BluetoothDeviceRecord, incoming: &BluetoothDeviceRecord) {
    if incoming.first_seen < existing.first_seen {
        existing.first_seen = incoming.first_seen;
    }
    if incoming.last_seen > existing.last_seen {
        existing.last_seen = incoming.last_seen;
    }

    if incoming.rssi_dbm.is_some() {
        existing.rssi_dbm = incoming.rssi_dbm;
    }
    if existing.oui_manufacturer.is_none() && incoming.oui_manufacturer.is_some() {
        existing.oui_manufacturer = incoming.oui_manufacturer.clone();
    }
    if existing.advertised_name.is_none() && incoming.advertised_name.is_some() {
        existing.advertised_name = incoming.advertised_name.clone();
    }
    if existing.alias.is_none() && incoming.alias.is_some() {
        existing.alias = incoming.alias.clone();
    }
    if existing.device_type.is_none() && incoming.device_type.is_some() {
        existing.device_type = incoming.device_type.clone();
    }

    for value in &incoming.mfgr_ids {
        if !existing.mfgr_ids.contains(value) {
            existing.mfgr_ids.push(value.clone());
        }
    }
    for value in &incoming.mfgr_names {
        if !existing.mfgr_names.contains(value) {
            existing.mfgr_names.push(value.clone());
        }
    }
    for value in &incoming.uuids {
        if !existing.uuids.contains(value) {
            existing.uuids.push(value.clone());
        }
    }
    for value in &incoming.uuid_names {
        if !existing.uuid_names.contains(value) {
            existing.uuid_names.push(value.clone());
        }
    }
}

fn parse_channels(value: &str) -> Result<Vec<u16>> {
    let mut channels = value
        .split(',')
        .filter_map(|part| {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                None
            } else {
                trimmed.parse::<u16>().ok()
            }
        })
        .collect::<Vec<_>>();
    channels.sort_unstable();
    channels.dedup();
    if channels.is_empty() {
        bail!("no valid channels in `{}`", value);
    }
    Ok(channels)
}

fn parse_interface_list(value: &str) -> Result<Vec<String>> {
    let mut interfaces = value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    interfaces.sort();
    interfaces.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    if interfaces.is_empty() {
        bail!("no valid interfaces in `{}`", value);
    }
    Ok(interfaces)
}

fn parse_wifi_packet_header_mode(value: &str) -> Result<WifiPacketHeaderMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "radiotap" => Ok(WifiPacketHeaderMode::Radiotap),
        "ppi" => Ok(WifiPacketHeaderMode::Ppi),
        other => bail!(
            "invalid value for --packet-headers `{}` (expected radiotap|ppi)",
            other
        ),
    }
}

fn run_wifi_test(options: &WifiTestOptions) -> Result<()> {
    if !capture::running_as_root() {
        bail!("--test-wifi must be run as root, for example: `sudo -E ./target/debug/easywifi --test-wifi ...`");
    }
    if !command_exists("tshark") {
        bail!("tshark is required for --test-wifi");
    }

    println!("EasyWiFi Wi-Fi test mode");
    println!("privilege mode: {}", capture::privilege_mode_summary());
    println!("interfaces: {}", options.interfaces.join(", "));
    println!(
        "channels: {}",
        options
            .channels
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    println!("duration per channel: {}s", options.duration_secs);
    println!("ht mode: {}", options.ht_mode);
    println!(
        "packet headers: {}",
        match options.packet_header_mode {
            WifiPacketHeaderMode::Radiotap => "Radiotap",
            WifiPacketHeaderMode::Ppi => "PPI",
        }
    );
    println!();

    let mut interface_totals = Vec::new();
    for interface in &options.interfaces {
        println!("=== interface {} ===", interface);
        let total_networks = run_wifi_test_for_interface(interface, options)?;
        interface_totals.push((interface.clone(), total_networks));
        println!();
    }

    if interface_totals.len() > 1 {
        println!("Wi-Fi interface summary:");
        for (interface, total_networks) in &interface_totals {
            println!("  {}: {} observed access points", interface, total_networks);
        }
        let grand_total = interface_totals
            .iter()
            .map(|(_, count)| *count)
            .sum::<usize>();
        println!("  combined: {} observed access points", grand_total);
    } else if let Some((_, total_networks)) = interface_totals.first() {
        println!("total observed access points: {}", total_networks);
    }

    Ok(())
}

fn run_wifi_test_for_interface(interface: &str, options: &WifiTestOptions) -> Result<usize> {
    let prepare_target_channel = *options
        .channels
        .first()
        .ok_or_else(|| anyhow::anyhow!("--channels resolved to an empty set"))?;
    let prepared = capture::prepare_interface_for_capture(
        InterfaceSettings {
            interface_name: interface.to_string(),
            monitor_interface_name: None,
            channel_mode: ChannelSelectionMode::Locked {
                channel: prepare_target_channel,
                ht_mode: options.ht_mode.clone(),
            },
            enabled: true,
        },
        true,
    )?;
    let restore_type = prepared
        .original_type
        .clone()
        .unwrap_or_else(|| "managed".to_string());
    let active_interface = prepared.active_interface_name.clone();

    struct RestoreGuard {
        interface: String,
        restore_type: String,
    }
    impl Drop for RestoreGuard {
        fn drop(&mut self) {
            let _ = capture::set_interface_type(&self.interface, &self.restore_type);
        }
    }
    let _restore = RestoreGuard {
        interface: interface.to_string(),
        restore_type,
    };

    for line in &prepared.status_lines {
        println!("{line}");
    }

    let mut total_networks = 0usize;
    for channel in &options.channels {
        capture::set_channel_with_ht(&active_interface, *channel, &options.ht_mode)
            .with_context(|| format!("failed to set {} channel {}", active_interface, channel))?;
        let observations = capture_beacons_for_channel(
            &active_interface,
            *channel,
            options.duration_secs,
            options.packet_header_mode,
        )?;
        total_networks += observations.len();
        print_channel_summary(*channel, &observations, options.max_networks_per_channel);
    }

    println!("interface total observed access points: {}", total_networks);
    Ok(total_networks)
}

fn print_channel_summary(channel: u16, observations: &[BeaconObservation], limit: usize) {
    println!("=== channel {} ===", channel);
    if observations.is_empty() {
        println!("no 802.11 frames with BSSID observed");
        println!();
        return;
    }

    for observation in observations.iter().take(limit) {
        let ssid = observation
            .ssid
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or("<hidden>");
        let rssi = observation
            .rssi_dbm
            .map(|value| format!("{} dBm", value))
            .unwrap_or_else(|| "unknown".to_string());
        let source = if observation.saw_beacon {
            "beacon"
        } else {
            "non-beacon"
        };
        println!(
            "{}  {:<16}  {:>8}  {}  ({} frames)",
            observation.bssid,
            truncate_text(ssid, 16),
            rssi,
            source,
            observation.frame_count
        );
    }
    if observations.len() > limit {
        println!("... {} more", observations.len() - limit);
    }
    println!();
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect()
}

fn capture_beacons_for_channel(
    interface: &str,
    channel: u16,
    duration_secs: u64,
    packet_header_mode: WifiPacketHeaderMode,
) -> Result<Vec<BeaconObservation>> {
    let mut command = build_tshark_command(interface, duration_secs, packet_header_mode);
    let mut output = command
        .output()
        .with_context(|| format!("failed to launch tshark on {}", interface))?;

    if !output.status.success() {
        if packet_header_mode == WifiPacketHeaderMode::Ppi {
            eprintln!(
                "PPI packet headers failed on {}; retrying with Radiotap",
                interface
            );
            let mut fallback_command =
                build_tshark_command(interface, duration_secs, WifiPacketHeaderMode::Radiotap);
            output = fallback_command
                .output()
                .with_context(|| format!("failed to launch tshark on {}", interface))?;
        }
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            bail!("tshark exited with {}", output.status);
        }
        bail!("{}", stderr);
    }

    let mut by_bssid = BTreeMap::<String, BeaconObservation>::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split('\t');
        let bssid = fields.next().unwrap_or("").trim().to_string();
        if bssid.is_empty() {
            continue;
        }
        if bssid.eq_ignore_ascii_case("ff:ff:ff:ff:ff:ff") {
            continue;
        }
        let ssid_raw = decode_ascii_hex_ssid(fields.next().unwrap_or("").trim());
        let radiotap_rssi = fields
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<i32>().ok());
        let rssi = radiotap_rssi;
        let channel_seen = fields
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<u16>().ok());
        let subtype = fields
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<u16>().ok());

        let entry = by_bssid
            .entry(bssid.clone())
            .or_insert_with(|| BeaconObservation {
                bssid: bssid.clone(),
                ssid: None,
                rssi_dbm: None,
                channel: channel_seen.or(Some(channel)),
                frame_count: 0,
                saw_beacon: false,
            });
        entry.frame_count = entry.frame_count.saturating_add(1);
        if subtype == Some(8) {
            entry.saw_beacon = true;
        }

        if !ssid_raw.is_empty() {
            entry.ssid = Some(ssid_raw);
        }
        if rssi.is_some() {
            entry.rssi_dbm = rssi;
        }
        if channel_seen.is_some() {
            entry.channel = channel_seen;
        }
    }

    let mut observations = by_bssid.into_values().collect::<Vec<_>>();
    observations.sort_by(|a, b| {
        b.rssi_dbm
            .unwrap_or(i32::MIN)
            .cmp(&a.rssi_dbm.unwrap_or(i32::MIN))
            .then_with(|| a.bssid.cmp(&b.bssid))
    });
    Ok(observations)
}

fn decode_ascii_hex_ssid(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() % 2 != 0 {
        return trimmed.to_string();
    }
    if !trimmed.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return trimmed.to_string();
    }

    let mut bytes = Vec::with_capacity(trimmed.len() / 2);
    let mut chars = trimmed.chars();
    while let (Some(a), Some(b)) = (chars.next(), chars.next()) {
        let Some(high) = a.to_digit(16) else {
            return trimmed.to_string();
        };
        let Some(low) = b.to_digit(16) else {
            return trimmed.to_string();
        };
        bytes.push(((high << 4) | low) as u8);
    }
    if bytes.is_empty() {
        return trimmed.to_string();
    }
    if bytes.iter().all(|b| (0x20..=0x7e).contains(b)) {
        if let Ok(text) = String::from_utf8(bytes) {
            return text;
        }
    }
    trimmed.to_string()
}

fn build_tshark_command(
    interface: &str,
    duration_secs: u64,
    packet_header_mode: WifiPacketHeaderMode,
) -> Command {
    let mut command = Command::new("tshark");

    command
        .arg("-i")
        .arg(interface)
        .arg("-y")
        .arg(match packet_header_mode {
            WifiPacketHeaderMode::Radiotap => "IEEE802_11_RADIO",
            WifiPacketHeaderMode::Ppi => "PPI",
        })
        .arg("-a")
        .arg(format!("duration:{}", duration_secs.max(1)))
        .arg("-l")
        .arg("-Y")
        .arg("wlan.bssid && (wlan.fc.type == 0 || wlan.fc.type == 2)")
        .arg("-T")
        .arg("fields")
        .arg("-E")
        .arg("separator=\t")
        .arg("-E")
        .arg("quote=n")
        .arg("-E")
        .arg("occurrence=f")
        .arg("-e")
        .arg("wlan.bssid")
        .arg("-e")
        .arg("wlan.ssid")
        .arg("-e")
        .arg("radiotap.dbm_antsignal")
        .arg("-e")
        .arg("wlan_radio.channel")
        .arg("-e")
        .arg("wlan.fc.subtype")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
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

#[cfg(test)]
mod tests {
    use super::{
        build_tshark_command, decode_ascii_hex_ssid, parse_channels, parse_interface_list,
        parse_wifi_test_args,
    };
    use crate::settings::WifiPacketHeaderMode;

    #[test]
    fn parses_channel_csv() {
        assert_eq!(parse_channels("11,1,6,1").unwrap(), vec![1, 6, 11]);
    }

    #[test]
    fn decodes_ascii_hex_ssid_values() {
        assert_eq!(
            decode_ascii_hex_ssid("486f6d654e6574776f726b"),
            "HomeNetwork".to_string()
        );
        assert_eq!(decode_ascii_hex_ssid("000102ff"), "000102ff".to_string());
    }

    #[test]
    fn parses_wifi_test_args() {
        let args = vec![
            "--interface".to_string(),
            "wlx1cbfcef8e928".to_string(),
            "--channels".to_string(),
            "1,6,11".to_string(),
            "--duration-secs".to_string(),
            "8".to_string(),
            "--ht-mode".to_string(),
            "HT20".to_string(),
            "--max-networks".to_string(),
            "25".to_string(),
        ];
        let options = parse_wifi_test_args(&args).unwrap();
        assert_eq!(options.interfaces, vec!["wlx1cbfcef8e928".to_string()]);
        assert_eq!(options.channels, vec![1, 6, 11]);
        assert_eq!(options.duration_secs, 8);
        assert_eq!(options.ht_mode, "HT20");
        assert_eq!(options.packet_header_mode, WifiPacketHeaderMode::Radiotap);
        assert_eq!(options.max_networks_per_channel, 25);
    }

    #[test]
    fn parses_wifi_packet_headers_mode() {
        let args = vec![
            "--interface".to_string(),
            "wlx1cbfcef8e928".to_string(),
            "--packet-headers".to_string(),
            "ppi".to_string(),
        ];
        let options = parse_wifi_test_args(&args).unwrap();
        assert_eq!(options.packet_header_mode, WifiPacketHeaderMode::Ppi);
    }

    #[test]
    fn rejects_invalid_wifi_packet_headers_mode() {
        let args = vec![
            "--interface".to_string(),
            "wlx1cbfcef8e928".to_string(),
            "--packet-headers".to_string(),
            "invalid".to_string(),
        ];
        let err = parse_wifi_test_args(&args).unwrap_err().to_string();
        assert!(err.contains("expected radiotap|ppi"));
    }

    #[test]
    fn build_tshark_command_uses_radiotap_link_type() {
        let cmd = build_tshark_command("wlan0", 3, WifiPacketHeaderMode::Radiotap);
        let args = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let y_index = args.iter().position(|arg| arg == "-y").unwrap();
        assert_eq!(args.get(y_index + 1), Some(&"IEEE802_11_RADIO".to_string()));
    }

    #[test]
    fn build_tshark_command_uses_ppi_link_type() {
        let cmd = build_tshark_command("wlan0", 3, WifiPacketHeaderMode::Ppi);
        let args = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let y_index = args.iter().position(|arg| arg == "-y").unwrap();
        assert_eq!(args.get(y_index + 1), Some(&"PPI".to_string()));
    }

    #[test]
    fn parses_multiple_wifi_interfaces() {
        let args = vec![
            "--interface".to_string(),
            "wlx1cbfcef8e928,wlp0s20f3".to_string(),
            "--interface".to_string(),
            "wlp0s20f3".to_string(),
        ];
        let options = parse_wifi_test_args(&args).unwrap();
        assert_eq!(
            options.interfaces,
            vec!["wlp0s20f3".to_string(), "wlx1cbfcef8e928".to_string()]
        );
    }

    #[test]
    fn parses_interface_csv_and_deduplicates() {
        assert_eq!(
            parse_interface_list("wlx1cbfcef8e928,wlp0s20f3,wlx1cbfcef8e928").unwrap(),
            vec!["wlp0s20f3".to_string(), "wlx1cbfcef8e928".to_string()]
        );
    }
}

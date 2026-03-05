use crate::capture;
use crate::settings::{ChannelSelectionMode, InterfaceSettings};
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiTestOptions {
    pub interface: String,
    pub channels: Vec<u16>,
    pub duration_secs: u64,
    pub ht_mode: String,
    pub max_networks_per_channel: usize,
}

impl Default for WifiTestOptions {
    fn default() -> Self {
        Self {
            interface: String::new(),
            channels: vec![1, 6, 11],
            duration_secs: 6,
            ht_mode: "HT20".to_string(),
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
}

pub fn run_wifi_cli(args: &[String]) -> Result<()> {
    let options = parse_wifi_test_args(args)?;
    run_wifi_test(&options)
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
                options.interface = value.clone();
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

    if options.interface.trim().is_empty() {
        bail!("--interface is required for --test-wifi");
    }
    if options.channels.is_empty() {
        bail!("--channels resolved to an empty set");
    }

    Ok(options)
}

pub fn print_wifi_test_usage() {
    println!("SimpleSTG non-interactive Wi-Fi test mode");
    println!();
    println!("Usage:");
    println!("  simplestg --test-wifi --interface <iface> [options]");
    println!();
    println!("Options:");
    println!("  --channels <csv>        Channel list, default: 1,6,11");
    println!("  --duration-secs <n>     Per-channel capture duration, default: 6");
    println!("  --ht-mode <mode>        HT mode for channel set, default: HT20");
    println!("  --max-networks <n>      Max APs shown per channel, default: 50");
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

fn run_wifi_test(options: &WifiTestOptions) -> Result<()> {
    if !capture::running_as_root() {
        bail!("--test-wifi must be run as root, for example: `sudo -E ./target/debug/simplestg --test-wifi ...`");
    }
    if !command_exists("tshark") {
        bail!("tshark is required for --test-wifi");
    }

    println!("SimpleSTG Wi-Fi test mode");
    println!("privilege mode: {}", capture::privilege_mode_summary());
    println!("interface: {}", options.interface);
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
    println!();

    let prepare_target_channel = *options
        .channels
        .first()
        .ok_or_else(|| anyhow::anyhow!("--channels resolved to an empty set"))?;
    let prepared = capture::prepare_interface_for_capture(
        InterfaceSettings {
            interface_name: options.interface.clone(),
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
        interface: options.interface.clone(),
        restore_type,
    };

    for line in &prepared.status_lines {
        println!("{line}");
    }

    let mut total_networks = 0usize;
    for channel in &options.channels {
        capture::set_channel_with_ht(&active_interface, *channel, &options.ht_mode)
            .with_context(|| format!("failed to set {} channel {}", active_interface, channel))?;
        let observations =
            capture_beacons_for_channel(&active_interface, *channel, options.duration_secs)?;
        total_networks += observations.len();
        print_channel_summary(*channel, &observations, options.max_networks_per_channel);
    }

    println!();
    println!("total observed access points: {}", total_networks);
    Ok(())
}

fn print_channel_summary(channel: u16, observations: &[BeaconObservation], limit: usize) {
    println!("=== channel {} ===", channel);
    if observations.is_empty() {
        println!("no beacon frames observed");
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
        println!(
            "{}  {:<16}  {}",
            observation.bssid,
            truncate_text(ssid, 16),
            rssi
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
) -> Result<Vec<BeaconObservation>> {
    let mut command = build_tshark_command(interface, duration_secs);
    let output = command
        .output()
        .with_context(|| format!("failed to launch tshark on {}", interface))?;

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
        let ssid_raw = fields.next().unwrap_or("").trim().to_string();
        let rssi = fields
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<i32>().ok());
        let channel_seen = fields
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
            });

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

fn build_tshark_command(interface: &str, duration_secs: u64) -> Command {
    let mut command = Command::new("tshark");

    command
        .arg("-i")
        .arg(interface)
        .arg("-a")
        .arg(format!("duration:{}", duration_secs.max(1)))
        .arg("-l")
        .arg("-Y")
        .arg("wlan.fc.type_subtype == 8")
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
    use super::{parse_channels, parse_wifi_test_args};

    #[test]
    fn parses_channel_csv() {
        assert_eq!(parse_channels("11,1,6,1").unwrap(), vec![1, 6, 11]);
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
        assert_eq!(options.interface, "wlx1cbfcef8e928");
        assert_eq!(options.channels, vec![1, 6, 11]);
        assert_eq!(options.duration_secs, 8);
        assert_eq!(options.ht_mode, "HT20");
        assert_eq!(options.max_networks_per_channel, 25);
    }
}

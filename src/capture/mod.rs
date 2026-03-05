use crate::model::{
    AccessPointRecord, ChannelUsagePoint, ClientEndpointRecord, ClientRecord, GeoObservation,
    HandshakeRecord, PacketTypeBreakdown, SpectrumBand,
};
use crate::netintel::GeoIpLookup;
use crate::privilege::{HelperRequest, HelperResponse};
use crate::settings::{ChannelSelectionMode, InterfaceSettings};
use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use crossbeam_channel::Sender;
use once_cell::sync::Lazy;
use rand::Rng;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub interfaces: Vec<InterfaceSettings>,
    pub session_pcap_path: Option<PathBuf>,
    pub geoip_city_db_path: Option<PathBuf>,
    pub gps_enabled: bool,
    pub passive_only: bool,
}

#[derive(Debug, Clone)]
pub enum CaptureEvent {
    AccessPointSeen(AccessPointRecord),
    ClientSeen(ClientRecord),
    Observation {
        device_type: String,
        device_id: String,
        observation: GeoObservation,
    },
    HandshakeSeen(HandshakeRecord),
    ChannelUsage(ChannelUsagePoint),
    Log(String),
}

pub struct CaptureRuntime {
    stop_flag: Arc<AtomicBool>,
    handles: Vec<thread::JoinHandle<()>>,
}

impl CaptureRuntime {
    pub fn stop(self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        for handle in self.handles {
            let _ = handle.join();
        }
    }
}

#[derive(Debug, Clone)]
pub struct InterfaceInfo {
    pub name: String,
    pub if_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SupportedChannel {
    pub channel: u16,
    pub frequency_mhz: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct GeigerUpdate {
    pub rssi_dbm: i32,
    pub tone_hz: u32,
}

#[derive(Debug, Clone)]
pub struct PreparedInterface {
    pub interface: InterfaceSettings,
    pub original_type: Option<String>,
    pub active_interface_name: String,
    pub status_lines: Vec<String>,
}

struct PrivilegedHelperClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    launch_mode: String,
    launch_attempts: Vec<String>,
}

static PRIVILEGED_HELPER: Lazy<Mutex<Option<PrivilegedHelperClient>>> =
    Lazy::new(|| Mutex::new(None));
static TSHARK_FIELDS: Lazy<HashSet<String>> = Lazy::new(load_tshark_fields);

pub fn list_interfaces() -> Result<Vec<InterfaceInfo>> {
    let mut interfaces = Vec::new();
    let mut seen = HashSet::new();

    if let Ok(output) = Command::new("iw").arg("dev").output() {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut current_name: Option<String> = None;

            for line in text.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("Interface ") {
                    current_name = Some(rest.trim().to_string());
                } else if let Some(rest) = trimmed.strip_prefix("type ") {
                    if let Some(name) = current_name.take() {
                        seen.insert(name.clone());
                        interfaces.push(InterfaceInfo {
                            name,
                            if_type: rest.trim().to_string(),
                        });
                    }
                }
            }
        }
    }

    for name in list_sysfs_wireless_interfaces() {
        if seen.contains(&name) {
            continue;
        }
        let if_type = interface_type_via_iw(&name).unwrap_or_else(|| "unknown".to_string());
        seen.insert(name.clone());
        interfaces.push(InterfaceInfo { name, if_type });
    }

    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(interfaces)
}

fn list_sysfs_wireless_interfaces() -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return out;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let is_wireless = path.join("wireless").exists() || path.join("phy80211").exists();
        if !is_wireless {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            out.push(name.to_string());
        }
    }

    out.sort();
    out.dedup();
    out
}

fn interface_type_via_iw(interface: &str) -> Option<String> {
    let output = Command::new("iw")
        .args(["dev", interface, "info"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("type ")
                .map(|v| v.trim().to_string())
        })
}

pub fn current_interface_type(interface: &str) -> Option<String> {
    interface_type_via_iw(interface)
}

pub fn shutdown_privileged_helper() {
    let Ok(mut guard) = PRIVILEGED_HELPER.lock() else {
        return;
    };
    if let Some(mut client) = guard.take() {
        let _ = send_helper_request_inner(&mut client, &HelperRequest::Shutdown);
        let _ = client.child.kill();
        let _ = client.child.wait();
    }
}

#[derive(Debug)]
struct PrivilegedPassthroughProcess {
    child: Child,
    launch_mode: String,
}

fn helper_binary_path() -> Result<PathBuf> {
    let current = std::env::current_exe().context("failed to resolve current executable path")?;
    let helper = current.with_file_name("simplestg-helper");
    if helper.exists() {
        Ok(helper)
    } else {
        anyhow::bail!("helper binary not found at {}", helper.display())
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

fn configure_parent_death_signal(command: &mut Command) {
    let parent_pid = std::process::id();
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() as u32 != parent_pid {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "parent exited before child exec",
                ));
            }
            Ok(())
        });
    }
}

fn is_effective_root() -> bool {
    Command::new("id")
        .arg("-u")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim() == "0")
        .unwrap_or(false)
}

pub fn running_as_root() -> bool {
    is_effective_root()
}

pub fn privilege_mode_summary() -> &'static str {
    if is_effective_root() {
        "direct root session"
    } else {
        "helper elevation per scan operation"
    }
}

fn load_tshark_fields() -> HashSet<String> {
    let Ok(output) = Command::new("tshark")
        .args(["-G", "fields"])
        .stderr(Stdio::null())
        .output()
    else {
        return HashSet::new();
    };
    if !output.status.success() {
        return HashSet::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let _kind = parts.next()?;
            let _display = parts.next()?;
            parts.next().map(str::to_string)
        })
        .collect()
}

fn spawn_privileged_helper() -> Result<PrivilegedHelperClient> {
    let (helper, candidates, mut attempt_errors) =
        helper_invocation_candidates(&["daemon".to_string()])?;

    for (label, candidate) in candidates {
        let command_display = candidate.join(" ");
        let program = &candidate[0];
        let args = &candidate[1..];
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("SIMPLESTG_PARENT_PID", std::process::id().to_string());
        configure_parent_death_signal(&mut command);
        match command.spawn() {
            Ok(mut child) => {
                let Some(stdin) = child.stdin.take() else {
                    let _ = child.kill();
                    attempt_errors.push(format!(
                        "{}: helper stdin unavailable after launch via `{}`",
                        label, command_display
                    ));
                    continue;
                };
                let Some(stdout) = child.stdout.take() else {
                    let _ = child.kill();
                    attempt_errors.push(format!(
                        "{}: helper stdout unavailable after launch via `{}`",
                        label, command_display
                    ));
                    continue;
                };
                let mut client = PrivilegedHelperClient {
                    child,
                    stdin,
                    stdout: BufReader::new(stdout),
                    launch_mode: label.clone(),
                    launch_attempts: attempt_errors.clone(),
                };
                match send_helper_request_inner(&mut client, &HelperRequest::Ping) {
                    Ok(_) => return Ok(client),
                    Err(err) => {
                        let exit_state = match client.child.try_wait() {
                            Ok(Some(status)) => format!("helper exited with status {}", status),
                            Ok(None) => "helper did not respond to ping".to_string(),
                            Err(wait_err) => {
                                format!("could not query helper exit status: {}", wait_err)
                            }
                        };
                        let _ = client.child.kill();
                        let _ = client.child.wait();
                        attempt_errors.push(format!(
                            "{}: launch via `{}` failed: {} ({})",
                            label, command_display, err, exit_state
                        ));
                    }
                }
            }
            Err(err) => {
                attempt_errors.push(format!(
                    "{}: failed to spawn `{}`: {}",
                    label, command_display, err
                ));
            }
        }
    }

    let mut message = vec![
        "privileged helper startup failed".to_string(),
        format!("helper binary: {}", helper.display()),
        "attempt results:".to_string(),
    ];
    message.extend(
        attempt_errors
            .into_iter()
            .map(|entry| format!("- {}", entry)),
    );
    if is_effective_root() {
        message.push(
            "SimpleSTG is already running as root. Direct helper startup still failed.".to_string(),
        );
        message.push(format!(
            "verify that the helper binary exists and is executable: {}",
            helper.display()
        ));
    } else {
        message.push(
            "required: keep the GUI unprivileged and make one privilege path work:".to_string(),
        );
        message.push("1. `pkexec` with a working polkit agent".to_string());
        message.push("2. passwordless `sudo -n` for `simplestg-helper`".to_string());
        message.push(format!(
            "3. capabilities on the helper: `sudo setcap cap_net_admin,cap_net_raw=eip {}`",
            helper.display()
        ));
    }

    Err(anyhow::anyhow!(message.join("\n")))
}

fn helper_invocation_candidates(
    helper_args: &[String],
) -> Result<(PathBuf, Vec<(String, Vec<String>)>, Vec<String>)> {
    let helper = helper_binary_path()?;
    let helper_str = helper.to_string_lossy().to_string();
    let mut candidates: Vec<(String, Vec<String>)> = Vec::new();
    let mut attempt_errors = Vec::new();

    if is_effective_root() {
        let mut args = vec![helper_str.clone()];
        args.extend(helper_args.iter().cloned());
        candidates.push(("direct helper (already root)".to_string(), args));
    } else {
        if command_exists("pkexec") {
            let mut args = vec!["pkexec".to_string(), helper_str.clone()];
            args.extend(helper_args.iter().cloned());
            candidates.push(("pkexec".to_string(), args));
        } else {
            attempt_errors.push("pkexec: not found in PATH".to_string());
        }
        if command_exists("sudo") {
            let mut args = vec!["sudo".to_string(), "-n".to_string(), helper_str.clone()];
            args.extend(helper_args.iter().cloned());
            candidates.push(("sudo -n".to_string(), args));
        } else {
            attempt_errors.push("sudo -n: sudo not found in PATH".to_string());
        }
        let mut args = vec![helper_str.clone()];
        args.extend(helper_args.iter().cloned());
        candidates.push(("direct helper".to_string(), args));
    }

    Ok((helper, candidates, attempt_errors))
}

fn spawn_privileged_tshark(args: &[String]) -> Result<PrivilegedPassthroughProcess> {
    if is_effective_root() {
        return spawn_direct_passthrough_command(
            Command::new("tshark").args(args),
            "direct tshark (already root)",
        );
    }
    let helper_args = std::iter::once("tshark".to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>();
    spawn_privileged_helper_command(
        &helper_args,
        "privileged tshark startup failed",
        "required capture privilege paths:",
    )
}

fn spawn_privileged_helper_command(
    helper_args: &[String],
    failure_title: &str,
    requirements_title: &str,
) -> Result<PrivilegedPassthroughProcess> {
    let (helper, candidates, mut attempt_errors) = helper_invocation_candidates(helper_args)?;

    for (label, candidate) in candidates {
        let command_display = candidate.join(" ");
        let program = &candidate[0];
        let args = &candidate[1..];
        let mut command = Command::new(program);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("SIMPLESTG_PARENT_PID", std::process::id().to_string());
        configure_parent_death_signal(&mut command);
        match command.spawn() {
            Ok(child) => {
                return Ok(PrivilegedPassthroughProcess {
                    child,
                    launch_mode: label,
                });
            }
            Err(err) => {
                attempt_errors.push(format!(
                    "{}: failed to spawn `{}`: {}",
                    label, command_display, err
                ));
            }
        }
    }

    let mut message = vec![
        failure_title.to_string(),
        format!("helper binary: {}", helper.display()),
        "attempt results:".to_string(),
    ];
    message.extend(
        attempt_errors
            .into_iter()
            .map(|entry| format!("- {}", entry)),
    );
    if is_effective_root() {
        message.push(
            "SimpleSTG is already running as root. Direct helper launch still failed.".to_string(),
        );
        message.push(format!(
            "verify that the helper binary exists and is executable: {}",
            helper.display()
        ));
    } else {
        message.push(requirements_title.to_string());
        message.push("1. `pkexec` with a working polkit agent".to_string());
        message.push("2. passwordless `sudo -n` for `simplestg-helper`".to_string());
        message.push(format!(
            "3. capabilities on the helper: `sudo setcap cap_net_admin,cap_net_raw=eip {}`",
            helper.display()
        ));
    }

    Err(anyhow::anyhow!(message.join("\n")))
}

fn spawn_privileged_channel_hopper(
    interface: &str,
    dwell_ms: u64,
    ht_mode: &str,
    channels: &[u16],
) -> Result<PrivilegedPassthroughProcess> {
    let helper_args = std::iter::once("hop".to_string())
        .chain(std::iter::once(interface.to_string()))
        .chain(std::iter::once(dwell_ms.to_string()))
        .chain(std::iter::once(ht_mode.to_string()))
        .chain(channels.iter().map(|ch| ch.to_string()))
        .collect::<Vec<_>>();
    spawn_privileged_helper_command(
        &helper_args,
        "privileged channel hopper startup failed",
        "required channel control privilege paths:",
    )
}

fn with_privileged_helper<T>(
    func: impl FnOnce(&mut PrivilegedHelperClient) -> Result<T>,
) -> Result<T> {
    let mut guard = PRIVILEGED_HELPER
        .lock()
        .map_err(|_| anyhow::anyhow!("privileged helper mutex poisoned"))?;
    if guard.is_none() {
        *guard = Some(spawn_privileged_helper()?);
    }

    let result = {
        let client = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("privileged helper unavailable"))?;
        func(client)
    };
    if result.is_err() {
        if let Some(mut client) = guard.take() {
            let _ = client.child.kill();
            let _ = client.child.wait();
        }
    }
    result
}

fn send_helper_request(request: &HelperRequest) -> Result<HelperResponse> {
    if is_effective_root() {
        return match request {
            HelperRequest::Ping => Ok(HelperResponse::ok(Some("pong".to_string()))),
            HelperRequest::CurrentInterfaceType { interface } => {
                Ok(HelperResponse::ok(current_interface_type(interface)))
            }
            HelperRequest::SetMonitorMode {
                interface,
                monitor_name,
            } => set_interface_monitor_mode_direct(interface, monitor_name.as_deref())
                .map(|active| HelperResponse::ok(Some(active))),
            HelperRequest::SetChannel {
                interface,
                channel,
                ht_mode,
            } => set_channel_with_ht_direct(interface, *channel, ht_mode)
                .map(|_| HelperResponse::ok(Some("ok".to_string()))),
            HelperRequest::SetInterfaceType { interface, if_type } => {
                set_interface_type_direct(interface, if_type)
                    .map(|_| HelperResponse::ok(Some("ok".to_string())))
            }
            HelperRequest::Shutdown => Ok(HelperResponse::ok(Some("bye".to_string()))),
        };
    }
    with_privileged_helper(|client| send_helper_request_inner(client, request))
}

fn send_helper_request_inner(
    client: &mut PrivilegedHelperClient,
    request: &HelperRequest,
) -> Result<HelperResponse> {
    let payload = serde_json::to_string(request)?;
    client
        .stdin
        .write_all(payload.as_bytes())
        .with_context(|| format!("failed to write helper request via {}", client.launch_mode))?;
    client.stdin.write_all(b"\n").with_context(|| {
        format!(
            "failed to terminate helper request via {}",
            client.launch_mode
        )
    })?;
    client
        .stdin
        .flush()
        .with_context(|| format!("failed to flush helper request via {}", client.launch_mode))?;

    let mut line = String::new();
    let read = client
        .stdout
        .read_line(&mut line)
        .with_context(|| format!("failed to read helper response via {}", client.launch_mode))?;
    if read == 0 {
        anyhow::bail!(
            "privileged helper exited unexpectedly via {}",
            client.launch_mode
        );
    }

    let response: HelperResponse =
        serde_json::from_str(line.trim()).context("invalid helper response")?;
    if response.ok {
        Ok(response)
    } else {
        let prior_attempts = format_prior_launch_attempts(&client.launch_attempts);
        anyhow::bail!(
            "{} (helper path: {}){}",
            response
                .error
                .unwrap_or_else(|| "privileged helper command failed".to_string()),
            client.launch_mode,
            prior_attempts
        );
    }
}

fn format_prior_launch_attempts(attempts: &[String]) -> String {
    if attempts.is_empty() {
        String::new()
    } else {
        format!("\nprior launch attempts:\n- {}", attempts.join("\n- "))
    }
}

pub fn helper_binary_hint() -> String {
    helper_binary_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "simplestg-helper".to_string())
}

pub fn list_supported_channels(interface: &str) -> Result<Vec<u16>> {
    let mut channels = list_supported_channel_details(interface)?
        .into_iter()
        .map(|c| c.channel)
        .collect::<Vec<_>>();
    channels.sort_unstable();
    channels.dedup();
    Ok(channels)
}

pub fn list_supported_channel_details(interface: &str) -> Result<Vec<SupportedChannel>> {
    let Some(text) = phy_info_text_for_interface(interface)? else {
        return Ok(Vec::new());
    };
    let re = Regex::new(r"(\d+)\s+MHz\s+\[(\d+)\]").unwrap();
    let mut channels = Vec::new();
    let mut seen = HashSet::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.contains("(disabled)") {
            continue;
        }

        let Some(cap) = re.captures(trimmed) else {
            continue;
        };

        let freq = cap.get(1).and_then(|m| m.as_str().parse::<u32>().ok());
        let channel = cap.get(2).and_then(|m| m.as_str().parse::<u16>().ok());
        let (Some(freq), Some(channel)) = (freq, channel) else {
            continue;
        };

        if seen.insert((freq, channel)) {
            channels.push(SupportedChannel {
                channel,
                frequency_mhz: Some(freq),
            });
        }
    }

    channels.sort_by_key(|c| (c.frequency_mhz.unwrap_or(0), c.channel));
    Ok(channels)
}

pub fn interface_supports_monitor_mode(interface: &str) -> Result<bool> {
    let Some(text) = phy_info_text_for_interface(interface)? else {
        return Ok(false);
    };

    let mut in_modes = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Supported interface modes:") {
            in_modes = true;
            continue;
        }
        if !in_modes {
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('*') {
            if trimmed.contains("monitor") {
                return Ok(true);
            }
            continue;
        }
        if !line.starts_with('\t') && !line.starts_with(' ') {
            break;
        }
    }

    Ok(text.contains("* monitor"))
}

pub fn list_supported_ht_modes(interface: &str) -> Result<Vec<String>> {
    let Some(text) = phy_info_text_for_interface(interface)? else {
        return Ok(vec!["HT20".to_string()]);
    };

    let mut modes = Vec::new();
    let lower = text.to_ascii_lowercase();

    // Conservative lock-channel modes that work with `iw dev set channel`.
    modes.push("HT20".to_string());

    if lower.contains("ht20/ht40") || lower.contains("ht capabilities") {
        modes.push("HT40+".to_string());
        modes.push("HT40-".to_string());
    }
    if lower.contains("noht") {
        modes.push("NOHT".to_string());
    }

    // Expose read-only hints for modern widths in the UI capability table.
    if lower.contains("vht capabilities") {
        modes.push("VHT (device capability)".to_string());
    }
    if lower.contains("he iftypes") || lower.contains("he capabilities") {
        modes.push("HE (device capability)".to_string());
    }

    modes.sort();
    modes.dedup();
    Ok(modes)
}

pub fn prepare_interface_for_capture(
    mut interface: InterfaceSettings,
    apply_initial_channel: bool,
) -> Result<PreparedInterface> {
    let original_type = current_interface_type(&interface.interface_name);
    let mut status_lines = Vec::new();

    let active_interface_name = if matches!(original_type.as_deref(), Some("monitor")) {
        interface.monitor_interface_name = None;
        status_lines.push(format!(
            "{} already in monitor mode",
            interface.interface_name
        ));
        interface.interface_name.clone()
    } else {
        let active_iface = set_interface_monitor_mode(
            &interface.interface_name,
            interface.monitor_interface_name.as_deref(),
        )
        .with_context(|| {
            format!(
                "failed to enable monitor mode on {}",
                interface.interface_name
            )
        })?;
        if active_iface != interface.interface_name {
            interface.monitor_interface_name = Some(active_iface.clone());
        } else {
            interface.monitor_interface_name = None;
        }
        status_lines.push(format!(
            "{} monitor mode enabled on {}",
            interface.interface_name, active_iface
        ));
        active_iface
    };

    if apply_initial_channel {
        if let Some((initial_channel, ht_mode)) = initial_channel_request(&interface.channel_mode) {
            set_channel_with_ht(&active_interface_name, initial_channel, &ht_mode).with_context(
                || {
                    format!(
                        "failed to set initial channel {} ({}) on {}",
                        initial_channel, ht_mode, active_interface_name
                    )
                },
            )?;
            status_lines.push(format!(
                "{} initial channel set to {} ({})",
                active_interface_name, initial_channel, ht_mode
            ));
        }
    }

    Ok(PreparedInterface {
        interface,
        original_type,
        active_interface_name,
        status_lines,
    })
}

fn initial_channel_request(mode: &ChannelSelectionMode) -> Option<(u16, String)> {
    match mode {
        ChannelSelectionMode::Locked { channel, ht_mode } => Some((*channel, ht_mode.clone())),
        ChannelSelectionMode::HopAll { channels, .. }
        | ChannelSelectionMode::HopBand { channels, .. } => channels
            .first()
            .copied()
            .map(|channel| (channel, "HT20".to_string())),
    }
}

pub fn set_interface_monitor_mode(interface: &str, monitor_name: Option<&str>) -> Result<String> {
    if is_effective_root() {
        return set_interface_monitor_mode_direct(interface, monitor_name);
    }
    let response = send_helper_request(&HelperRequest::SetMonitorMode {
        interface: interface.to_string(),
        monitor_name: monitor_name.map(str::to_string),
    })?;
    response
        .result
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("helper returned no active monitor interface"))
}

pub fn set_interface_monitor_mode_direct(
    interface: &str,
    monitor_name: Option<&str>,
) -> Result<String> {
    run_command_checked(
        Command::new("ip").args(["link", "set", interface, "down"]),
        format!("failed to set {} down", interface),
    )?;

    let active_iface = if let Some(mon) = monitor_name {
        run_command_checked(
            Command::new("iw").args(["dev", interface, "interface", "add", mon, "type", "monitor"]),
            format!(
                "failed to create monitor interface {} from {}",
                mon, interface
            ),
        )?;
        mon.to_string()
    } else {
        run_command_checked(
            Command::new("iw").args(["dev", interface, "set", "type", "monitor"]),
            format!("failed to set {} monitor mode", interface),
        )?;
        interface.to_string()
    };

    run_command_checked(
        Command::new("ip").args(["link", "set", &active_iface, "up"]),
        format!("failed to set {} up", active_iface),
    )?;

    Ok(active_iface)
}

pub fn set_channel(interface: &str, channel: u16) -> Result<()> {
    set_channel_with_ht(interface, channel, "HT20")
}

pub fn set_interface_type(interface: &str, if_type: &str) -> Result<()> {
    if is_effective_root() {
        return set_interface_type_direct(interface, if_type);
    }
    let _ = send_helper_request(&HelperRequest::SetInterfaceType {
        interface: interface.to_string(),
        if_type: if_type.to_string(),
    })?;
    Ok(())
}

pub fn set_interface_type_direct(interface: &str, if_type: &str) -> Result<()> {
    run_command_checked(
        Command::new("ip").args(["link", "set", interface, "down"]),
        format!("failed to set {} down", interface),
    )?;
    run_command_checked(
        Command::new("iw").args(["dev", interface, "set", "type", if_type]),
        format!("failed to set {} interface type {}", interface, if_type),
    )?;
    run_command_checked(
        Command::new("ip").args(["link", "set", interface, "up"]),
        format!("failed to set {} up", interface),
    )?;
    Ok(())
}

pub fn set_channel_with_ht(interface: &str, channel: u16, ht_mode: &str) -> Result<()> {
    if is_effective_root() {
        return set_channel_with_ht_direct(interface, channel, ht_mode);
    }
    let _ = send_helper_request(&HelperRequest::SetChannel {
        interface: interface.to_string(),
        channel,
        ht_mode: ht_mode.to_string(),
    })?;
    Ok(())
}

pub fn set_channel_with_ht_direct(interface: &str, channel: u16, ht_mode: &str) -> Result<()> {
    run_command_checked(
        Command::new("iw").args([
            "dev",
            interface,
            "set",
            "channel",
            &channel.to_string(),
            ht_mode,
        ]),
        format!(
            "failed to set channel {} ({}) on {}",
            channel, ht_mode, interface
        ),
    )?;

    Ok(())
}

fn run_command_checked(command: &mut Command, context: String) -> Result<()> {
    let output = command.output().with_context(|| context.clone())?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("exit status {}", output.status)
    };

    anyhow::bail!("{}: {}", context, detail)
}

fn spawn_direct_passthrough_command(
    command: &mut Command,
    launch_mode: &str,
) -> Result<PrivilegedPassthroughProcess> {
    configure_parent_death_signal(command);
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn {}", launch_mode))?;
    Ok(PrivilegedPassthroughProcess {
        child,
        launch_mode: launch_mode.to_string(),
    })
}

pub fn start_capture(config: CaptureConfig, sender: Sender<CaptureEvent>) -> CaptureRuntime {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::new();

    for interface in config.interfaces.iter().filter(|i| i.enabled) {
        let active_interface_name = interface
            .monitor_interface_name
            .clone()
            .unwrap_or_else(|| interface.interface_name.clone());
        let stop = Arc::clone(&stop_flag);
        let tx = sender.clone();
        let mut iface_settings = interface.clone();
        iface_settings.interface_name = active_interface_name.clone();
        let pcap_path = config.session_pcap_path.clone();
        let geoip_city_db_path = config.geoip_city_db_path.clone();

        let handle = thread::spawn(move || {
            run_interface_capture(&iface_settings, pcap_path, geoip_city_db_path, tx, stop);
        });

        handles.push(handle);

        let hop_stop = Arc::clone(&stop_flag);
        let hop_iface = active_interface_name;
        let hop_mode = interface.channel_mode.clone();
        let hop_tx = sender.clone();

        let hop_handle = thread::spawn(move || {
            run_channel_control_loop(&hop_iface, hop_mode, hop_tx, hop_stop);
        });
        handles.push(hop_handle);
    }

    CaptureRuntime { stop_flag, handles }
}

pub fn start_geiger_mode(
    interface: &str,
    bssid: &str,
    lock_channel: u16,
    sender: Sender<GeigerUpdate>,
    stop_flag: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    let iface = interface.to_string();
    let target_bssid = bssid.to_string();
    thread::spawn(move || {
        let _ = set_channel(&iface, lock_channel);

        let tshark_available = Command::new("which")
            .arg("tshark")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if tshark_available {
            let args = vec![
                "-i".to_string(),
                iface.clone(),
                "-l".to_string(),
                "-n".to_string(),
                "-Y".to_string(),
                format!("wlan.bssid == {}", target_bssid),
                "-T".to_string(),
                "fields".to_string(),
                "-E".to_string(),
                "separator=\t".to_string(),
                "-e".to_string(),
                "radiotap.dbm_antsignal".to_string(),
            ];
            let mut child = match spawn_privileged_tshark(&args) {
                Ok(c) => c.child,
                Err(_) => return,
            };

            if let Some(stdout) = child.stdout.take() {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    if stop_flag.load(Ordering::Relaxed) {
                        let _ = child.kill();
                        break;
                    }
                    let Ok(line) = line else {
                        continue;
                    };
                    let rssi = line.trim().parse::<i32>().unwrap_or(-100);
                    let tone = rssi_to_tone_hz(rssi);
                    let _ = sender.send(GeigerUpdate {
                        rssi_dbm: rssi,
                        tone_hz: tone,
                    });
                }
            }
        } else {
            let mut rng = rand::thread_rng();
            while !stop_flag.load(Ordering::Relaxed) {
                let rssi = rng.gen_range(-90..=-35);
                let _ = sender.send(GeigerUpdate {
                    rssi_dbm: rssi,
                    tone_hz: rssi_to_tone_hz(rssi),
                });
                thread::sleep(Duration::from_millis(150));
            }
        }
    })
}

fn run_interface_capture(
    interface: &InterfaceSettings,
    session_pcap_path: Option<PathBuf>,
    geoip_city_db_path: Option<PathBuf>,
    sender: Sender<CaptureEvent>,
    stop_flag: Arc<AtomicBool>,
) {
    let _ = sender.send(CaptureEvent::Log(format!(
        "starting Wi-Fi capture on {}",
        interface.interface_name
    )));
    let tshark_available = Command::new("which")
        .arg("tshark")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !tshark_available {
        let _ = sender.send(CaptureEvent::Log(format!(
            "tshark not found; running simulated capture on {}",
            interface.interface_name
        )));
        run_simulated_capture(interface, sender, stop_flag);
        return;
    }

    let ssid_field = tshark_pick_supported_field(&["wlan_mgt.ssid", "wlan.ssid"]);
    if ssid_field.is_none() {
        let _ = sender.send(CaptureEvent::Log(
            "no supported tshark SSID field found; SSIDs may be unavailable".to_string(),
        ));
    }
    let eapol_msg_field = tshark_pick_supported_field(&[
        "eapol.keydes.msgnr",
        "wlan_rsna_eapol.keydes.msg_type",
        "wlan_rsna_eapol.keydes.msgnr",
    ]);
    if eapol_msg_field.is_none() {
        let _ = sender.send(CaptureEvent::Log(
            "no supported tshark EAPOL message-number field found; WPA2 handshake counting disabled"
                .to_string(),
        ));
    }
    let rsn_version_field =
        tshark_pick_supported_field(&["wlan_mgt.rsn.version", "wlan.rsn.version"]);
    if rsn_version_field.is_none() {
        let _ = sender.send(CaptureEvent::Log(
            "no supported tshark RSN version field found; WPA2/RSN labeling may be less precise"
                .to_string(),
        ));
    }

    let supports_country_field = tshark_supports_field("wlan.country_info.code");
    if !supports_country_field {
        let _ = sender.send(CaptureEvent::Log(
            "tshark field wlan.country_info.code not available; 802.11d country capture disabled"
                .to_string(),
        ));
    }
    let supports_beacon_tsf_field = tshark_supports_field("wlan.fixed.timestamp");
    if !supports_beacon_tsf_field {
        let _ = sender.send(CaptureEvent::Log(
            "tshark field wlan.fixed.timestamp not available; beacon uptime estimation disabled"
                .to_string(),
        ));
    }
    let supports_privacy_field = tshark_supports_field("wlan.fixed.capabilities.privacy");
    let supports_rsn_akm_field = tshark_supports_field("wlan.rsn.akms.type");
    let supports_rsn_cipher_field = tshark_supports_field("wlan.rsn.pcs.type");
    let supports_rsn_mfpc_field = tshark_supports_field("wlan.rsn.capabilities.mfpc");
    let supports_rsn_mfpr_field = tshark_supports_field("wlan.rsn.capabilities.mfpr");
    let supports_wpa_version_field = tshark_supports_field("wlan.wfa.ie.wpa.version");
    let supports_wpa_akm_field = tshark_supports_field("wlan.wfa.ie.wpa.akms.type");
    let supports_wpa_cipher_field = tshark_supports_field("wlan.wfa.ie.wpa.ucs.type");
    let supports_retry_field = tshark_supports_field("wlan.fc.retry");
    let supports_power_save_field = tshark_supports_field("wlan.fc.pwrmgt");
    let supports_qos_priority_field = tshark_supports_field("wlan.qos.priority");
    let supports_status_code_field = tshark_supports_field("wlan.fixed.status_code");
    let supports_reason_code_field = tshark_supports_field("wlan.fixed.reason_code");
    let supports_listen_interval_field = tshark_supports_field("wlan.fixed.listen_ival");
    let supports_pmkid_count_field = tshark_supports_field("wlan.rsn.pmkid.count");
    let supports_ip_src_field = tshark_supports_field("ip.src");
    let supports_ip_dst_field = tshark_supports_field("ip.dst");
    let supports_ipv6_src_field = tshark_supports_field("ipv6.src");
    let supports_ipv6_dst_field = tshark_supports_field("ipv6.dst");
    let supports_tcp_srcport_field = tshark_supports_field("tcp.srcport");
    let supports_tcp_dstport_field = tshark_supports_field("tcp.dstport");
    let supports_udp_srcport_field = tshark_supports_field("udp.srcport");
    let supports_udp_dstport_field = tshark_supports_field("udp.dstport");
    let supports_arp_src_ipv4_field = tshark_supports_field("arp.src.proto_ipv4");
    let supports_arp_dst_ipv4_field = tshark_supports_field("arp.dst.proto_ipv4");
    let supports_dns_qry_name_field = tshark_supports_field("dns.qry.name");
    let supports_dns_resp_name_field = tshark_supports_field("dns.resp.name");
    let supports_dns_a_field = tshark_supports_field("dns.a");
    let supports_dns_aaaa_field = tshark_supports_field("dns.aaaa");
    let supports_dhcp_hostname_field = tshark_supports_field("dhcp.option.hostname");
    let supports_dhcp_fqdn_field = tshark_supports_field("dhcp.fqdn.name");
    let supports_dhcp_vendor_class_field = tshark_supports_field("dhcp.option.vendor_class_id");
    let supports_dhcp_requested_ip_field =
        tshark_supports_field("dhcp.option.requested_ip_address");
    let supports_dhcp_client_ip_field = tshark_supports_field("dhcp.ip.client");
    let supports_dhcp_your_ip_field = tshark_supports_field("dhcp.ip.your");

    let parse_layout = TSharkParseLayout {
        has_ssid_field: ssid_field.is_some(),
        has_eapol_msg_field: eapol_msg_field.is_some(),
        has_rsn_version_field: rsn_version_field.is_some(),
        has_country_field: supports_country_field,
        has_beacon_tsf_field: supports_beacon_tsf_field,
        has_privacy_field: supports_privacy_field,
        has_rsn_akm_field: supports_rsn_akm_field,
        has_rsn_cipher_field: supports_rsn_cipher_field,
        has_rsn_mfpc_field: supports_rsn_mfpc_field,
        has_rsn_mfpr_field: supports_rsn_mfpr_field,
        has_wpa_version_field: supports_wpa_version_field,
        has_wpa_akm_field: supports_wpa_akm_field,
        has_wpa_cipher_field: supports_wpa_cipher_field,
        has_retry_field: supports_retry_field,
        has_power_save_field: supports_power_save_field,
        has_qos_priority_field: supports_qos_priority_field,
        has_status_code_field: supports_status_code_field,
        has_reason_code_field: supports_reason_code_field,
        has_listen_interval_field: supports_listen_interval_field,
        has_pmkid_count_field: supports_pmkid_count_field,
        has_ip_src_field: supports_ip_src_field,
        has_ip_dst_field: supports_ip_dst_field,
        has_ipv6_src_field: supports_ipv6_src_field,
        has_ipv6_dst_field: supports_ipv6_dst_field,
        has_tcp_srcport_field: supports_tcp_srcport_field,
        has_tcp_dstport_field: supports_tcp_dstport_field,
        has_udp_srcport_field: supports_udp_srcport_field,
        has_udp_dstport_field: supports_udp_dstport_field,
        has_arp_src_ipv4_field: supports_arp_src_ipv4_field,
        has_arp_dst_ipv4_field: supports_arp_dst_ipv4_field,
        has_dns_qry_name_field: supports_dns_qry_name_field,
        has_dns_resp_name_field: supports_dns_resp_name_field,
        has_dns_a_field: supports_dns_a_field,
        has_dns_aaaa_field: supports_dns_aaaa_field,
        has_dhcp_hostname_field: supports_dhcp_hostname_field,
        has_dhcp_fqdn_field: supports_dhcp_fqdn_field,
        has_dhcp_vendor_class_field: supports_dhcp_vendor_class_field,
        has_dhcp_requested_ip_field: supports_dhcp_requested_ip_field,
        has_dhcp_client_ip_field: supports_dhcp_client_ip_field,
        has_dhcp_your_ip_field: supports_dhcp_your_ip_field,
    };

    let mut decoder_args = vec![
        "-i".to_string(),
        interface.interface_name.clone(),
        "-l".to_string(),
        "-n".to_string(),
        "-T".to_string(),
        "fields".to_string(),
        "-E".to_string(),
        "separator=\t".to_string(),
        "-E".to_string(),
        "quote=n".to_string(),
        "-E".to_string(),
        "occurrence=f".to_string(),
    ];
    let mut push_decoder_field = |field: &str| {
        decoder_args.push("-e".to_string());
        decoder_args.push(field.to_string());
    };
    push_decoder_field("frame.time_epoch");
    push_decoder_field("frame.len");
    push_decoder_field("wlan.bssid");
    push_decoder_field("wlan.sa");
    push_decoder_field("wlan.da");
    if let Some(field) = &ssid_field {
        push_decoder_field(field);
    }
    push_decoder_field("radiotap.dbm_antsignal");
    push_decoder_field("wlan_radio.channel");
    push_decoder_field("wlan_radio.frequency");
    push_decoder_field("wlan.fc.type");
    push_decoder_field("wlan.fc.subtype");
    if let Some(field) = &eapol_msg_field {
        push_decoder_field(field);
    }
    if let Some(field) = &rsn_version_field {
        push_decoder_field(field);
    }
    push_decoder_field("wlan.fc.protected");
    if supports_country_field {
        push_decoder_field("wlan.country_info.code");
    }
    if supports_beacon_tsf_field {
        push_decoder_field("wlan.fixed.timestamp");
    }
    if supports_privacy_field {
        push_decoder_field("wlan.fixed.capabilities.privacy");
    }
    if supports_rsn_akm_field {
        push_decoder_field("wlan.rsn.akms.type");
    }
    if supports_rsn_cipher_field {
        push_decoder_field("wlan.rsn.pcs.type");
    }
    if supports_rsn_mfpc_field {
        push_decoder_field("wlan.rsn.capabilities.mfpc");
    }
    if supports_rsn_mfpr_field {
        push_decoder_field("wlan.rsn.capabilities.mfpr");
    }
    if supports_wpa_version_field {
        push_decoder_field("wlan.wfa.ie.wpa.version");
    }
    if supports_wpa_akm_field {
        push_decoder_field("wlan.wfa.ie.wpa.akms.type");
    }
    if supports_wpa_cipher_field {
        push_decoder_field("wlan.wfa.ie.wpa.ucs.type");
    }
    if supports_retry_field {
        push_decoder_field("wlan.fc.retry");
    }
    if supports_power_save_field {
        push_decoder_field("wlan.fc.pwrmgt");
    }
    if supports_qos_priority_field {
        push_decoder_field("wlan.qos.priority");
    }
    if supports_status_code_field {
        push_decoder_field("wlan.fixed.status_code");
    }
    if supports_reason_code_field {
        push_decoder_field("wlan.fixed.reason_code");
    }
    if supports_listen_interval_field {
        push_decoder_field("wlan.fixed.listen_ival");
    }
    if supports_pmkid_count_field {
        push_decoder_field("wlan.rsn.pmkid.count");
    }
    if supports_ip_src_field {
        push_decoder_field("ip.src");
    }
    if supports_ip_dst_field {
        push_decoder_field("ip.dst");
    }
    if supports_ipv6_src_field {
        push_decoder_field("ipv6.src");
    }
    if supports_ipv6_dst_field {
        push_decoder_field("ipv6.dst");
    }
    if supports_tcp_srcport_field {
        push_decoder_field("tcp.srcport");
    }
    if supports_tcp_dstport_field {
        push_decoder_field("tcp.dstport");
    }
    if supports_udp_srcport_field {
        push_decoder_field("udp.srcport");
    }
    if supports_udp_dstport_field {
        push_decoder_field("udp.dstport");
    }
    if supports_arp_src_ipv4_field {
        push_decoder_field("arp.src.proto_ipv4");
    }
    if supports_arp_dst_ipv4_field {
        push_decoder_field("arp.dst.proto_ipv4");
    }
    if supports_dns_qry_name_field {
        push_decoder_field("dns.qry.name");
    }
    if supports_dns_resp_name_field {
        push_decoder_field("dns.resp.name");
    }
    if supports_dns_a_field {
        push_decoder_field("dns.a");
    }
    if supports_dns_aaaa_field {
        push_decoder_field("dns.aaaa");
    }
    if supports_dhcp_hostname_field {
        push_decoder_field("dhcp.option.hostname");
    }
    if supports_dhcp_fqdn_field {
        push_decoder_field("dhcp.fqdn.name");
    }
    if supports_dhcp_vendor_class_field {
        push_decoder_field("dhcp.option.vendor_class_id");
    }
    if supports_dhcp_requested_ip_field {
        push_decoder_field("dhcp.option.requested_ip_address");
    }
    if supports_dhcp_client_ip_field {
        push_decoder_field("dhcp.ip.client");
    }
    if supports_dhcp_your_ip_field {
        push_decoder_field("dhcp.ip.your");
    }

    let mut decoder = match spawn_privileged_tshark(&decoder_args) {
        Ok(proc) => {
            let _ = sender.send(CaptureEvent::Log(format!(
                "privileged Wi-Fi capture running on {} via {}",
                interface.interface_name, proc.launch_mode
            )));
            proc.child
        }
        Err(err) => {
            let _ = sender.send(CaptureEvent::Log(format!(
                "failed to start privileged tshark on {}: {}",
                interface.interface_name, err
            )));
            return;
        }
    };

    let Some(decoder_stdout) = decoder.stdout.take() else {
        let _ = sender.send(CaptureEvent::Log(format!(
            "live decoder stdout unavailable on {}",
            interface.interface_name
        )));
        let _ = decoder.kill();
        let _ = decoder.wait();
        return;
    };

    let decoder_stderr_handle = decoder.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut buf = String::new();
            let _ = stderr.read_to_string(&mut buf);
            buf
        })
    });

    let mut saver = None;
    let saver_stderr_handle = if let Some(path) = session_pcap_path.as_ref() {
        let saver_args = vec![
            "-i".to_string(),
            interface.interface_name.clone(),
            "-n".to_string(),
            "-Q".to_string(),
            "-w".to_string(),
            path.display().to_string(),
        ];

        match spawn_privileged_tshark(&saver_args) {
            Ok(proc) => {
                let _ = sender.send(CaptureEvent::Log(format!(
                    "privileged PCAP saver running on {} via {}",
                    interface.interface_name, proc.launch_mode
                )));
                let mut child = proc.child;
                let stderr = child.stderr.take().map(|mut stderr| {
                    thread::spawn(move || {
                        let mut buf = String::new();
                        let _ = stderr.read_to_string(&mut buf);
                        buf
                    })
                });
                saver = Some(child);
                stderr
            }
            Err(err) => {
                let _ = sender.send(CaptureEvent::Log(format!(
                    "failed to start privileged PCAP saver on {}: {}",
                    interface.interface_name, err
                )));
                None
            }
        }
    } else {
        None
    };

    let parse_sender = sender.clone();
    let parse_stop = Arc::clone(&stop_flag);
    let parse_iface = interface.interface_name.clone();
    let parse_handle = thread::spawn(move || {
        process_live_tshark_fields(
            BufReader::new(decoder_stdout),
            parse_layout,
            geoip_city_db_path.as_deref(),
            &parse_sender,
            &parse_stop,
            &parse_iface,
        )
    });

    let mut decoder_exit_status = None;
    let mut saver_exit_status = None;
    while !stop_flag.load(Ordering::Relaxed) {
        if decoder_exit_status.is_none() {
            decoder_exit_status = decoder.try_wait().ok().flatten();
        }
        if let Some(saver_child) = saver.as_mut() {
            if saver_exit_status.is_none() {
                saver_exit_status = saver_child.try_wait().ok().flatten();
            }
        }
        if decoder_exit_status.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(150));
    }

    if stop_flag.load(Ordering::Relaxed) {
        let _ = decoder.kill();
        if let Some(saver_child) = saver.as_mut() {
            let _ = saver_child.kill();
        }
    }

    let saw_frames = parse_handle.join().unwrap_or(false);

    if decoder_exit_status.is_none() {
        decoder_exit_status = decoder.wait().ok();
    }
    if saver_exit_status.is_none() {
        if let Some(mut saver_child) = saver {
            saver_exit_status = saver_child.wait().ok();
        }
    }

    let decoder_stderr_text = decoder_stderr_handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    let saver_stderr_text = saver_stderr_handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();

    let stderr_text = [decoder_stderr_text, saver_stderr_text]
        .into_iter()
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let exit_status = decoder_exit_status.or(saver_exit_status);
    if !saw_frames && !stop_flag.load(Ordering::Relaxed) {
        let stderr_summary = stderr_text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .filter(|line| !line.starts_with("Running as user"))
            .filter(|line| !line.contains("This could be dangerous"))
            .filter(|line| !line.contains("androiddump"))
            .take(6)
            .collect::<Vec<_>>()
            .join(" | ");
        let _ = sender.send(CaptureEvent::Log(format!(
            "no Wi-Fi packets observed on {}; verify monitor mode, channel control, and live decoder path{}{}",
            interface.interface_name,
            exit_status
                .map(|s| format!(" (tshark exited with {})", s))
                .unwrap_or_default(),
            if stderr_summary.is_empty() {
                String::new()
            } else {
                format!(" | {}", stderr_summary)
            }
        )));
    }
}

fn process_live_tshark_fields(
    reader: BufReader<ChildStdout>,
    parse_layout: TSharkParseLayout,
    geoip_city_db_path: Option<&std::path::Path>,
    sender: &Sender<CaptureEvent>,
    stop_flag: &Arc<AtomicBool>,
    interface_name: &str,
) -> bool {
    let mut ap_state: HashMap<String, AccessPointRecord> = HashMap::new();
    let mut client_state: HashMap<String, ClientRecord> = HashMap::new();
    let mut handshake_state: HashMap<(String, String), HashSet<u8>> = HashMap::new();
    let mut ap_clients: HashMap<String, HashSet<String>> = HashMap::new();
    let mut channel_counts: HashMap<u16, u64> = HashMap::new();
    let mut dns_ip_map: HashMap<String, String> = HashMap::new();
    let mut geo_lookup = GeoIpLookup::with_preferred_path(geoip_city_db_path);
    let mut usage_tick = Instant::now();
    let mut saw_frames = false;

    if let Some(path) = geo_lookup.source_path() {
        let _ = sender.send(CaptureEvent::Log(format!(
            "IP geolocation enabled from {}",
            path.display()
        )));
    } else {
        let _ = sender.send(CaptureEvent::Log(
            "IP geolocation disabled; no local city database found".to_string(),
        ));
    }

    for line in reader.lines() {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        let Ok(line) = line else {
            continue;
        };

        if line.trim().is_empty() {
            continue;
        }

        if let Some(frame) = parse_tshark_line(&line, parse_layout) {
            if !saw_frames {
                let _ = sender.send(CaptureEvent::Log(format!(
                    "first live Wi-Fi frame decoded on {}",
                    interface_name
                )));
            }
            saw_frames = true;
            let now = frame
                .epoch
                .and_then(|s| Utc.timestamp_opt(s as i64, 0).single())
                .unwrap_or_else(Utc::now);

            let bssid = normalize_mac(frame.bssid.clone().unwrap_or_default());
            let bssid_is_broadcast = is_broadcast_mac(&bssid);
            let is_probe_request = frame.fc_type == Some(0) && frame.subtype == Some(4);
            let is_probe_response = frame.fc_type == Some(0) && frame.subtype == Some(5);
            let is_beacon = frame.fc_type == Some(0) && frame.subtype == Some(8);
            let is_open_payload = !frame.protected;

            if is_open_payload {
                update_passive_dns_map(&frame, &mut dns_ip_map);
            }

            if !bssid.is_empty() && !bssid_is_broadcast {
                let ap = ap_state
                    .entry(bssid.clone())
                    .or_insert_with(|| AccessPointRecord::new(bssid.clone(), now));

                ap.last_seen = now;
                if ap.first_seen > now {
                    ap.first_seen = now;
                }

                if let Some(ssid) = frame.ssid.clone().filter(|s| !s.is_empty()) {
                    ap.ssid = Some(ssid);
                }
                if let Some(country_code) = frame.country_code.clone().filter(|v| !v.is_empty()) {
                    ap.country_code_80211d = Some(country_code);
                }
                if is_beacon {
                    if let Some(tsf_us) = frame.beacon_tsf_us.filter(|v| *v > 0) {
                        ap.uptime_beacons = Some(tsf_us / 1_000_000);
                    }
                }
                ap.channel = frame.channel;
                ap.frequency_mhz = frame.frequency;
                ap.band = SpectrumBand::from_frequency_mhz(ap.frequency_mhz);
                ap.rssi_dbm = frame.rssi;
                if let Some((short, full)) = classify_ap_encryption(&frame) {
                    ap.encryption_short = short;
                    ap.encryption_full = full;
                }
                ap.number_of_clients =
                    ap_clients.get(&bssid).map(|set| set.len()).unwrap_or(0) as u32;

                match frame.fc_type {
                    Some(0) => ap.packet_mix.management += 1,
                    Some(1) => ap.packet_mix.control += 1,
                    Some(2) => ap.packet_mix.data += 1,
                    _ => ap.packet_mix.other += 1,
                }

                let _ = sender.send(CaptureEvent::AccessPointSeen(ap.clone()));
            }

            if let Some(channel) = frame.channel {
                *channel_counts.entry(channel).or_insert(0) += 1;
            }

            if let Some(client_mac) = infer_client_mac(&bssid, &frame.sa, &frame.da) {
                if !client_mac.is_empty() {
                    let client = client_state
                        .entry(client_mac.clone())
                        .or_insert_with(|| ClientRecord::new(client_mac.clone(), now));
                    let direction = client_traffic_direction(&client_mac, &frame.sa, &frame.da);
                    client.last_seen = now;
                    if client.first_seen > now {
                        client.first_seen = now;
                    }
                    client.associated_ap = if bssid.is_empty() || bssid_is_broadcast {
                        client.associated_ap.clone()
                    } else {
                        Some(bssid.clone())
                    };
                    client.rssi_dbm = frame.rssi;
                    client.data_transferred_bytes += frame.frame_len.unwrap_or(0) as u64;
                    client.network_intel.last_channel =
                        frame.channel.or(client.network_intel.last_channel);
                    client.network_intel.last_frequency_mhz =
                        frame.frequency.or(client.network_intel.last_frequency_mhz);
                    client.network_intel.band =
                        SpectrumBand::from_frequency_mhz(client.network_intel.last_frequency_mhz);
                    client.network_intel.last_frame_type =
                        frame.fc_type.or(client.network_intel.last_frame_type);
                    client.network_intel.last_frame_subtype =
                        frame.subtype.or(client.network_intel.last_frame_subtype);
                    client.network_intel.last_status_code =
                        frame.status_code.or(client.network_intel.last_status_code);
                    client.network_intel.last_reason_code =
                        frame.reason_code.or(client.network_intel.last_reason_code);
                    client.network_intel.listen_interval = frame
                        .listen_interval
                        .or(client.network_intel.listen_interval);
                    client.network_intel.pmkid_count = client
                        .network_intel
                        .pmkid_count
                        .max(frame.pmkid_count.unwrap_or_default() as u32);
                    client.network_intel.power_save_observed |= frame.power_save.unwrap_or(false);
                    push_unique_u8(
                        &mut client.network_intel.qos_priorities,
                        frame.qos_priority,
                        8,
                    );
                    if frame.retry.unwrap_or(false) {
                        client.network_intel.retry_frame_count =
                            client.network_intel.retry_frame_count.saturating_add(1);
                    }
                    if frame.eapol_msg.is_some() {
                        client.network_intel.eapol_frame_count =
                            client.network_intel.eapol_frame_count.saturating_add(1);
                    }
                    match frame.fc_type {
                        Some(0) => client.network_intel.packet_mix.management += 1,
                        Some(1) => client.network_intel.packet_mix.control += 1,
                        Some(2) => client.network_intel.packet_mix.data += 1,
                        _ => client.network_intel.packet_mix.other += 1,
                    }
                    match direction {
                        ClientTrafficDirection::Uplink => {
                            client.network_intel.uplink_bytes = client
                                .network_intel
                                .uplink_bytes
                                .saturating_add(frame.frame_len.unwrap_or(0) as u64);
                        }
                        ClientTrafficDirection::Downlink => {
                            client.network_intel.downlink_bytes = client
                                .network_intel
                                .downlink_bytes
                                .saturating_add(frame.frame_len.unwrap_or(0) as u64);
                        }
                        ClientTrafficDirection::Unknown => {}
                    }
                    if !bssid.is_empty()
                        && !bssid_is_broadcast
                        && !client.seen_access_points.contains(&bssid)
                    {
                        client.seen_access_points.push(bssid.clone());
                        let assoc_clients = ap_clients.entry(bssid.clone()).or_default();
                        assoc_clients.insert(client_mac.clone());
                        if let Some(ap) = ap_state.get_mut(&bssid) {
                            ap.number_of_clients = assoc_clients.len() as u32;
                            let _ = sender.send(CaptureEvent::AccessPointSeen(ap.clone()));
                        }
                    }
                    if is_probe_request {
                        if let Some(probe_ssid) = frame.ssid.clone().filter(|v| !v.is_empty()) {
                            if !client.probes.contains(&probe_ssid) {
                                client.probes.push(probe_ssid);
                            }
                        }
                    }
                    if (is_beacon || is_probe_response) && !bssid_is_broadcast {
                        client.associated_ap = Some(bssid.clone());
                    }
                    if is_open_payload {
                        observe_open_network_activity(
                            client,
                            direction,
                            &frame,
                            &dns_ip_map,
                            &mut geo_lookup,
                            now,
                        );
                    } else {
                        backfill_endpoint_domains(
                            &mut client.network_intel.remote_endpoints,
                            &dns_ip_map,
                            &mut geo_lookup,
                        );
                    }

                    let _ = sender.send(CaptureEvent::ClientSeen(client.clone()));
                }
            }

            if let Some(msg_no) = frame.eapol_msg {
                if (1..=4).contains(&msg_no) && !bssid.is_empty() && !bssid_is_broadcast {
                    if let Some(client_mac) = infer_client_mac(&bssid, &frame.sa, &frame.da) {
                        let key = (bssid.clone(), client_mac.clone());
                        let set = handshake_state.entry(key.clone()).or_default();
                        set.insert(msg_no);

                        if set.len() == 4 {
                            let record = HandshakeRecord {
                                bssid: key.0.clone(),
                                client_mac: key.1.clone(),
                                timestamp: now,
                                full_wpa2_4way: true,
                                pcap_path: None,
                            };
                            if let Some(client) = client_state.get_mut(&key.1) {
                                if !client.handshake_networks.contains(&key.0) {
                                    client.handshake_networks.push(key.0.clone());
                                    let _ = sender.send(CaptureEvent::ClientSeen(client.clone()));
                                }
                            }
                            let _ = sender.send(CaptureEvent::HandshakeSeen(record));
                            set.clear();
                        }
                    }
                }
            }

            if usage_tick.elapsed() >= Duration::from_secs(1) {
                let max_packets = channel_counts.values().copied().max().unwrap_or(1);
                for (channel, packets) in channel_counts.drain() {
                    let utilization = (packets as f32 / max_packets as f32) * 100.0;
                    let usage = ChannelUsagePoint {
                        timestamp: now,
                        channel,
                        band: SpectrumBand::from_frequency_mhz(frame.frequency),
                        utilization_percent: utilization,
                        packets,
                    };
                    let _ = sender.send(CaptureEvent::ChannelUsage(usage));
                }
                usage_tick = Instant::now();
            }
        }
    }

    if !saw_frames {
        let _ = sender.send(CaptureEvent::Log(format!(
            "live decoder received no parsed 802.11 frames on {}",
            interface_name
        )));
    }

    saw_frames
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientTrafficDirection {
    Uplink,
    Downlink,
    Unknown,
}

fn client_traffic_direction(
    client_mac: &str,
    sa: &Option<String>,
    da: &Option<String>,
) -> ClientTrafficDirection {
    let sa = sa.as_ref().map(|value| normalize_mac(value.clone()));
    let da = da.as_ref().map(|value| normalize_mac(value.clone()));

    if sa.as_deref() == Some(client_mac) {
        ClientTrafficDirection::Uplink
    } else if da.as_deref() == Some(client_mac) {
        ClientTrafficDirection::Downlink
    } else {
        ClientTrafficDirection::Unknown
    }
}

fn push_unique_string(values: &mut Vec<String>, value: Option<String>, limit: usize) {
    let Some(value) = value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    else {
        return;
    };
    if values.iter().any(|existing| existing == &value) {
        return;
    }
    values.push(value);
    if values.len() > limit {
        values.drain(0..values.len().saturating_sub(limit));
    }
}

fn push_unique_u8(values: &mut Vec<u8>, value: Option<u8>, limit: usize) {
    let Some(value) = value else {
        return;
    };
    if values.iter().any(|existing| *existing == value) {
        return;
    }
    values.push(value);
    values.sort_unstable();
    if values.len() > limit {
        values.truncate(limit);
    }
}

fn update_passive_dns_map(frame: &ParsedFrame, dns_ip_map: &mut HashMap<String, String>) {
    let Some(domain) = frame
        .dns_resp_name
        .clone()
        .or_else(|| frame.dns_qry_name.clone())
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    if let Some(ipv4) = frame.dns_a.clone().filter(|value| !value.is_empty()) {
        dns_ip_map.insert(ipv4, domain.clone());
    }
    if let Some(ipv6) = frame.dns_aaaa.clone().filter(|value| !value.is_empty()) {
        dns_ip_map.insert(ipv6, domain);
    }
}

fn backfill_endpoint_domains(
    endpoints: &mut [ClientEndpointRecord],
    dns_ip_map: &HashMap<String, String>,
    geo_lookup: &mut GeoIpLookup,
) {
    for endpoint in endpoints.iter_mut() {
        if endpoint.domain.is_none() {
            endpoint.domain = dns_ip_map.get(&endpoint.ip_address).cloned();
        }
        if endpoint.geo_city.is_none() {
            endpoint.geo_city = geo_lookup.lookup_city_label(&endpoint.ip_address);
        }
    }
}

fn upsert_client_endpoint(
    client: &mut ClientRecord,
    ip_address: String,
    protocol: &str,
    port: Option<u16>,
    domain: Option<String>,
    geo_city: Option<String>,
    now: chrono::DateTime<Utc>,
) {
    if ip_address.is_empty() {
        return;
    }

    if let Some(existing) = client
        .network_intel
        .remote_endpoints
        .iter_mut()
        .find(|endpoint| {
            endpoint.ip_address == ip_address
                && endpoint.port == port
                && endpoint.protocol.eq_ignore_ascii_case(protocol)
        })
    {
        existing.last_seen = now;
        existing.packet_count = existing.packet_count.saturating_add(1);
        if existing.domain.is_none() && domain.is_some() {
            existing.domain = domain;
        }
        if existing.geo_city.is_none() && geo_city.is_some() {
            existing.geo_city = geo_city;
        }
        return;
    }

    client
        .network_intel
        .remote_endpoints
        .push(ClientEndpointRecord {
            ip_address,
            protocol: protocol.to_string(),
            port,
            domain,
            geo_city,
            first_seen: now,
            last_seen: now,
            packet_count: 1,
        });
    client.network_intel.remote_endpoints.sort_by(|a, b| {
        b.last_seen
            .cmp(&a.last_seen)
            .then_with(|| b.packet_count.cmp(&a.packet_count))
    });
    if client.network_intel.remote_endpoints.len() > 64 {
        client.network_intel.remote_endpoints.truncate(64);
    }
}

fn observe_open_network_activity(
    client: &mut ClientRecord,
    direction: ClientTrafficDirection,
    frame: &ParsedFrame,
    dns_ip_map: &HashMap<String, String>,
    geo_lookup: &mut GeoIpLookup,
    now: chrono::DateTime<Utc>,
) {
    match direction {
        ClientTrafficDirection::Uplink => {
            push_unique_string(
                &mut client.network_intel.local_ipv4_addresses,
                frame
                    .dhcp_client_ip
                    .clone()
                    .or_else(|| frame.dhcp_requested_ip.clone())
                    .or_else(|| frame.arp_src_ipv4.clone())
                    .or_else(|| frame.ip_src.clone()),
                16,
            );
            push_unique_string(
                &mut client.network_intel.local_ipv6_addresses,
                frame.ipv6_src.clone(),
                16,
            );
        }
        ClientTrafficDirection::Downlink => {
            push_unique_string(
                &mut client.network_intel.local_ipv4_addresses,
                frame
                    .dhcp_your_ip
                    .clone()
                    .or_else(|| frame.arp_dst_ipv4.clone())
                    .or_else(|| frame.ip_dst.clone()),
                16,
            );
            push_unique_string(
                &mut client.network_intel.local_ipv6_addresses,
                frame.ipv6_dst.clone(),
                16,
            );
        }
        ClientTrafficDirection::Unknown => {}
    }

    push_unique_string(
        &mut client.network_intel.local_ipv4_addresses,
        frame.dhcp_client_ip.clone(),
        16,
    );
    push_unique_string(
        &mut client.network_intel.local_ipv4_addresses,
        frame.dhcp_your_ip.clone(),
        16,
    );
    push_unique_string(
        &mut client.network_intel.dhcp_hostnames,
        frame.dhcp_hostname.clone(),
        16,
    );
    push_unique_string(
        &mut client.network_intel.dhcp_fqdns,
        frame.dhcp_fqdn.clone(),
        16,
    );
    push_unique_string(
        &mut client.network_intel.dhcp_vendor_classes,
        frame.dhcp_vendor_class_id.clone(),
        16,
    );
    push_unique_string(
        &mut client.network_intel.dns_names,
        frame.dns_qry_name.clone(),
        32,
    );
    push_unique_string(
        &mut client.network_intel.dns_names,
        frame.dns_resp_name.clone(),
        32,
    );

    let (remote_ip, remote_port, protocol) = match direction {
        ClientTrafficDirection::Uplink => (
            frame
                .ip_dst
                .clone()
                .or_else(|| frame.ipv6_dst.clone())
                .or_else(|| frame.arp_dst_ipv4.clone()),
            frame.tcp_dstport.or(frame.udp_dstport),
            if frame.tcp_dstport.is_some() {
                "TCP"
            } else if frame.udp_dstport.is_some() {
                "UDP"
            } else if frame.arp_dst_ipv4.is_some() {
                "ARP"
            } else {
                "IP"
            },
        ),
        ClientTrafficDirection::Downlink => (
            frame
                .ip_src
                .clone()
                .or_else(|| frame.ipv6_src.clone())
                .or_else(|| frame.arp_src_ipv4.clone()),
            frame.tcp_srcport.or(frame.udp_srcport),
            if frame.tcp_srcport.is_some() {
                "TCP"
            } else if frame.udp_srcport.is_some() {
                "UDP"
            } else if frame.arp_src_ipv4.is_some() {
                "ARP"
            } else {
                "IP"
            },
        ),
        ClientTrafficDirection::Unknown => (None, None, "IP"),
    };

    if let Some(remote_ip) = remote_ip {
        let domain = dns_ip_map.get(&remote_ip).cloned();
        let geo_city = geo_lookup.lookup_city_label(&remote_ip);
        upsert_client_endpoint(
            client,
            remote_ip,
            protocol,
            remote_port,
            domain,
            geo_city,
            now,
        );
    }

    backfill_endpoint_domains(
        &mut client.network_intel.remote_endpoints,
        dns_ip_map,
        geo_lookup,
    );
}

fn run_channel_control_loop(
    interface_name: &str,
    mode: ChannelSelectionMode,
    sender: Sender<CaptureEvent>,
    stop_flag: Arc<AtomicBool>,
) {
    let (channels, dwell_ms, locked) = match mode {
        ChannelSelectionMode::HopAll { channels, dwell_ms } => (channels, dwell_ms, None),
        ChannelSelectionMode::HopBand {
            channels, dwell_ms, ..
        } => (channels, dwell_ms, None),
        ChannelSelectionMode::Locked { channel, ht_mode } => {
            (vec![channel], 0, Some((channel, ht_mode)))
        }
    };

    if let Some((channel, ht_mode)) = locked {
        match set_channel_with_ht(interface_name, channel, &ht_mode) {
            Ok(()) => {
                let _ = sender.send(CaptureEvent::Log(format!(
                    "{} locked to channel {} ({})",
                    interface_name, channel, ht_mode
                )));
            }
            Err(err) => {
                let _ = sender.send(CaptureEvent::Log(format!(
                    "failed to lock {} to channel {} ({}): {}",
                    interface_name, channel, ht_mode, err
                )));
            }
        }
        while !stop_flag.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_secs(1));
        }
        return;
    }

    if channels.is_empty() {
        let _ = sender.send(CaptureEvent::Log(format!(
            "channel hopping disabled on {}; no channels selected",
            interface_name
        )));
        while !stop_flag.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(250));
        }
        return;
    }

    let dwell = if dwell_ms == 0 { 200 } else { dwell_ms };
    if is_effective_root() {
        let mut active_channels = channels.clone();
        let _ = sender.send(CaptureEvent::Log(format!(
            "channel hopper running on {} across {} channels at {} ms dwell (direct root mode)",
            interface_name,
            active_channels.len(),
            dwell
        )));
        let mut index = 0usize;
        while !stop_flag.load(Ordering::Relaxed) && !active_channels.is_empty() {
            let channel = active_channels[index % active_channels.len()];
            if let Err(err) = set_channel_with_ht_direct(interface_name, channel, "HT20") {
                let _ = sender.send(CaptureEvent::Log(format!(
                    "channel hop set failed on {} channel {} (HT20): {}",
                    interface_name, channel, err
                )));
                active_channels.retain(|candidate| *candidate != channel);
                if active_channels.is_empty() {
                    let _ = sender.send(CaptureEvent::Log(format!(
                        "channel hopper stopped on {}; no valid channels remain after removing channel {}",
                        interface_name, channel
                    )));
                    break;
                }
                let _ = sender.send(CaptureEvent::Log(format!(
                    "removed invalid channel {} from hopper on {}; {} channels remain",
                    channel,
                    interface_name,
                    active_channels.len()
                )));
                if index >= active_channels.len() {
                    index = 0;
                }
            }
            thread::sleep(Duration::from_millis(dwell));
            index += 1;
        }
        return;
    }

    let mut hopper = match spawn_privileged_channel_hopper(interface_name, dwell, "HT20", &channels)
    {
        Ok(proc) => {
            let _ = sender.send(CaptureEvent::Log(format!(
                "channel hopper running on {} across {} channels at {} ms dwell",
                interface_name,
                channels.len(),
                dwell
            )));
            proc.child
        }
        Err(err) => {
            let _ = sender.send(CaptureEvent::Log(format!(
                "failed to start channel hopper on {}: {}",
                interface_name, err
            )));
            return;
        }
    };

    let mut stderr_handle = hopper.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut buf = String::new();
            let _ = stderr.read_to_string(&mut buf);
            buf
        })
    });

    while !stop_flag.load(Ordering::Relaxed) {
        if let Ok(Some(status)) = hopper.try_wait() {
            let stderr_summary = stderr_handle
                .take()
                .and_then(|handle| handle.join().ok())
                .unwrap_or_default()
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .take(4)
                .collect::<Vec<_>>()
                .join(" | ");
            let _ = sender.send(CaptureEvent::Log(format!(
                "channel hopper exited on {} with {}{}",
                interface_name,
                status,
                if stderr_summary.is_empty() {
                    String::new()
                } else {
                    format!(" | {}", stderr_summary)
                }
            )));
            return;
        }
        thread::sleep(Duration::from_millis(150));
    }

    let _ = hopper.kill();
    let _ = hopper.wait();
    if let Some(handle) = stderr_handle.take() {
        let _ = handle.join();
    }
}

fn phy_index_for_interface(interface: &str) -> Result<Option<String>> {
    let output = Command::new("iw")
        .arg("dev")
        .arg(interface)
        .arg("info")
        .output()
        .with_context(|| format!("failed to run iw dev {} info", interface))?;

    if !output.status.success() {
        return Ok(None);
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.trim().strip_prefix("wiphy ").map(str::to_string)))
}

fn phy_info_text_for_interface(interface: &str) -> Result<Option<String>> {
    let Some(phy_index) = phy_index_for_interface(interface)? else {
        return Ok(None);
    };

    let output = Command::new("iw")
        .arg(format!("phy{}", phy_index))
        .arg("info")
        .output()
        .context("failed to run iw phy info")?;

    if !output.status.success() {
        return Ok(None);
    }

    Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()))
}

fn run_simulated_capture(
    interface: &InterfaceSettings,
    sender: Sender<CaptureEvent>,
    stop_flag: Arc<AtomicBool>,
) {
    let mut rng = rand::thread_rng();
    let mut tick: u64 = 0;

    while !stop_flag.load(Ordering::Relaxed) {
        let now = Utc::now();

        let ap_bssid = format!("AA:BB:CC:DD:EE:{:02X}", (tick % 32) as u8);
        let mut ap = AccessPointRecord::new(ap_bssid.clone(), now);
        ap.ssid = Some(format!("DemoNet-{}", tick % 8));
        ap.country_code_80211d = Some("US".to_string());
        ap.uptime_beacons = Some(86_400 + tick * 2);
        ap.channel = Some([1, 6, 11, 36, 40, 44, 149][(tick as usize) % 7]);
        ap.frequency_mhz = Some(match ap.channel.unwrap_or(1) {
            1 => 2412,
            6 => 2437,
            11 => 2462,
            36 => 5180,
            40 => 5200,
            44 => 5220,
            _ => 5745,
        });
        ap.band = SpectrumBand::from_frequency_mhz(ap.frequency_mhz);
        ap.encryption_short = "WPA2".to_string();
        ap.encryption_full = "WPA2-PSK-CCMP".to_string();
        ap.rssi_dbm = Some(rng.gen_range(-88..=-35));
        ap.handshake_count = (tick % 4) as u32;
        ap.number_of_clients = (tick % 5) as u32;
        ap.packet_mix = PacketTypeBreakdown {
            management: rng.gen_range(20..200),
            control: rng.gen_range(10..180),
            data: rng.gen_range(100..1200),
            other: rng.gen_range(0..20),
        };

        let _ = sender.send(CaptureEvent::AccessPointSeen(ap.clone()));

        let mut client =
            ClientRecord::new(format!("DE:AD:BE:EF:00:{:02X}", (tick % 16) as u8), now);
        client.associated_ap = Some(ap_bssid.clone());
        client.rssi_dbm = Some(rng.gen_range(-88..=-35));
        client.data_transferred_bytes = rng.gen_range(3000..3_000_000);
        client.probes = vec!["CoffeeShopWiFi".into(), "GuestNet".into()];

        let _ = sender.send(CaptureEvent::ClientSeen(client.clone()));

        let usage = ChannelUsagePoint {
            timestamp: now,
            channel: ap.channel.unwrap_or(1),
            band: ap.band.clone(),
            utilization_percent: rng.gen_range(5.0..95.0),
            packets: rng.gen_range(10..1000),
        };
        let _ = sender.send(CaptureEvent::ChannelUsage(usage));

        if tick % 20 == 0 {
            let hs = HandshakeRecord {
                bssid: ap_bssid,
                client_mac: client.mac,
                timestamp: now,
                full_wpa2_4way: true,
                pcap_path: None,
            };
            let _ = sender.send(CaptureEvent::HandshakeSeen(hs));
        }

        let _ = sender.send(CaptureEvent::Log(format!(
            "sim capture tick {} on {}",
            tick, interface.interface_name
        )));

        tick += 1;
        thread::sleep(Duration::from_millis(700));
    }
}

#[derive(Debug)]
struct ParsedFrame {
    epoch: Option<f64>,
    frame_len: Option<u32>,
    bssid: Option<String>,
    sa: Option<String>,
    da: Option<String>,
    ssid: Option<String>,
    rssi: Option<i32>,
    channel: Option<u16>,
    frequency: Option<u32>,
    fc_type: Option<u8>,
    subtype: Option<u8>,
    eapol_msg: Option<u8>,
    rsn_version: Option<u8>,
    protected: bool,
    country_code: Option<String>,
    beacon_tsf_us: Option<u64>,
    capability_privacy: Option<bool>,
    rsn_akm_type: Option<u8>,
    rsn_cipher_type: Option<u8>,
    rsn_mfpc: Option<bool>,
    rsn_mfpr: Option<bool>,
    wpa_version: Option<u8>,
    wpa_akm_type: Option<u8>,
    wpa_cipher_type: Option<u8>,
    retry: Option<bool>,
    power_save: Option<bool>,
    qos_priority: Option<u8>,
    status_code: Option<u16>,
    reason_code: Option<u16>,
    listen_interval: Option<u16>,
    pmkid_count: Option<u16>,
    ip_src: Option<String>,
    ip_dst: Option<String>,
    ipv6_src: Option<String>,
    ipv6_dst: Option<String>,
    tcp_srcport: Option<u16>,
    tcp_dstport: Option<u16>,
    udp_srcport: Option<u16>,
    udp_dstport: Option<u16>,
    arp_src_ipv4: Option<String>,
    arp_dst_ipv4: Option<String>,
    dns_qry_name: Option<String>,
    dns_resp_name: Option<String>,
    dns_a: Option<String>,
    dns_aaaa: Option<String>,
    dhcp_hostname: Option<String>,
    dhcp_fqdn: Option<String>,
    dhcp_vendor_class_id: Option<String>,
    dhcp_requested_ip: Option<String>,
    dhcp_client_ip: Option<String>,
    dhcp_your_ip: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct TSharkParseLayout {
    has_ssid_field: bool,
    has_eapol_msg_field: bool,
    has_rsn_version_field: bool,
    has_country_field: bool,
    has_beacon_tsf_field: bool,
    has_privacy_field: bool,
    has_rsn_akm_field: bool,
    has_rsn_cipher_field: bool,
    has_rsn_mfpc_field: bool,
    has_rsn_mfpr_field: bool,
    has_wpa_version_field: bool,
    has_wpa_akm_field: bool,
    has_wpa_cipher_field: bool,
    has_retry_field: bool,
    has_power_save_field: bool,
    has_qos_priority_field: bool,
    has_status_code_field: bool,
    has_reason_code_field: bool,
    has_listen_interval_field: bool,
    has_pmkid_count_field: bool,
    has_ip_src_field: bool,
    has_ip_dst_field: bool,
    has_ipv6_src_field: bool,
    has_ipv6_dst_field: bool,
    has_tcp_srcport_field: bool,
    has_tcp_dstport_field: bool,
    has_udp_srcport_field: bool,
    has_udp_dstport_field: bool,
    has_arp_src_ipv4_field: bool,
    has_arp_dst_ipv4_field: bool,
    has_dns_qry_name_field: bool,
    has_dns_resp_name_field: bool,
    has_dns_a_field: bool,
    has_dns_aaaa_field: bool,
    has_dhcp_hostname_field: bool,
    has_dhcp_fqdn_field: bool,
    has_dhcp_vendor_class_field: bool,
    has_dhcp_requested_ip_field: bool,
    has_dhcp_client_ip_field: bool,
    has_dhcp_your_ip_field: bool,
}

fn parse_tshark_line(line: &str, layout: TSharkParseLayout) -> Option<ParsedFrame> {
    let fields: Vec<&str> = line.split('\t').collect();
    let required = 10
        + usize::from(layout.has_ssid_field)
        + usize::from(layout.has_eapol_msg_field)
        + usize::from(layout.has_rsn_version_field)
        + 1 // wlan.fc.protected
        + usize::from(layout.has_country_field)
        + usize::from(layout.has_beacon_tsf_field)
        + usize::from(layout.has_privacy_field)
        + usize::from(layout.has_rsn_akm_field)
        + usize::from(layout.has_rsn_cipher_field)
        + usize::from(layout.has_rsn_mfpc_field)
        + usize::from(layout.has_rsn_mfpr_field)
        + usize::from(layout.has_wpa_version_field)
        + usize::from(layout.has_wpa_akm_field)
        + usize::from(layout.has_wpa_cipher_field)
        + usize::from(layout.has_retry_field)
        + usize::from(layout.has_power_save_field)
        + usize::from(layout.has_qos_priority_field)
        + usize::from(layout.has_status_code_field)
        + usize::from(layout.has_reason_code_field)
        + usize::from(layout.has_listen_interval_field)
        + usize::from(layout.has_pmkid_count_field)
        + usize::from(layout.has_ip_src_field)
        + usize::from(layout.has_ip_dst_field)
        + usize::from(layout.has_ipv6_src_field)
        + usize::from(layout.has_ipv6_dst_field)
        + usize::from(layout.has_tcp_srcport_field)
        + usize::from(layout.has_tcp_dstport_field)
        + usize::from(layout.has_udp_srcport_field)
        + usize::from(layout.has_udp_dstport_field)
        + usize::from(layout.has_arp_src_ipv4_field)
        + usize::from(layout.has_arp_dst_ipv4_field)
        + usize::from(layout.has_dns_qry_name_field)
        + usize::from(layout.has_dns_resp_name_field)
        + usize::from(layout.has_dns_a_field)
        + usize::from(layout.has_dns_aaaa_field)
        + usize::from(layout.has_dhcp_hostname_field)
        + usize::from(layout.has_dhcp_fqdn_field)
        + usize::from(layout.has_dhcp_vendor_class_field)
        + usize::from(layout.has_dhcp_requested_ip_field)
        + usize::from(layout.has_dhcp_client_ip_field)
        + usize::from(layout.has_dhcp_your_ip_field);
    if fields.len() < required {
        return None;
    }

    let mut i = 0usize;
    let epoch = parse_opt_f64(fields.get(i).copied().unwrap_or(""));
    i += 1;
    let frame_len = parse_opt_u32(fields.get(i).copied().unwrap_or(""));
    i += 1;
    let bssid = parse_opt_string(fields.get(i).copied().unwrap_or(""));
    i += 1;
    let sa = parse_opt_string(fields.get(i).copied().unwrap_or(""));
    i += 1;
    let da = parse_opt_string(fields.get(i).copied().unwrap_or(""));
    i += 1;

    let ssid = if layout.has_ssid_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };

    let rssi = parse_opt_i32(fields.get(i).copied().unwrap_or(""));
    i += 1;
    let channel = parse_opt_u16(fields.get(i).copied().unwrap_or(""));
    i += 1;
    let frequency = parse_opt_u32(fields.get(i).copied().unwrap_or(""));
    i += 1;
    let fc_type = parse_opt_u8(fields.get(i).copied().unwrap_or(""));
    i += 1;
    let subtype = parse_opt_u8(fields.get(i).copied().unwrap_or(""));
    i += 1;

    let eapol_msg = if layout.has_eapol_msg_field {
        let v = parse_opt_u8(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let rsn_version = if layout.has_rsn_version_field {
        let v = parse_opt_u8(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };

    let protected = matches!(fields.get(i).copied().unwrap_or("").trim(), "1" | "true");
    i += 1;

    let country_code = if layout.has_country_field {
        let raw = fields.get(i).copied().unwrap_or("");
        i += 1;
        parse_opt_string(raw).map(|v| v.trim().to_string())
    } else {
        None
    };
    let beacon_tsf_us = if layout.has_beacon_tsf_field {
        let v = parse_opt_u64(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let capability_privacy = if layout.has_privacy_field {
        let v = parse_opt_bool(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let rsn_akm_type = if layout.has_rsn_akm_field {
        let v = parse_opt_u8(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let rsn_cipher_type = if layout.has_rsn_cipher_field {
        let v = parse_opt_u8(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let rsn_mfpc = if layout.has_rsn_mfpc_field {
        let v = parse_opt_bool(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let rsn_mfpr = if layout.has_rsn_mfpr_field {
        let v = parse_opt_bool(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let wpa_version = if layout.has_wpa_version_field {
        let v = parse_opt_u8(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let wpa_akm_type = if layout.has_wpa_akm_field {
        let v = parse_opt_u8(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let wpa_cipher_type = if layout.has_wpa_cipher_field {
        parse_opt_u8(fields.get(i).copied().unwrap_or(""))
    } else {
        None
    };
    if layout.has_wpa_cipher_field {
        i += 1;
    }

    let retry = if layout.has_retry_field {
        let v = parse_opt_bool(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let power_save = if layout.has_power_save_field {
        let v = parse_opt_bool(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let qos_priority = if layout.has_qos_priority_field {
        let v = parse_opt_u8(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let status_code = if layout.has_status_code_field {
        let v = parse_opt_u16(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let reason_code = if layout.has_reason_code_field {
        let v = parse_opt_u16(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let listen_interval = if layout.has_listen_interval_field {
        let v = parse_opt_u16(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let pmkid_count = if layout.has_pmkid_count_field {
        let v = parse_opt_u16(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let ip_src = if layout.has_ip_src_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let ip_dst = if layout.has_ip_dst_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let ipv6_src = if layout.has_ipv6_src_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let ipv6_dst = if layout.has_ipv6_dst_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let tcp_srcport = if layout.has_tcp_srcport_field {
        let v = parse_opt_u16(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let tcp_dstport = if layout.has_tcp_dstport_field {
        let v = parse_opt_u16(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let udp_srcport = if layout.has_udp_srcport_field {
        let v = parse_opt_u16(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let udp_dstport = if layout.has_udp_dstport_field {
        let v = parse_opt_u16(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let arp_src_ipv4 = if layout.has_arp_src_ipv4_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let arp_dst_ipv4 = if layout.has_arp_dst_ipv4_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let dns_qry_name = if layout.has_dns_qry_name_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let dns_resp_name = if layout.has_dns_resp_name_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let dns_a = if layout.has_dns_a_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let dns_aaaa = if layout.has_dns_aaaa_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let dhcp_hostname = if layout.has_dhcp_hostname_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let dhcp_fqdn = if layout.has_dhcp_fqdn_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let dhcp_vendor_class_id = if layout.has_dhcp_vendor_class_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let dhcp_requested_ip = if layout.has_dhcp_requested_ip_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let dhcp_client_ip = if layout.has_dhcp_client_ip_field {
        let v = parse_opt_string(fields.get(i).copied().unwrap_or(""));
        i += 1;
        v
    } else {
        None
    };
    let dhcp_your_ip = if layout.has_dhcp_your_ip_field {
        parse_opt_string(fields.get(i).copied().unwrap_or(""))
    } else {
        None
    };

    Some(ParsedFrame {
        epoch,
        frame_len,
        bssid,
        sa,
        da,
        ssid,
        rssi,
        channel,
        frequency,
        fc_type,
        subtype,
        eapol_msg,
        rsn_version,
        protected,
        country_code,
        beacon_tsf_us,
        capability_privacy,
        rsn_akm_type,
        rsn_cipher_type,
        rsn_mfpc,
        rsn_mfpr,
        wpa_version,
        wpa_akm_type,
        wpa_cipher_type,
        retry,
        power_save,
        qos_priority,
        status_code,
        reason_code,
        listen_interval,
        pmkid_count,
        ip_src,
        ip_dst,
        ipv6_src,
        ipv6_dst,
        tcp_srcport,
        tcp_dstport,
        udp_srcport,
        udp_dstport,
        arp_src_ipv4,
        arp_dst_ipv4,
        dns_qry_name,
        dns_resp_name,
        dns_a,
        dns_aaaa,
        dhcp_hostname,
        dhcp_fqdn,
        dhcp_vendor_class_id,
        dhcp_requested_ip,
        dhcp_client_ip,
        dhcp_your_ip,
    })
}

fn parse_opt_string(raw: &str) -> Option<String> {
    let v = raw.trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

fn parse_opt_u8(raw: &str) -> Option<u8> {
    raw.trim().parse::<u8>().ok()
}

fn parse_opt_u16(raw: &str) -> Option<u16> {
    raw.trim().parse::<u16>().ok()
}

fn parse_opt_u32(raw: &str) -> Option<u32> {
    raw.trim().parse::<u32>().ok()
}

fn parse_opt_u64(raw: &str) -> Option<u64> {
    raw.trim().parse::<u64>().ok()
}

fn parse_opt_i32(raw: &str) -> Option<i32> {
    raw.trim().parse::<i32>().ok()
}

fn parse_opt_f64(raw: &str) -> Option<f64> {
    raw.trim().parse::<f64>().ok()
}

fn parse_opt_bool(raw: &str) -> Option<bool> {
    match raw.trim() {
        "1" | "true" | "TRUE" => Some(true),
        "0" | "false" | "FALSE" => Some(false),
        _ => None,
    }
}

fn normalize_mac(value: String) -> String {
    value
        .split(':')
        .filter(|part| !part.is_empty())
        .map(|part| format!("{:0>2}", part.to_uppercase()))
        .collect::<Vec<_>>()
        .join(":")
}

fn is_broadcast_mac(value: &str) -> bool {
    value.eq_ignore_ascii_case("FF:FF:FF:FF:FF:FF")
}

fn classify_ap_encryption(frame: &ParsedFrame) -> Option<(String, String)> {
    let has_rsn = frame.rsn_version.is_some()
        || frame.rsn_akm_type.is_some()
        || frame.rsn_cipher_type.is_some();
    let has_wpa = frame.wpa_version.is_some()
        || frame.wpa_akm_type.is_some()
        || frame.wpa_cipher_type.is_some();
    let privacy = frame.capability_privacy.or_else(|| {
        if has_rsn || has_wpa || frame.protected {
            Some(true)
        } else {
            None
        }
    });

    if !has_rsn && !has_wpa {
        return match privacy {
            Some(false) => Some(("Open".to_string(), "Open".to_string())),
            Some(true) if frame.protected => Some((
                "Protected".to_string(),
                "Protected (encrypted data observed, no RSN/WPA IE decoded)".to_string(),
            )),
            Some(true) => Some((
                "WEP".to_string(),
                "WEP / privacy bit set with no RSN or WPA information element".to_string(),
            )),
            None => None,
        };
    }

    let mut short_parts = Vec::new();
    if has_wpa {
        short_parts.push("WPA".to_string());
    }
    if has_rsn {
        short_parts.push(rsn_security_label(frame.rsn_akm_type).to_string());
    }
    short_parts.dedup();
    let short = if short_parts.is_empty() {
        "Protected".to_string()
    } else {
        short_parts.join("/")
    };

    let mut full_parts = vec![short.clone()];
    let mut akms = Vec::new();
    if let Some(label) = wpa_akm_label(frame.wpa_akm_type) {
        akms.push(label.to_string());
    }
    if let Some(label) = rsn_akm_label(frame.rsn_akm_type) {
        akms.push(label.to_string());
    }
    akms.sort();
    akms.dedup();
    if !akms.is_empty() {
        full_parts.push(format!("AKM {}", akms.join("/")));
    }

    let mut ciphers = Vec::new();
    if let Some(label) = cipher_label(frame.wpa_cipher_type) {
        ciphers.push(label.to_string());
    }
    if let Some(label) = cipher_label(frame.rsn_cipher_type) {
        ciphers.push(label.to_string());
    }
    ciphers.sort();
    ciphers.dedup();
    if !ciphers.is_empty() {
        full_parts.push(format!("Cipher {}", ciphers.join("/")));
    }

    match (
        frame.rsn_mfpc.unwrap_or(false),
        frame.rsn_mfpr.unwrap_or(false),
    ) {
        (_, true) => full_parts.push("PMF required".to_string()),
        (true, false) => full_parts.push("PMF capable".to_string()),
        _ => {}
    }

    Some((short, full_parts.join(" - ")))
}

fn rsn_security_label(akm_type: Option<u8>) -> &'static str {
    match akm_type {
        Some(8) | Some(9) | Some(24) => "WPA3",
        Some(18) => "OWE",
        _ => "WPA2",
    }
}

fn rsn_akm_label(akm_type: Option<u8>) -> Option<&'static str> {
    match akm_type? {
        1 => Some("802.1X"),
        2 => Some("PSK"),
        3 => Some("FT-802.1X"),
        4 => Some("FT-PSK"),
        5 => Some("802.1X-SHA256"),
        6 => Some("PSK-SHA256"),
        8 => Some("SAE"),
        9 => Some("FT-SAE"),
        11 => Some("Suite-B-802.1X"),
        12 => Some("Suite-B-192"),
        18 => Some("OWE"),
        24 => Some("SAE-H2E"),
        _ => Some("Unknown-RSN-AKM"),
    }
}

fn wpa_akm_label(akm_type: Option<u8>) -> Option<&'static str> {
    match akm_type? {
        1 => Some("802.1X"),
        2 => Some("PSK"),
        _ => Some("Unknown-WPA-AKM"),
    }
}

fn cipher_label(cipher_type: Option<u8>) -> Option<&'static str> {
    match cipher_type? {
        1 => Some("WEP40"),
        2 => Some("TKIP"),
        4 => Some("CCMP"),
        5 => Some("WEP104"),
        8 => Some("GCMP"),
        9 => Some("GCMP-256"),
        10 => Some("CCMP-256"),
        11 => Some("BIP-GMAC-128"),
        12 => Some("BIP-GMAC-256"),
        13 => Some("BIP-CMAC-256"),
        _ => Some("Unknown-Cipher"),
    }
}

fn infer_client_mac(bssid: &str, sa: &Option<String>, da: &Option<String>) -> Option<String> {
    let sa = sa.as_ref().map(|v| normalize_mac(v.clone()));
    let da = da.as_ref().map(|v| normalize_mac(v.clone()));

    let candidate = match (sa, da) {
        (Some(sa), Some(da)) if sa == bssid && !is_broadcast_mac(&da) => Some(da),
        (Some(sa), Some(da)) if da == bssid && !is_broadcast_mac(&sa) => Some(sa),
        (Some(sa), _) if !is_broadcast_mac(&sa) && sa != bssid => Some(sa),
        (_, Some(da)) if !is_broadcast_mac(&da) && da != bssid => Some(da),
        _ => None,
    };

    candidate.filter(|m| m.len() >= 17)
}

fn tshark_supports_field(field_name: &str) -> bool {
    TSHARK_FIELDS.contains(field_name)
}

fn tshark_pick_supported_field(candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|f| tshark_supports_field(f))
        .map(|f| (*f).to_string())
}

pub fn rssi_to_tone_hz(rssi_dbm: i32) -> u32 {
    let clamped = rssi_dbm.clamp(-100, -30);
    let normalized = (clamped + 100) as f32 / 70.0;
    let hz = 120.0 + normalized * (2300.0 - 120.0);
    hz.round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_network_activity_updates_client_network_intel() {
        let now = Utc::now();
        let mut client = ClientRecord::new("AA:BB:CC:DD:EE:FF", now);
        let mut dns_ip_map = HashMap::new();
        dns_ip_map.insert("93.184.216.34".to_string(), "example.com".to_string());
        let mut geo_lookup = GeoIpLookup::new();
        let frame = ParsedFrame {
            epoch: None,
            frame_len: Some(128),
            bssid: Some("11:22:33:44:55:66".to_string()),
            sa: Some(client.mac.clone()),
            da: Some("11:22:33:44:55:66".to_string()),
            ssid: None,
            rssi: Some(-42),
            channel: Some(6),
            frequency: Some(2437),
            fc_type: Some(2),
            subtype: Some(0),
            eapol_msg: None,
            rsn_version: None,
            protected: false,
            country_code: None,
            beacon_tsf_us: None,
            capability_privacy: None,
            rsn_akm_type: None,
            rsn_cipher_type: None,
            rsn_mfpc: None,
            rsn_mfpr: None,
            wpa_version: None,
            wpa_akm_type: None,
            wpa_cipher_type: None,
            retry: Some(true),
            power_save: Some(true),
            qos_priority: Some(5),
            status_code: None,
            reason_code: None,
            listen_interval: Some(10),
            pmkid_count: Some(1),
            ip_src: Some("192.168.1.25".to_string()),
            ip_dst: Some("93.184.216.34".to_string()),
            ipv6_src: None,
            ipv6_dst: None,
            tcp_srcport: Some(49152),
            tcp_dstport: Some(443),
            udp_srcport: None,
            udp_dstport: None,
            arp_src_ipv4: None,
            arp_dst_ipv4: None,
            dns_qry_name: Some("example.com".to_string()),
            dns_resp_name: None,
            dns_a: None,
            dns_aaaa: None,
            dhcp_hostname: Some("phone".to_string()),
            dhcp_fqdn: Some("phone.lan".to_string()),
            dhcp_vendor_class_id: Some("android-dhcp-13".to_string()),
            dhcp_requested_ip: Some("192.168.1.25".to_string()),
            dhcp_client_ip: None,
            dhcp_your_ip: None,
        };

        observe_open_network_activity(
            &mut client,
            ClientTrafficDirection::Uplink,
            &frame,
            &dns_ip_map,
            &mut geo_lookup,
            now,
        );

        assert!(client
            .network_intel
            .local_ipv4_addresses
            .contains(&"192.168.1.25".to_string()));
        assert!(client
            .network_intel
            .dhcp_hostnames
            .contains(&"phone".to_string()));
        assert!(client
            .network_intel
            .dns_names
            .contains(&"example.com".to_string()));
        assert_eq!(client.network_intel.remote_endpoints.len(), 1);
        let endpoint = &client.network_intel.remote_endpoints[0];
        assert_eq!(endpoint.ip_address, "93.184.216.34");
        assert_eq!(endpoint.port, Some(443));
        assert_eq!(endpoint.domain.as_deref(), Some("example.com"));
    }
}

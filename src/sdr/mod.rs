use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use crossbeam_channel::{unbounded, Receiver, Sender};
use rand::Rng;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
#[cfg(target_family = "unix")]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_FFT_BINS: usize = 256;
const DEFAULT_REFRESH_MS: u64 = 260;
const MAX_DECODE_MESSAGE_LEN: usize = 2048;
const DEFAULT_SCAN_STEP_HZ: u64 = 25_000;
const DEFAULT_SCAN_STEPS_PER_SEC: f64 = 4.0;
const DEFAULT_SQUELCH_DBM: f32 = -78.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SdrHardware {
    RtlSdr,
    HackRf,
    BladeRf,
    EttusB210,
}

impl Default for SdrHardware {
    fn default() -> Self {
        Self::RtlSdr
    }
}

impl SdrHardware {
    pub fn label(&self) -> &'static str {
        match self {
            Self::RtlSdr => "RTL-SDR",
            Self::HackRf => "HackRF",
            Self::BladeRf => "bladeRF",
            Self::EttusB210 => "Ettus B210",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Self::RtlSdr => "rtl_sdr",
            Self::HackRf => "hackrf",
            Self::BladeRf => "bladerf",
            Self::EttusB210 => "ettus_b210",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SdrConfig {
    pub hardware: SdrHardware,
    pub center_freq_hz: u64,
    pub sample_rate_hz: u32,
    pub fft_bins: usize,
    pub refresh_ms: u64,
    pub log_output_enabled: bool,
    pub log_output_dir: PathBuf,
    pub plugin_config_path: Option<PathBuf>,
    pub scan_range_enabled: bool,
    pub scan_start_hz: u64,
    pub scan_end_hz: u64,
    pub scan_step_hz: u64,
    pub scan_steps_per_sec: f64,
    pub squelch_dbm: f32,
    pub auto_tune_decoders: bool,
    pub bias_tee_enabled: bool,
    pub no_payload_satcom: bool,
}

impl Default for SdrConfig {
    fn default() -> Self {
        Self {
            hardware: SdrHardware::RtlSdr,
            center_freq_hz: 433_920_000,
            sample_rate_hz: 2_400_000,
            fft_bins: DEFAULT_FFT_BINS,
            refresh_ms: DEFAULT_REFRESH_MS,
            log_output_enabled: false,
            log_output_dir: std::env::temp_dir().join("wirelessexplorer-sdr-logs"),
            plugin_config_path: default_plugin_config_path(),
            scan_range_enabled: false,
            scan_start_hz: 118_000_000,
            scan_end_hz: 137_000_000,
            scan_step_hz: DEFAULT_SCAN_STEP_HZ,
            scan_steps_per_sec: DEFAULT_SCAN_STEPS_PER_SEC,
            squelch_dbm: DEFAULT_SQUELCH_DBM,
            auto_tune_decoders: true,
            bias_tee_enabled: false,
            no_payload_satcom: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SdrSpectrumFrame {
    pub timestamp: DateTime<Utc>,
    pub center_freq_hz: u64,
    pub sample_rate_hz: u32,
    pub bins_db: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SdrDecodeRow {
    pub timestamp: DateTime<Utc>,
    pub decoder: String,
    pub freq_hz: u64,
    pub protocol: String,
    pub message: String,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SdrMapPoint {
    pub timestamp: DateTime<Utc>,
    pub decoder: String,
    pub protocol: String,
    pub freq_hz: u64,
    pub latitude: f64,
    pub longitude: f64,
    pub altitude_m: Option<f64>,
    pub label: String,
    pub message: String,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SdrSatcomObservation {
    pub timestamp: DateTime<Utc>,
    pub decoder: String,
    pub protocol: String,
    pub freq_hz: u64,
    pub band: String,
    pub encryption_posture: String,
    pub has_coordinates: bool,
    pub identifier_hints: Vec<String>,
    pub summary: String,
    pub message: String,
    pub raw: String,
}

#[derive(Debug, Clone)]
pub struct SdrDependencyStatus {
    pub tool: String,
    pub package_hint: String,
    pub installed: bool,
}

#[derive(Debug, Clone)]
pub enum SdrEvent {
    Log(String),
    FrequencyChanged(u64),
    SpectrumFrame(SdrSpectrumFrame),
    DecodeRow(SdrDecodeRow),
    DecoderState {
        running: bool,
        decoder: Option<String>,
    },
    DependencyStatus(Vec<SdrDependencyStatus>),
    MapPoint(SdrMapPoint),
    SatcomObservation(SdrSatcomObservation),
    SquelchChanged(f32),
}

#[derive(Debug, Clone)]
pub enum SdrDecoderKind {
    Rtl433,
    Adsb,
    Acars,
    Ais,
    Pocsag,
    Iridium,
    Dect,
    GsmLte,
    Plugin {
        id: String,
        label: String,
        command_template: String,
        protocol: Option<String>,
    },
}

impl SdrDecoderKind {
    pub fn id(&self) -> String {
        match self {
            Self::Rtl433 => "rtl_433".to_string(),
            Self::Adsb => "ads_b".to_string(),
            Self::Acars => "acars".to_string(),
            Self::Ais => "ais".to_string(),
            Self::Pocsag => "pocsag".to_string(),
            Self::Iridium => "iridium".to_string(),
            Self::Dect => "dect".to_string(),
            Self::GsmLte => "gsm_lte".to_string(),
            Self::Plugin { id, .. } => id.clone(),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Rtl433 => "rtl_433".to_string(),
            Self::Adsb => "ADS-B".to_string(),
            Self::Acars => "ACARS".to_string(),
            Self::Ais => "AIS".to_string(),
            Self::Pocsag => "POCSAG".to_string(),
            Self::Iridium => "Iridium".to_string(),
            Self::Dect => "DECT".to_string(),
            Self::GsmLte => "GSM/LTE Metadata".to_string(),
            Self::Plugin { label, .. } => label.clone(),
        }
    }

    pub fn default_protocol(&self) -> &'static str {
        match self {
            Self::Rtl433 => "rtl_433",
            Self::Adsb => "adsb",
            Self::Acars => "acars",
            Self::Ais => "ais",
            Self::Pocsag => "pocsag",
            Self::Iridium => "iridium",
            Self::Dect => "dect",
            Self::GsmLte => "gsm_lte",
            Self::Plugin { .. } => "plugin",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SdrPluginDefinition {
    pub id: String,
    pub label: String,
    pub command_template: String,
    pub protocol: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PluginConfigFile {
    #[serde(default)]
    plugins: Vec<PluginConfigEntry>,
}

#[derive(Debug, Deserialize)]
struct PluginConfigEntry {
    id: String,
    label: String,
    command: String,
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default = "default_plugin_enabled")]
    enabled: bool,
}

fn default_plugin_enabled() -> bool {
    true
}

#[derive(Debug, Clone)]
enum SdrCommand {
    SetCenterFreq(u64),
    SetSweepPaused(bool),
    SetLogging {
        enabled: bool,
        output_dir: PathBuf,
    },
    SetScanRange {
        enabled: bool,
        start_hz: u64,
        end_hz: u64,
        step_hz: u64,
        steps_per_sec: f64,
    },
    SetSquelch(f32),
    SetAutoTune(bool),
    SetBiasTee(bool),
    SetNoPayloadSatcom(bool),
    CaptureSample {
        duration_secs: u32,
        output_dir: PathBuf,
    },
    StartDecode(SdrDecoderKind),
    StopDecode,
    RefreshDependencies,
    InstallMissingDependencies,
    Shutdown,
}

pub struct SdrRuntime {
    stop_flag: Arc<AtomicBool>,
    command_tx: Sender<SdrCommand>,
    handle: Option<thread::JoinHandle<()>>,
}

impl SdrRuntime {
    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        let _ = self.command_tx.send(SdrCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    pub fn set_center_freq(&self, freq_hz: u64) {
        let _ = self.command_tx.send(SdrCommand::SetCenterFreq(freq_hz));
    }

    pub fn set_sweep_paused(&self, paused: bool) {
        let _ = self.command_tx.send(SdrCommand::SetSweepPaused(paused));
    }

    pub fn set_logging(&self, enabled: bool, output_dir: PathBuf) {
        let _ = self.command_tx.send(SdrCommand::SetLogging {
            enabled,
            output_dir,
        });
    }

    pub fn set_scan_range(
        &self,
        enabled: bool,
        start_hz: u64,
        end_hz: u64,
        step_hz: u64,
        steps_per_sec: f64,
    ) {
        let _ = self.command_tx.send(SdrCommand::SetScanRange {
            enabled,
            start_hz,
            end_hz,
            step_hz,
            steps_per_sec,
        });
    }

    pub fn set_squelch(&self, squelch_dbm: f32) {
        let _ = self.command_tx.send(SdrCommand::SetSquelch(squelch_dbm));
    }

    pub fn set_auto_tune(&self, enabled: bool) {
        let _ = self.command_tx.send(SdrCommand::SetAutoTune(enabled));
    }

    pub fn set_bias_tee(&self, enabled: bool) {
        let _ = self.command_tx.send(SdrCommand::SetBiasTee(enabled));
    }

    pub fn set_no_payload_satcom(&self, enabled: bool) {
        let _ = self
            .command_tx
            .send(SdrCommand::SetNoPayloadSatcom(enabled));
    }

    pub fn capture_sample(&self, duration_secs: u32, output_dir: PathBuf) {
        let _ = self.command_tx.send(SdrCommand::CaptureSample {
            duration_secs,
            output_dir,
        });
    }

    pub fn start_decode(&self, decoder: SdrDecoderKind) {
        let _ = self.command_tx.send(SdrCommand::StartDecode(decoder));
    }

    pub fn stop_decode(&self) {
        let _ = self.command_tx.send(SdrCommand::StopDecode);
    }

    pub fn refresh_dependencies(&self) {
        let _ = self.command_tx.send(SdrCommand::RefreshDependencies);
    }

    pub fn install_missing_dependencies(&self) {
        let _ = self.command_tx.send(SdrCommand::InstallMissingDependencies);
    }
}

struct RunningDecoder {
    name: String,
    child: Child,
    stop: Arc<AtomicBool>,
    stdout_handle: Option<thread::JoinHandle<()>>,
    stderr_handle: Option<thread::JoinHandle<()>>,
}

impl RunningDecoder {
    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        #[cfg(target_family = "unix")]
        {
            let pid = self.child.id() as i32;
            if pid > 0 {
                unsafe {
                    libc::kill(-pid, libc::SIGTERM);
                }
                thread::sleep(Duration::from_millis(150));
                if self.child.try_wait().ok().flatten().is_none() {
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.stdout_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_handle.take() {
            let _ = handle.join();
        }
    }
}

pub fn default_plugin_config_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("WIRELESSEXPLORER_SDR_PLUGINS") {
        let path = path.trim();
        if !path.is_empty() {
            candidates.push(PathBuf::from(path));
        }
    }

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(root.join("sdr-plugins.json"));

    if let Some(config_dir) = dirs::config_dir() {
        candidates.push(config_dir.join("WirelessExplorer/sdr-plugins.json"));
        candidates.push(config_dir.join("gqrx/sdr-plugins.json"));
    }

    candidates.push(PathBuf::from(
        "/usr/share/wirelessexplorer/sdr-plugins.json",
    ));
    candidates.into_iter().find(|path| path.exists())
}

pub fn load_plugin_definitions(path: Option<&Path>) -> Vec<SdrPluginDefinition> {
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(config) = serde_json::from_str::<PluginConfigFile>(&raw) else {
        return Vec::new();
    };

    config
        .plugins
        .into_iter()
        .filter(|entry| entry.enabled)
        .filter(|entry| !entry.id.trim().is_empty() && !entry.command.trim().is_empty())
        .map(|entry| SdrPluginDefinition {
            id: entry.id.trim().to_string(),
            label: entry.label.trim().to_string(),
            command_template: entry.command.trim().to_string(),
            protocol: entry
                .protocol
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        })
        .collect()
}

pub fn builtin_decoders_in_priority_order() -> Vec<SdrDecoderKind> {
    vec![
        SdrDecoderKind::Rtl433,
        SdrDecoderKind::Adsb,
        SdrDecoderKind::Acars,
        SdrDecoderKind::Ais,
        SdrDecoderKind::Pocsag,
        SdrDecoderKind::Iridium,
        SdrDecoderKind::Dect,
        SdrDecoderKind::GsmLte,
    ]
}

pub fn start_runtime(config: SdrConfig, sender: Sender<SdrEvent>) -> SdrRuntime {
    let (command_tx, command_rx) = unbounded::<SdrCommand>();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&stop_flag);

    let handle = thread::spawn(move || {
        run_sdr_loop(config, sender, command_rx, stop);
    });

    SdrRuntime {
        stop_flag,
        command_tx,
        handle: Some(handle),
    }
}

fn run_sdr_loop(
    config: SdrConfig,
    sender: Sender<SdrEvent>,
    command_rx: Receiver<SdrCommand>,
    stop_flag: Arc<AtomicBool>,
) {
    let mut center_freq_hz = config.center_freq_hz;
    let sample_rate_hz = config.sample_rate_hz.max(200_000);
    let fft_bins = config.fft_bins.max(64);
    let refresh_ms = config.refresh_ms.max(100);
    let hardware = config.hardware;
    let mut sweep_paused = false;
    let mut running_decoder: Option<RunningDecoder> = None;
    let mut restore_center_after_decode: Option<u64> = None;
    let mut log_output_enabled = config.log_output_enabled;
    let mut log_output_dir = config.log_output_dir;
    let mut scan_range_enabled = config.scan_range_enabled;
    let (initial_scan_start_hz, initial_scan_end_hz, initial_scan_step_hz) = normalize_scan_range(
        config.scan_start_hz,
        config.scan_end_hz,
        config.scan_step_hz,
    );
    let mut scan_start_hz = initial_scan_start_hz;
    let mut scan_end_hz = initial_scan_end_hz;
    let mut scan_step_hz = initial_scan_step_hz;
    let mut scan_steps_per_sec = config.scan_steps_per_sec.max(0.1);
    let mut scan_next_hz = initial_scan_start_hz;
    let mut squelch_dbm = config.squelch_dbm.clamp(-130.0, -10.0);
    let mut auto_tune_decoders = config.auto_tune_decoders;
    let mut bias_tee_enabled = config.bias_tee_enabled;
    let mut no_payload_satcom = config.no_payload_satcom;
    let plugin_defs = load_plugin_definitions(config.plugin_config_path.as_deref());

    let _ = sender.send(SdrEvent::Log(format!(
        "SDR runtime started: source={} center={} Hz sample_rate={} Hz",
        hardware.label(),
        center_freq_hz,
        sample_rate_hz
    )));
    let _ = sender.send(SdrEvent::Log(format!(
        "scan={} start={} end={} step={} Hz speed={:.2} steps/s",
        if scan_range_enabled { "on" } else { "off" },
        scan_start_hz,
        scan_end_hz,
        scan_step_hz,
        scan_steps_per_sec
    )));
    let _ = sender.send(SdrEvent::Log(format!(
        "squelch={:.1} dBm auto_tune={} bias_tee={} satcom_payload_capture={}",
        squelch_dbm,
        if auto_tune_decoders { "on" } else { "off" },
        if bias_tee_enabled { "on" } else { "off" },
        if no_payload_satcom {
            "disabled"
        } else {
            "enabled"
        }
    )));
    let _ = sender.send(SdrEvent::FrequencyChanged(center_freq_hz));
    let _ = sender.send(SdrEvent::SquelchChanged(squelch_dbm));
    let _ = sender.send(SdrEvent::DependencyStatus(check_dependencies_for_plugins(
        &plugin_defs,
    )));
    let _ = try_set_bias_tee(hardware, bias_tee_enabled, &sender);

    let mut last_frame_at = Instant::now() - Duration::from_millis(refresh_ms);
    let mut last_scan_step_at = Instant::now();

    while !stop_flag.load(Ordering::Relaxed) {
        while let Ok(command) = command_rx.try_recv() {
            match command {
                SdrCommand::SetCenterFreq(freq_hz) => {
                    center_freq_hz = freq_hz.max(100_000);
                    let _ = sender.send(SdrEvent::FrequencyChanged(center_freq_hz));
                }
                SdrCommand::SetSweepPaused(paused) => {
                    sweep_paused = paused;
                    let _ = sender.send(SdrEvent::Log(if paused {
                        "SDR sweep paused".to_string()
                    } else {
                        "SDR sweep resumed".to_string()
                    }));
                }
                SdrCommand::SetLogging {
                    enabled,
                    output_dir,
                } => {
                    log_output_enabled = enabled;
                    log_output_dir = output_dir;
                    let _ = sender.send(SdrEvent::Log(if log_output_enabled {
                        format!(
                            "SDR decoder log output enabled: {}",
                            log_output_dir.display()
                        )
                    } else {
                        "SDR decoder log output disabled".to_string()
                    }));
                }
                SdrCommand::SetScanRange {
                    enabled,
                    start_hz,
                    end_hz,
                    step_hz,
                    steps_per_sec,
                } => {
                    let (normalized_start, normalized_end, normalized_step) =
                        normalize_scan_range(start_hz, end_hz, step_hz);
                    scan_range_enabled = enabled;
                    scan_start_hz = normalized_start;
                    scan_end_hz = normalized_end;
                    scan_step_hz = normalized_step;
                    scan_steps_per_sec = steps_per_sec.max(0.1);
                    scan_next_hz = scan_start_hz;
                    last_scan_step_at = Instant::now();
                    if scan_range_enabled {
                        center_freq_hz = scan_start_hz;
                        let _ = sender.send(SdrEvent::FrequencyChanged(center_freq_hz));
                    }
                    let _ = sender.send(SdrEvent::Log(format!(
                        "scan {} ({}..{} Hz step={} Hz speed={:.2} steps/s)",
                        if scan_range_enabled {
                            "enabled"
                        } else {
                            "disabled"
                        },
                        scan_start_hz,
                        scan_end_hz,
                        scan_step_hz,
                        scan_steps_per_sec
                    )));
                }
                SdrCommand::SetSquelch(new_squelch_dbm) => {
                    squelch_dbm = new_squelch_dbm.clamp(-130.0, -10.0);
                    let _ = sender.send(SdrEvent::SquelchChanged(squelch_dbm));
                    let _ = sender.send(SdrEvent::Log(format!(
                        "squelch set to {:.1} dBm",
                        squelch_dbm
                    )));
                }
                SdrCommand::SetAutoTune(enabled) => {
                    auto_tune_decoders = enabled;
                    let _ = sender.send(SdrEvent::Log(format!(
                        "decoder auto-tune {}",
                        if enabled { "enabled" } else { "disabled" }
                    )));
                }
                SdrCommand::SetBiasTee(enabled) => {
                    bias_tee_enabled = enabled;
                    let _ = try_set_bias_tee(hardware, bias_tee_enabled, &sender);
                }
                SdrCommand::SetNoPayloadSatcom(enabled) => {
                    no_payload_satcom = enabled;
                    let _ = sender.send(SdrEvent::Log(format!(
                        "satcom payload capture {}",
                        if enabled { "disabled" } else { "enabled" }
                    )));
                }
                SdrCommand::CaptureSample {
                    duration_secs,
                    output_dir,
                } => {
                    let _ = capture_iq_sample(
                        hardware,
                        center_freq_hz,
                        sample_rate_hz,
                        duration_secs,
                        output_dir,
                        &sender,
                    );
                }
                SdrCommand::StartDecode(kind) => {
                    if running_decoder.is_none() {
                        restore_center_after_decode = Some(center_freq_hz);
                    }
                    if let Some(decoder) = running_decoder.take() {
                        decoder.stop();
                    }

                    let decode_freq = if auto_tune_decoders {
                        decoder_autotune_frequency_hz(&kind).unwrap_or(center_freq_hz)
                    } else {
                        center_freq_hz
                    };
                    center_freq_hz = decode_freq;
                    let _ = sender.send(SdrEvent::FrequencyChanged(center_freq_hz));
                    if let SdrDecoderKind::Plugin { .. } = &kind {
                        // plugin command may define its own center frequency usage
                    }

                    let command_line = resolve_decoder_command_line(
                        &kind,
                        decode_freq,
                        sample_rate_hz,
                        hardware,
                        &plugin_defs,
                    );
                    let Some(command_line) = command_line else {
                        let reason = decoder_unavailability_reason(&kind, hardware)
                            .unwrap_or_else(|| "required tool not found".to_string());
                        let _ = sender.send(SdrEvent::Log(format!(
                            "decoder {} unavailable: {}",
                            kind.label(),
                            reason
                        )));
                        continue;
                    };

                    sweep_paused = true;
                    let _ = sender.send(SdrEvent::DecoderState {
                        running: true,
                        decoder: Some(kind.label()),
                    });
                    let _ = sender.send(SdrEvent::Log(format!(
                        "starting decoder {} at {} Hz",
                        kind.label(),
                        decode_freq
                    )));

                    match spawn_decoder(
                        kind,
                        decode_freq,
                        command_line,
                        sender.clone(),
                        log_output_enabled,
                        log_output_dir.clone(),
                        no_payload_satcom,
                    ) {
                        Ok(decoder) => {
                            running_decoder = Some(decoder);
                        }
                        Err(err) => {
                            let _ = sender.send(SdrEvent::DecoderState {
                                running: false,
                                decoder: None,
                            });
                            let _ = sender
                                .send(SdrEvent::Log(format!("failed to start decoder: {err}")));
                            sweep_paused = false;
                        }
                    }
                }
                SdrCommand::StopDecode => {
                    if let Some(decoder) = running_decoder.take() {
                        let decoder_name = decoder.name.clone();
                        decoder.stop();
                        let _ = sender
                            .send(SdrEvent::Log(format!("decoder stopped: {}", decoder_name)));
                    }
                    let _ = sender.send(SdrEvent::DecoderState {
                        running: false,
                        decoder: None,
                    });
                    if let Some(previous_center) = restore_center_after_decode.take() {
                        center_freq_hz = previous_center;
                        let _ = sender.send(SdrEvent::FrequencyChanged(center_freq_hz));
                    }
                    sweep_paused = false;
                }
                SdrCommand::RefreshDependencies => {
                    let _ = sender.send(SdrEvent::DependencyStatus(
                        check_dependencies_for_plugins(&plugin_defs),
                    ));
                }
                SdrCommand::InstallMissingDependencies => {
                    let status_before = check_dependencies_for_plugins(&plugin_defs);
                    let missing = status_before
                        .iter()
                        .filter(|entry| !entry.installed)
                        .map(|entry| entry.package_hint.clone())
                        .collect::<Vec<_>>();
                    if missing.is_empty() {
                        let _ = sender.send(SdrEvent::Log(
                            "all SDR dependencies already installed".to_string(),
                        ));
                    } else {
                        let _ = sender.send(SdrEvent::Log(format!(
                            "installing missing SDR dependencies: {}",
                            missing.join(", ")
                        )));
                        match install_dependency_packages(&missing) {
                            Ok(failed_packages) => {
                                if failed_packages.is_empty() {
                                    let _ = sender.send(SdrEvent::Log(
                                        "dependency installation completed".to_string(),
                                    ));
                                } else {
                                    let _ = sender.send(SdrEvent::Log(format!(
                                        "dependency installation completed with warnings; failed: {}",
                                        failed_packages.join(", ")
                                    )));
                                }
                            }
                            Err(err) => {
                                let _ = sender.send(SdrEvent::Log(format!(
                                    "dependency installation failed: {err}"
                                )));
                            }
                        }
                    }
                    let _ = sender.send(SdrEvent::DependencyStatus(
                        check_dependencies_for_plugins(&plugin_defs),
                    ));
                }
                SdrCommand::Shutdown => {
                    stop_flag.store(true, Ordering::Relaxed);
                }
            }
        }

        if scan_range_enabled && !sweep_paused {
            let now = Instant::now();
            let scan_interval_ms = (1000.0 / scan_steps_per_sec).max(20.0) as u64;
            if now.saturating_duration_since(last_scan_step_at)
                >= Duration::from_millis(scan_interval_ms)
            {
                center_freq_hz = scan_next_hz;
                let _ = sender.send(SdrEvent::FrequencyChanged(center_freq_hz));
                let next = scan_next_hz.saturating_add(scan_step_hz);
                scan_next_hz = if next > scan_end_hz {
                    scan_start_hz
                } else {
                    next
                };
                last_scan_step_at = now;
            }
        }

        if let Some(decoder) = running_decoder.as_mut() {
            if decoder.stop.load(Ordering::Relaxed) {
                if let Some(stopped) = running_decoder.take() {
                    stopped.stop();
                }
                let _ = sender.send(SdrEvent::DecoderState {
                    running: false,
                    decoder: None,
                });
                sweep_paused = false;
            } else {
                match decoder.child.try_wait() {
                    Ok(Some(status)) => {
                        let decoder_name = decoder.name.clone();
                        if let Some(stopped) = running_decoder.take() {
                            stopped.stop();
                        }
                        let _ = sender.send(SdrEvent::DecoderState {
                            running: false,
                            decoder: None,
                        });
                        let _ = sender.send(SdrEvent::Log(format!(
                            "decoder exited ({}) with status {}",
                            decoder_name, status
                        )));
                        if let Some(previous_center) = restore_center_after_decode.take() {
                            center_freq_hz = previous_center;
                            let _ = sender.send(SdrEvent::FrequencyChanged(center_freq_hz));
                        }
                        sweep_paused = false;
                    }
                    Ok(None) => {}
                    Err(err) => {
                        let _ = sender
                            .send(SdrEvent::Log(format!("decoder status check failed: {err}")));
                    }
                }
            }
        }

        let now = Instant::now();
        if !sweep_paused
            && now.saturating_duration_since(last_frame_at) >= Duration::from_millis(refresh_ms)
        {
            let frame = generate_synthetic_spectrum_frame(
                center_freq_hz,
                sample_rate_hz,
                fft_bins,
                hardware,
                squelch_dbm,
            );
            let _ = sender.send(SdrEvent::SpectrumFrame(frame));
            last_frame_at = now;
        }

        thread::sleep(Duration::from_millis(20));
    }

    if let Some(decoder) = running_decoder.take() {
        decoder.stop();
    }
    let _ = sender.send(SdrEvent::DecoderState {
        running: false,
        decoder: None,
    });
    let _ = sender.send(SdrEvent::Log("SDR runtime stopped".to_string()));
}

fn generate_synthetic_spectrum_frame(
    center_freq_hz: u64,
    sample_rate_hz: u32,
    fft_bins: usize,
    hardware: SdrHardware,
    squelch_dbm: f32,
) -> SdrSpectrumFrame {
    let mut rng = rand::thread_rng();
    let mut bins = Vec::with_capacity(fft_bins);

    let peak_center = rng.gen_range(fft_bins / 4..(fft_bins * 3 / 4).max(fft_bins / 4 + 1));
    let peak_width = rng.gen_range(3..20);
    let baseline = match hardware {
        SdrHardware::RtlSdr => -90.0,
        SdrHardware::HackRf => -88.0,
        SdrHardware::BladeRf => -87.0,
        SdrHardware::EttusB210 => -85.0,
    };

    for i in 0..fft_bins {
        let distance = (i as i32 - peak_center as i32).unsigned_abs() as f32;
        let peak = (1.0 - (distance / peak_width as f32)).max(0.0) * rng.gen_range(8.0..26.0);
        let noise = rng.gen_range(-4.0..4.5);
        let mut value = (baseline + peak + noise) as f32;
        if value < squelch_dbm {
            value = squelch_dbm - rng.gen_range(3.0..10.0);
        }
        bins.push(value);
    }

    SdrSpectrumFrame {
        timestamp: Utc::now(),
        center_freq_hz,
        sample_rate_hz,
        bins_db: bins,
    }
}

fn resolve_decoder_command_line(
    decoder: &SdrDecoderKind,
    freq_hz: u64,
    sample_rate_hz: u32,
    hardware: SdrHardware,
    plugins: &[SdrPluginDefinition],
) -> Option<String> {
    let freq_mhz = (freq_hz as f64) / 1_000_000.0;
    let replacements = HashMap::from([
        ("{freq_hz}", freq_hz.to_string()),
        ("{freq_khz}", format!("{:.3}", (freq_hz as f64) / 1_000.0)),
        ("{freq_mhz}", format!("{freq_mhz:.6}")),
        ("{sample_rate_hz}", sample_rate_hz.to_string()),
        (
            "{sample_rate_khz}",
            format!("{:.3}", (sample_rate_hz as f64) / 1_000.0),
        ),
        ("{hardware}", hardware.id().to_string()),
    ]);

    let template = match decoder {
        SdrDecoderKind::Rtl433 => command_exists("rtl_433")
            .then_some("rtl_433 -f {freq_mhz}M -M level")
            .map(str::to_string),
        SdrDecoderKind::Adsb => {
            if command_exists("dump1090") {
                Some("dump1090 --net --quiet".to_string())
            } else if command_exists("dump1090-mutability") {
                Some("dump1090-mutability --net --quiet".to_string())
            } else if command_exists("readsb") {
                Some("readsb --quiet --net".to_string())
            } else {
                None
            }
        }
        SdrDecoderKind::Acars => {
            if command_exists("acarsdec") {
                resolve_acarsdec_command_line(freq_hz, hardware)
            } else {
                None
            }
        }
        SdrDecoderKind::Ais => {
            if command_exists("rtl_ais") {
                Some("rtl_ais".to_string())
            } else if command_exists("aisdecoder") {
                Some("aisdecoder".to_string())
            } else {
                None
            }
        }
        SdrDecoderKind::Pocsag => {
            if command_exists("rtl_fm") && command_exists("multimon-ng") {
                Some(
                    "rtl_fm -f {freq_hz} -M fm -s 22050 -g 35 - | multimon-ng -t raw -a POCSAG1200 -a POCSAG2400 -"
                        .to_string(),
                )
            } else {
                None
            }
        }
        SdrDecoderKind::Iridium => {
            if command_exists("iridium-extractor") {
                Some("iridium-extractor".to_string())
            } else {
                None
            }
        }
        SdrDecoderKind::Dect => {
            if command_exists("multimon-ng") && command_exists("rtl_fm") {
                Some(
                    "rtl_fm -f {freq_hz} -M fm -s 48000 -g 35 - | multimon-ng -t raw -a DECT -"
                        .to_string(),
                )
            } else {
                None
            }
        }
        SdrDecoderKind::GsmLte => {
            if command_exists("grgsm_livemon_headless") {
                Some("grgsm_livemon_headless".to_string())
            } else if command_exists("cell_search") {
                Some("cell_search -g 50".to_string())
            } else {
                None
            }
        }
        SdrDecoderKind::Plugin {
            id,
            command_template,
            ..
        } => {
            if let Some(def) = plugins.iter().find(|entry| entry.id == *id) {
                Some(def.command_template.clone())
            } else {
                Some(command_template.clone())
            }
        }
    };

    template.map(|template| {
        let mut command = template;
        for (needle, replacement) in replacements {
            command = command.replace(needle, &replacement);
        }
        command
    })
}

fn decoder_unavailability_reason(kind: &SdrDecoderKind, hardware: SdrHardware) -> Option<String> {
    match kind {
        SdrDecoderKind::Acars => {
            if command_exists("acarsdec")
                && resolve_acarsdec_command_line(131_550_000, hardware).is_none()
            {
                Some(format!(
                    "acarsdec is installed but {} mode is not configured for ACARS in this build",
                    hardware.label()
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn resolve_acarsdec_command_line(freq_hz: u64, hardware: SdrHardware) -> Option<String> {
    let freq_mhz = (freq_hz as f64) / 1_000_000.0;
    match hardware {
        SdrHardware::RtlSdr => Some(format!("acarsdec -o 4 -r 0 {freq_mhz:.3}")),
        _ => None,
    }
}

fn spawn_decoder(
    decoder: SdrDecoderKind,
    freq_hz: u64,
    command_line: String,
    sender: Sender<SdrEvent>,
    log_output_enabled: bool,
    log_output_dir: PathBuf,
    no_payload_satcom: bool,
) -> Result<RunningDecoder> {
    let mut command = Command::new("bash");
    command
        .arg("-lc")
        .arg(command_line.clone())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_family = "unix")]
    command.process_group(0);

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to launch decoder command: {}", command_line))?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_stdout = Arc::clone(&stop);
    let stop_stderr = Arc::clone(&stop);

    let decoder_name = decoder.label();
    let protocol = match &decoder {
        SdrDecoderKind::Plugin {
            protocol: Some(protocol),
            ..
        } => protocol.clone(),
        _ => decoder.default_protocol().to_string(),
    };

    let log_file = if log_output_enabled {
        let path = build_decode_log_path(&log_output_dir, &decoder_name);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                let _ = sender.send(SdrEvent::Log(format!(
                    "decoder logging to {}",
                    path.display()
                )));
                Some(Arc::new(Mutex::new(file)))
            }
            Err(err) => {
                let _ = sender.send(SdrEvent::Log(format!(
                    "failed to open decoder log file {}: {}",
                    path.display(),
                    err
                )));
                None
            }
        }
    } else {
        None
    };
    let map_log_file = if log_output_enabled {
        let path = build_map_log_path(&log_output_dir, &decoder_name);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => Some(Arc::new(Mutex::new(file))),
            Err(err) => {
                let _ = sender.send(SdrEvent::Log(format!(
                    "failed to open decoder map log {}: {}",
                    path.display(),
                    err
                )));
                None
            }
        }
    } else {
        None
    };
    let satcom_log_file = if log_output_enabled {
        let path = build_satcom_log_path(&log_output_dir, &decoder_name);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => Some(Arc::new(Mutex::new(file))),
            Err(err) => {
                let _ = sender.send(SdrEvent::Log(format!(
                    "failed to open decoder satcom audit log {}: {}",
                    path.display(),
                    err
                )));
                None
            }
        }
    } else {
        None
    };

    let stdout = child
        .stdout
        .take()
        .context("decoder stdout was not available")?;
    let stderr = child
        .stderr
        .take()
        .context("decoder stderr was not available")?;

    let sender_stdout = sender.clone();
    let decoder_name_stdout = decoder_name.clone();
    let protocol_stdout = protocol.clone();
    let log_stdout = log_file.clone();
    let map_log_stdout = map_log_file.clone();
    let satcom_log_stdout = satcom_log_file.clone();
    let no_payload_satcom_stdout = no_payload_satcom;
    let stdout_handle = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            if stop_stdout.load(Ordering::Relaxed) {
                break;
            }
            let Ok(line) = line else {
                break;
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let message = clamp_line(trimmed);
            let mut row = SdrDecodeRow {
                timestamp: Utc::now(),
                decoder: decoder_name_stdout.clone(),
                freq_hz,
                protocol: protocol_stdout.clone(),
                message: message.clone(),
                raw: message,
            };
            let mut map_point = parse_map_point(&row);
            let satcom_detected = derive_satcom_observation(&row, map_point.as_ref()).is_some();

            if no_payload_satcom_stdout && satcom_detected {
                redact_satcom_decode_row(&mut row);
                if let Some(point) = map_point.as_mut() {
                    sync_map_point_payload_from_row(point, &row);
                }
            }
            let satcom_observation = derive_satcom_observation(&row, map_point.as_ref());

            let _ = sender_stdout.send(SdrEvent::DecodeRow(row.clone()));
            if let Some(log_file) = &log_stdout {
                append_decode_log(log_file, &row);
            }

            if let Some(point) = map_point {
                let _ = sender_stdout.send(SdrEvent::MapPoint(point.clone()));
                if let Some(map_log_file) = &map_log_stdout {
                    append_map_point_log(map_log_file, &point);
                }
            }

            if let Some(satcom) = satcom_observation {
                let _ = sender_stdout.send(SdrEvent::SatcomObservation(satcom.clone()));
                if let Some(satcom_log_file) = &satcom_log_stdout {
                    append_satcom_observation_log(satcom_log_file, &satcom);
                }
            }
        }
    });

    let sender_stderr = sender.clone();
    let decoder_name_stderr = decoder_name.clone();
    let stderr_handle = thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            if stop_stderr.load(Ordering::Relaxed) {
                break;
            }
            let Ok(line) = line else {
                break;
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let _ = sender_stderr.send(SdrEvent::Log(format!(
                "{}: {}",
                decoder_name_stderr,
                clamp_line(trimmed)
            )));
        }
    });

    Ok(RunningDecoder {
        name: decoder_name,
        child,
        stop,
        stdout_handle: Some(stdout_handle),
        stderr_handle: Some(stderr_handle),
    })
}

fn build_decode_log_path(output_dir: &Path, decoder_name: &str) -> PathBuf {
    let ts = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    output_dir.join(format!(
        "{}_{}.log",
        sanitize_name(decoder_name),
        sanitize_name(&ts)
    ))
}

fn append_decode_log(log_file: &Arc<Mutex<File>>, row: &SdrDecodeRow) {
    if let Ok(mut file) = log_file.lock() {
        let _ = writeln!(
            file,
            "{}\t{}\t{}\t{}\t{}\t{}",
            row.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
            row.decoder,
            row.freq_hz,
            row.protocol,
            row.message,
            row.raw
        );
    }
}

fn build_map_log_path(output_dir: &Path, decoder_name: &str) -> PathBuf {
    let ts = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    output_dir.join(format!(
        "{}_{}_map.jsonl",
        sanitize_name(decoder_name),
        sanitize_name(&ts)
    ))
}

fn append_map_point_log(log_file: &Arc<Mutex<File>>, point: &SdrMapPoint) {
    if let Ok(mut file) = log_file.lock() {
        if let Ok(encoded) = serde_json::to_string(point) {
            let _ = writeln!(file, "{encoded}");
        }
    }
}

fn build_satcom_log_path(output_dir: &Path, decoder_name: &str) -> PathBuf {
    let ts = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    output_dir.join(format!(
        "{}_{}_satcom_audit.jsonl",
        sanitize_name(decoder_name),
        sanitize_name(&ts)
    ))
}

fn append_satcom_observation_log(log_file: &Arc<Mutex<File>>, observation: &SdrSatcomObservation) {
    if let Ok(mut file) = log_file.lock() {
        if let Ok(encoded) = serde_json::to_string(observation) {
            let _ = writeln!(file, "{encoded}");
        }
    }
}

fn parse_map_point(row: &SdrDecodeRow) -> Option<SdrMapPoint> {
    let line = row.message.as_str();

    let lat_named = Regex::new(r"(?i)\b(?:lat|latitude)\s*[:=]\s*(-?\d{1,2}(?:\.\d+)?)").ok()?;
    let lon_named =
        Regex::new(r"(?i)\b(?:lon|long|lng|longitude)\s*[:=]\s*(-?\d{1,3}(?:\.\d+)?)").ok()?;
    let pair_re = Regex::new(r"(-?\d{1,2}\.\d+)\s*[,/ ]\s*(-?\d{1,3}\.\d+)").ok()?;
    let alt_re =
        Regex::new(r"(?i)\b(?:alt|altitude)\s*[:=]?\s*(-?\d+(?:\.\d+)?)\s*(m|ft)?").ok()?;

    let mut latitude = None;
    let mut longitude = None;
    if let Some(caps) = lat_named.captures(line) {
        latitude = caps.get(1).and_then(|m| m.as_str().parse::<f64>().ok());
    }
    if let Some(caps) = lon_named.captures(line) {
        longitude = caps.get(1).and_then(|m| m.as_str().parse::<f64>().ok());
    }

    if latitude.is_none() || longitude.is_none() {
        if let Some(caps) = pair_re.captures(line) {
            latitude = caps.get(1).and_then(|m| m.as_str().parse::<f64>().ok());
            longitude = caps.get(2).and_then(|m| m.as_str().parse::<f64>().ok());
        }
    }

    let latitude = latitude?;
    let longitude = longitude?;
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return None;
    }

    let altitude_m = alt_re.captures(line).and_then(|caps| {
        let value = caps.get(1)?.as_str().parse::<f64>().ok()?;
        let unit = caps
            .get(2)
            .map(|m| m.as_str().to_ascii_lowercase())
            .unwrap_or_else(|| "m".to_string());
        Some(if unit == "ft" { value * 0.3048 } else { value })
    });

    let label = format!("{} {}", row.decoder, row.protocol);
    Some(SdrMapPoint {
        timestamp: row.timestamp,
        decoder: row.decoder.clone(),
        protocol: row.protocol.clone(),
        freq_hz: row.freq_hz,
        latitude,
        longitude,
        altitude_m,
        label,
        message: row.message.clone(),
        raw: row.raw.clone(),
    })
}

fn sync_map_point_payload_from_row(point: &mut SdrMapPoint, row: &SdrDecodeRow) {
    point.message = row.message.clone();
    point.raw = row.raw.clone();
}

fn derive_satcom_observation(
    row: &SdrDecodeRow,
    map_point: Option<&SdrMapPoint>,
) -> Option<SdrSatcomObservation> {
    let band = satcom_band_from_freq(row.freq_hz).or_else(|| {
        protocol_or_decoder_is_satcom(&row.protocol, &row.decoder).then_some("Unknown Satcom Band")
    })?;
    let posture = satcom_encryption_posture(&row.message);
    let identifier_hints = satcom_identifier_hints(&row.message);
    let has_coordinates = map_point.is_some();
    let summary = format!(
        "band={} posture={} coords={} identifiers={}",
        band,
        posture,
        if has_coordinates { "yes" } else { "no" },
        if identifier_hints.is_empty() {
            "none".to_string()
        } else {
            identifier_hints.join(",")
        }
    );

    Some(SdrSatcomObservation {
        timestamp: row.timestamp,
        decoder: row.decoder.clone(),
        protocol: row.protocol.clone(),
        freq_hz: row.freq_hz,
        band: band.to_string(),
        encryption_posture: posture,
        has_coordinates,
        identifier_hints,
        summary,
        message: row.message.clone(),
        raw: row.raw.clone(),
    })
}

fn satcom_band_from_freq(freq_hz: u64) -> Option<&'static str> {
    match freq_hz {
        1_525_000_000..=1_710_000_000 => Some("L-Band"),
        1_980_000_000..=2_300_000_000 => Some("S-Band"),
        3_400_000_000..=4_200_000_000 => Some("C-Band"),
        10_700_000_000..=12_750_000_000 => Some("Ku-Band"),
        17_700_000_000..=31_000_000_000 => Some("Ka-Band"),
        _ => None,
    }
}

fn protocol_or_decoder_is_satcom(protocol: &str, decoder: &str) -> bool {
    let text = format!(
        "{} {}",
        protocol.to_ascii_lowercase(),
        decoder.to_ascii_lowercase()
    );
    [
        "satcom",
        "satellite",
        "iridium",
        "inmarsat",
        "stdc",
        "inmarsat_c",
        "inmarsat-c",
        "orbcomm",
        "globalstar",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn satcom_encryption_posture(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    let clear_markers = ["clear", "plaintext", "unencrypted", "no cipher", "open"];
    if clear_markers.iter().any(|marker| lower.contains(marker)) {
        return "unencrypted".to_string();
    }

    let encrypted_markers = ["encrypted", "cipher", "scrambled", "crypt", "secure", "aes"];
    if encrypted_markers
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return "encrypted".to_string();
    }

    "unknown".to_string()
}

fn satcom_identifier_hints(message: &str) -> Vec<String> {
    let mut hints = Vec::new();
    let lower = message.to_ascii_lowercase();

    let mmsi_re = Regex::new(r"(?i)\bmmsi\s*[:=]?\s*\d{9}\b").ok();
    let icao_re = Regex::new(r"(?i)\bicao(?:24)?\s*[:=]?\s*[0-9a-f]{6}\b").ok();
    let callsign_re = Regex::new(r"(?i)\bcallsign\s*[:=]?\s*[a-z0-9\-]{3,12}\b").ok();
    let flight_re = Regex::new(r"(?i)\bflight\s*[:=]?\s*[a-z0-9\-]{2,12}\b").ok();

    if mmsi_re
        .as_ref()
        .map(|re| re.is_match(message))
        .unwrap_or(false)
    {
        hints.push("mmsi".to_string());
    }
    if icao_re
        .as_ref()
        .map(|re| re.is_match(message))
        .unwrap_or(false)
    {
        hints.push("icao_hex".to_string());
    }
    if callsign_re
        .as_ref()
        .map(|re| re.is_match(message))
        .unwrap_or(false)
        || flight_re
            .as_ref()
            .map(|re| re.is_match(message))
            .unwrap_or(false)
    {
        hints.push("callsign".to_string());
    }
    if lower.contains("lat") && lower.contains("lon") {
        hints.push("coordinates".to_string());
    }
    hints
}

fn decoder_autotune_frequency_hz(kind: &SdrDecoderKind) -> Option<u64> {
    match kind {
        SdrDecoderKind::Adsb => Some(1_090_000_000),
        SdrDecoderKind::Acars => Some(131_550_000),
        SdrDecoderKind::Ais => Some(162_025_000),
        SdrDecoderKind::Pocsag => Some(929_612_500),
        SdrDecoderKind::Iridium => Some(1_626_000_000),
        SdrDecoderKind::Dect => Some(1_886_400_000),
        SdrDecoderKind::GsmLte => Some(947_200_000),
        SdrDecoderKind::Plugin { id, .. } => decoder_autotune_for_plugin_id(id),
        _ => None,
    }
}

fn decoder_autotune_for_plugin_id(id: &str) -> Option<u64> {
    let id = id.to_ascii_lowercase();
    if id.contains("adsb") {
        Some(1_090_000_000)
    } else if id.contains("acars") {
        Some(131_550_000)
    } else if id.contains("ais") {
        Some(162_025_000)
    } else if id.contains("aprs") || id.contains("ax25") || id.contains("packet") {
        Some(144_390_000)
    } else if id.contains("pocsag") || id.contains("flex") {
        Some(929_612_500)
    } else if id.contains("iridium") {
        Some(1_626_000_000)
    } else if id.contains("inmarsat") {
        Some(1_541_450_000)
    } else if id.contains("dect") {
        Some(1_886_400_000)
    } else if id.contains("gsm") || id.contains("lte") || id.contains("cell") {
        Some(947_200_000)
    } else if id.contains("radiosonde") || id.contains("rs41") {
        Some(403_500_000)
    } else if id.contains("weather") || id.contains("noaa") || id.contains("apt") {
        Some(137_100_000)
    } else if id.contains("vor") {
        Some(113_000_000)
    } else if id.contains("dab") {
        Some(220_352_000)
    } else if id.contains("dmr")
        || id.contains("dpmr")
        || id.contains("dstar")
        || id.contains("m17")
    {
        Some(446_000_000)
    } else if id.contains("tetra") {
        Some(390_000_000)
    } else if id.contains("dji") || id.contains("droneid") {
        Some(2_437_000_000)
    } else if id.contains("parrot") {
        Some(2_437_000_000)
    } else if id.contains("opendroneid") || id.contains("remoteid") || id.contains("rid") {
        Some(2_437_000_000)
    } else if id.contains("stdc") || id.contains("inmarsat_c") || id.contains("inmarsatc") {
        Some(1_541_450_000)
    } else if id.contains("lora") {
        Some(868_100_000)
    } else {
        None
    }
}

fn normalize_scan_range(start_hz: u64, end_hz: u64, step_hz: u64) -> (u64, u64, u64) {
    let start = start_hz.max(100_000);
    let end = end_hz.max(start);
    let step = step_hz.max(1);
    (start, end, step)
}

fn try_set_bias_tee(hardware: SdrHardware, enabled: bool, sender: &Sender<SdrEvent>) -> Result<()> {
    match hardware {
        SdrHardware::RtlSdr => {
            if !command_exists("rtl_biast") {
                let _ = sender.send(SdrEvent::Log(
                    "bias-tee request ignored: rtl_biast not installed".to_string(),
                ));
                return Ok(());
            }
            let command = format!("rtl_biast -b {}", if enabled { "1" } else { "0" });
            let output = Command::new("bash").arg("-lc").arg(command).output()?;
            if output.status.success() {
                let _ = sender.send(SdrEvent::Log(format!(
                    "RTL-SDR bias-tee {}",
                    if enabled { "enabled" } else { "disabled" }
                )));
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let _ = sender.send(SdrEvent::Log(format!(
                    "failed to toggle RTL-SDR bias-tee: {}",
                    stderr.trim()
                )));
            }
        }
        SdrHardware::HackRf => {
            if !command_exists("hackrf_transfer") {
                let _ = sender.send(SdrEvent::Log(
                    "bias-tee request ignored: hackrf_transfer not installed".to_string(),
                ));
                return Ok(());
            }
            let command = format!(
                "hackrf_transfer -f 100000000 -s 2000000 -n 1 -r /dev/null -p {}",
                if enabled { "1" } else { "0" }
            );
            let output = Command::new("bash").arg("-lc").arg(command).output()?;
            if output.status.success() {
                let _ = sender.send(SdrEvent::Log(format!(
                    "HackRF antenna port power {}",
                    if enabled { "enabled" } else { "disabled" }
                )));
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let _ = sender.send(SdrEvent::Log(format!(
                    "failed to toggle HackRF antenna port power: {}",
                    stderr.trim()
                )));
            }
        }
        SdrHardware::BladeRf => {
            if !command_exists("bladeRF-cli") {
                let _ = sender.send(SdrEvent::Log(
                    "bias-tee request ignored: bladeRF-cli not installed".to_string(),
                ));
                return Ok(());
            }
            let command = format!(
                "bladeRF-cli -e \"set biastee rx {}\"",
                if enabled { "on" } else { "off" }
            );
            let output = Command::new("bash").arg("-lc").arg(command).output()?;
            if output.status.success() {
                let _ = sender.send(SdrEvent::Log(format!(
                    "bladeRF bias-tee {}",
                    if enabled { "enabled" } else { "disabled" }
                )));
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let _ = sender.send(SdrEvent::Log(format!(
                    "failed to toggle bladeRF bias-tee: {}",
                    stderr.trim()
                )));
            }
        }
        SdrHardware::EttusB210 => {
            let _ = sender.send(SdrEvent::Log(
                "B210 antenna power/bias-tee toggle depends on external RF frontend and is not directly exposed by this runtime".to_string(),
            ));
        }
    }
    Ok(())
}

fn capture_iq_sample(
    hardware: SdrHardware,
    center_freq_hz: u64,
    sample_rate_hz: u32,
    duration_secs: u32,
    output_dir: PathBuf,
    sender: &Sender<SdrEvent>,
) -> Result<()> {
    let duration_secs = duration_secs.max(1);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "failed to create sample output dir {}",
            output_dir.display()
        )
    })?;

    let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let file_name = format!(
        "iq_{}_{}_{}_{}.iq",
        sanitize_name(hardware.id()),
        center_freq_hz,
        sample_rate_hz,
        timestamp
    );
    let output_path = output_dir.join(file_name);
    let sample_count = (sample_rate_hz as u64).saturating_mul(duration_secs as u64);

    let command_line = match hardware {
        SdrHardware::RtlSdr if command_exists("rtl_sdr") => format!(
            "rtl_sdr -f {} -s {} -n {} {}",
            center_freq_hz,
            sample_rate_hz,
            sample_count,
            shell_escape_path(&output_path)
        ),
        SdrHardware::HackRf if command_exists("hackrf_transfer") => format!(
            "hackrf_transfer -f {} -s {} -n {} -r {}",
            center_freq_hz,
            sample_rate_hz,
            sample_count,
            shell_escape_path(&output_path)
        ),
        SdrHardware::BladeRf if command_exists("bladeRF-cli") => {
            format!(
                "bladeRF-cli -e \"set frequency rx {}; set samplerate rx {}; rx config file={} format=bin n={}; rx start; rx wait\"",
                center_freq_hz,
                sample_rate_hz,
                shell_escape_path(&output_path),
                sample_count
            )
        }
        SdrHardware::EttusB210 if command_exists("uhd_rx_cfile") => format!(
            "uhd_rx_cfile -f {} -r {} -N {} {}",
            center_freq_hz,
            sample_rate_hz,
            sample_count,
            shell_escape_path(&output_path)
        ),
        _ => {
            let _ = sender.send(SdrEvent::Log(format!(
                "sample capture not available for {} (missing capture tool)",
                hardware.label()
            )));
            return Ok(());
        }
    };

    let _ = sender.send(SdrEvent::Log(format!(
        "capturing {}s IQ sample to {}",
        duration_secs,
        output_path.display()
    )));
    let output = Command::new("bash").arg("-lc").arg(command_line).output()?;
    if output.status.success() {
        let _ = sender.send(SdrEvent::Log(format!(
            "IQ sample saved: {}",
            output_path.display()
        )));
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = sender.send(SdrEvent::Log(format!(
            "IQ sample capture failed: {}",
            stderr.trim()
        )));
    }
    Ok(())
}

fn shell_escape_path(path: &Path) -> String {
    let raw = path.to_string_lossy().to_string();
    format!("'{}'", raw.replace('\'', "'\"'\"'"))
}

fn clamp_line(line: &str) -> String {
    line.chars()
        .take(MAX_DECODE_MESSAGE_LEN)
        .collect::<String>()
}

fn redact_satcom_decode_row(row: &mut SdrDecodeRow) {
    let redacted = "[redacted: satcom payload disabled]".to_string();
    row.message = redacted.clone();
    row.raw = redacted;
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
}

fn command_search_paths() -> Vec<PathBuf> {
    let mut paths = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    if let Some(home_dir) = dirs::home_dir() {
        paths.push(home_dir.join(".local/bin"));
    }
    paths
}

fn command_exists(command: &str) -> bool {
    command_search_paths()
        .iter()
        .any(|base| base.join(command).exists())
}

fn command_exists_any_owned(commands: &[String]) -> bool {
    commands.iter().any(|command| command_exists(command))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DependencyDescriptor {
    tool: String,
    package_hint: String,
}

#[derive(Debug, Clone)]
struct DependencyInstallPlan {
    apt_candidates: Vec<String>,
    pip_candidates: Vec<String>,
    verify_commands: Vec<String>,
    source_install_command: Option<String>,
}

fn dependency_install_plan(package_hint: &str) -> DependencyInstallPlan {
    match package_hint {
        "rtl-433" => DependencyInstallPlan {
            apt_candidates: vec!["rtl-433".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["rtl_433".to_string()],
            source_install_command: None,
        },
        "dump1090-mutability" => DependencyInstallPlan {
            apt_candidates: vec![
                "dump1090-mutability".to_string(),
                "dump1090-fa".to_string(),
                "readsb".to_string(),
                "dump1090".to_string(),
            ],
            pip_candidates: Vec::new(),
            verify_commands: vec![
                "dump1090".to_string(),
                "dump1090-mutability".to_string(),
                "readsb".to_string(),
            ],
            source_install_command: None,
        },
        "acarsdec" => DependencyInstallPlan {
            apt_candidates: vec!["acarsdec".to_string(), "dumpvdl2".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["acarsdec".to_string(), "acars_parser".to_string()],
            source_install_command: Some("sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y git build-essential cmake pkg-config && tmp=\"$(mktemp -d)\" && git clone --depth 1 https://github.com/TLeconte/acarsdec.git \"$tmp/acarsdec\" && cmake -S \"$tmp/acarsdec\" -B \"$tmp/acarsdec/build\" && cmake --build \"$tmp/acarsdec/build\" -j\"$(nproc)\" && sudo -n cmake --install \"$tmp/acarsdec/build\"".to_string()),
        },
        "ais-tools" => DependencyInstallPlan {
            apt_candidates: vec!["ais-tools".to_string(), "rtl-ais".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["rtl_ais".to_string(), "aisdecoder".to_string()],
            source_install_command: Some("sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y git build-essential librtlsdr-dev && tmp=\"$(mktemp -d)\" && git clone --depth 1 https://github.com/dgiardini/rtl-ais.git \"$tmp/rtl-ais\" && make -C \"$tmp/rtl-ais\" && sudo -n make -C \"$tmp/rtl-ais\" install".to_string()),
        },
        "multimon-ng" => DependencyInstallPlan {
            apt_candidates: vec!["multimon-ng".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["multimon-ng".to_string()],
            source_install_command: None,
        },
        "rtl-sdr" => DependencyInstallPlan {
            apt_candidates: vec![
                "rtl-sdr".to_string(),
                "librtlsdr-bin".to_string(),
                "rtl-sdr-tools".to_string(),
            ],
            pip_candidates: Vec::new(),
            verify_commands: vec!["rtl_fm".to_string(), "rtl_sdr".to_string()],
            source_install_command: None,
        },
        "iridium-toolkit" => DependencyInstallPlan {
            apt_candidates: vec!["iridium-toolkit".to_string()],
            pip_candidates: vec![
                "git+https://github.com/muccc/iridium-toolkit.git".to_string(),
                "iridium-toolkit".to_string(),
            ],
            verify_commands: vec!["iridium-extractor".to_string()],
            source_install_command: None,
        },
        "gr-gsm" => DependencyInstallPlan {
            apt_candidates: vec!["gr-gsm".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["grgsm_livemon_headless".to_string(), "cell_search".to_string()],
            source_install_command: Some("sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y git cmake g++ libboost-all-dev libcppunit-dev swig doxygen liblog4cpp5-dev python3-click python3-click-plugins python3-zmq python3-scipy python3-gi-cairo python3-apt && tmp=\"$(mktemp -d)\" && git clone --depth 1 https://github.com/ptrkrysik/gr-gsm.git \"$tmp/gr-gsm\" && cmake -S \"$tmp/gr-gsm\" -B \"$tmp/gr-gsm/build\" && cmake --build \"$tmp/gr-gsm/build\" -j\"$(nproc)\" && sudo -n cmake --install \"$tmp/gr-gsm/build\" && sudo -n ldconfig".to_string()),
        },
        "hackrf" => DependencyInstallPlan {
            apt_candidates: vec!["hackrf".to_string(), "hackrf-tools".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["hackrf_info".to_string(), "hackrf_transfer".to_string()],
            source_install_command: None,
        },
        "bladerf" => DependencyInstallPlan {
            apt_candidates: vec!["bladerf".to_string(), "bladerf-tools".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["bladeRF-cli".to_string()],
            source_install_command: None,
        },
        "uhd-host" => DependencyInstallPlan {
            apt_candidates: vec!["uhd-host".to_string(), "libuhd-utils".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["uhd_find_devices".to_string(), "uhd_rx_cfile".to_string()],
            source_install_command: None,
        },
        "gqrx-sdr" => DependencyInstallPlan {
            apt_candidates: vec!["gqrx-sdr".to_string(), "gqrx".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["gqrx".to_string()],
            source_install_command: None,
        },
        "direwolf" => DependencyInstallPlan {
            apt_candidates: vec!["direwolf".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["direwolf".to_string()],
            source_install_command: None,
        },
        "dsd" => DependencyInstallPlan {
            apt_candidates: vec!["dsd".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["dsd".to_string()],
            source_install_command: None,
        },
        "welle.io" => DependencyInstallPlan {
            apt_candidates: vec![
                "welle.io".to_string(),
                "welle-io".to_string(),
                "welle-cli".to_string(),
            ],
            pip_candidates: Vec::new(),
            verify_commands: vec!["welle-cli".to_string()],
            source_install_command: None,
        },
        "sox" => DependencyInstallPlan {
            apt_candidates: vec!["sox".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["sox".to_string()],
            source_install_command: None,
        },
        "csdr" => DependencyInstallPlan {
            apt_candidates: vec!["csdr".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["csdr".to_string()],
            source_install_command: None,
        },
        "satdump" => DependencyInstallPlan {
            apt_candidates: vec!["satdump".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["satdump".to_string()],
            source_install_command: Some("sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y git cmake build-essential libfftw3-dev libvolk2-dev && tmp=\"$(mktemp -d)\" && git clone --depth 1 https://github.com/SatDump/SatDump.git \"$tmp/satdump\" && cmake -S \"$tmp/satdump\" -B \"$tmp/satdump/build\" && cmake --build \"$tmp/satdump/build\" -j\"$(nproc)\" && sudo -n cmake --install \"$tmp/satdump/build\"".to_string()),
        },
        "radiosonde-auto-rx" => DependencyInstallPlan {
            apt_candidates: vec!["radiosonde-auto-rx".to_string()],
            pip_candidates: vec![
                "git+https://github.com/projecthorus/radiosonde_auto_rx.git".to_string(),
            ],
            verify_commands: vec!["auto_rx".to_string()],
            source_install_command: None,
        },
        "op25" => DependencyInstallPlan {
            apt_candidates: vec!["op25".to_string(), "op25-repeater".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["op25_rx.py".to_string(), "rx.py".to_string()],
            source_install_command: Some("sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y git g++ make cmake libitpp-dev libcppunit-dev libboost-all-dev python3-numpy python3-requests python3-matplotlib && tmp=\"$(mktemp -d)\" && git clone --depth 1 https://github.com/boatbod/op25.git \"$tmp/op25\" && cd \"$tmp/op25\" && ./install.sh".to_string()),
        },
        "dsd-fme" => DependencyInstallPlan {
            apt_candidates: vec!["dsd-fme".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["dsd-fme".to_string()],
            source_install_command: Some("sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y git build-essential cmake pkg-config libsndfile1-dev && tmp=\"$(mktemp -d)\" && git clone --depth 1 https://github.com/lwvmobile/dsd-fme.git \"$tmp/dsd-fme\" && cmake -S \"$tmp/dsd-fme\" -B \"$tmp/dsd-fme/build\" && cmake --build \"$tmp/dsd-fme/build\" -j\"$(nproc)\" && sudo -n cmake --install \"$tmp/dsd-fme/build\"".to_string()),
        },
        "fldigi" => DependencyInstallPlan {
            apt_candidates: vec!["fldigi".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["fldigi".to_string()],
            source_install_command: None,
        },
        "gr-droneid" => DependencyInstallPlan {
            apt_candidates: vec!["gr-droneid".to_string()],
            pip_candidates: vec!["git+https://github.com/proto17/dji_droneid.git".to_string()],
            verify_commands: vec!["droneid_receiver".to_string(), "droneid_decode".to_string()],
            source_install_command: None,
        },
        "opendroneid" => DependencyInstallPlan {
            apt_candidates: vec!["opendroneid".to_string()],
            pip_candidates: vec![
                "opendroneid".to_string(),
                "git+https://github.com/opendroneid/opendroneid-core-c.git".to_string(),
            ],
            verify_commands: vec![
                "opendroneid_rx".to_string(),
                "opendroneid-decode".to_string(),
                "odid-decode".to_string(),
            ],
            source_install_command: None,
        },
        "stdc-decoder" => DependencyInstallPlan {
            apt_candidates: vec!["stdc-decoder".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec![
                "stdc_decoder".to_string(),
                "stdcdec".to_string(),
                "inmarsatc-decoder".to_string(),
            ],
            source_install_command: None,
        },
        "freedv" => DependencyInstallPlan {
            apt_candidates: vec!["freedv".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["freedv_rx".to_string(), "freedv_tx".to_string()],
            source_install_command: None,
        },
        "leandvb" => DependencyInstallPlan {
            apt_candidates: vec!["leandvb".to_string(), "leansdr".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["leandvb".to_string()],
            source_install_command: Some("sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y git build-essential libfftw3-dev libusb-1.0-0-dev && tmp=\"$(mktemp -d)\" && git clone --depth 1 https://github.com/pabr/leansdr.git \"$tmp/leansdr\" && make -C \"$tmp/leansdr/src\" && sudo -n install -m 0755 \"$tmp/leansdr/src/leandvb\" /usr/local/bin/leandvb".to_string()),
        },
        "tvheadend" => DependencyInstallPlan {
            apt_candidates: vec!["tvheadend".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["tvheadend".to_string()],
            source_install_command: None,
        },
        "gr-lora" => DependencyInstallPlan {
            apt_candidates: vec!["gr-lora".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["lora_receive_file".to_string()],
            source_install_command: Some("sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y git cmake g++ swig python3-numpy libgmp-dev libmpfr-dev libboost-all-dev gnuradio-dev && tmp=\"$(mktemp -d)\" && git clone --depth 1 https://github.com/rpp0/gr-lora.git \"$tmp/gr-lora\" && cmake -S \"$tmp/gr-lora\" -B \"$tmp/gr-lora/build\" && cmake --build \"$tmp/gr-lora/build\" -j\"$(nproc)\" && sudo -n cmake --install \"$tmp/gr-lora/build\" && sudo -n ldconfig".to_string()),
        },
        "m17-tools" => DependencyInstallPlan {
            apt_candidates: vec!["m17-tools".to_string(), "m17-demod".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["m17-demod".to_string()],
            source_install_command: None,
        },
        "jaero" => DependencyInstallPlan {
            apt_candidates: vec!["jaero".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["jaero".to_string()],
            source_install_command: None,
        },
        "osmo-tetra" => DependencyInstallPlan {
            apt_candidates: vec!["osmo-tetra".to_string(), "tetra-rx".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["tetra-rx".to_string(), "osmo-tetra".to_string()],
            source_install_command: None,
        },
        "dump978" => DependencyInstallPlan {
            apt_candidates: vec!["dump978-fa".to_string(), "dump978".to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec!["dump978-fa".to_string(), "dump978".to_string()],
            source_install_command: None,
        },
        "srsran" => DependencyInstallPlan {
            apt_candidates: vec![
                "srsran".to_string(),
                "srsran-4g".to_string(),
                "srsran-5g".to_string(),
                "lte-cell-scanner".to_string(),
            ],
            pip_candidates: Vec::new(),
            verify_commands: vec![
                "srsue".to_string(),
                "srsenb".to_string(),
                "lte-cell-scanner".to_string(),
            ],
            source_install_command: None,
        },
        other => DependencyInstallPlan {
            apt_candidates: vec![other.to_string()],
            pip_candidates: Vec::new(),
            verify_commands: vec![other.to_string()],
            source_install_command: None,
        },
    }
}

fn base_sdr_dependency_definitions() -> Vec<DependencyDescriptor> {
    vec![
        DependencyDescriptor {
            tool: "rtl_433".to_string(),
            package_hint: "rtl-433".to_string(),
        },
        DependencyDescriptor {
            tool: "dump1090/readsb".to_string(),
            package_hint: "dump1090-mutability".to_string(),
        },
        DependencyDescriptor {
            tool: "acarsdec/acars_parser".to_string(),
            package_hint: "acarsdec".to_string(),
        },
        DependencyDescriptor {
            tool: "rtl_ais/aisdecoder".to_string(),
            package_hint: "ais-tools".to_string(),
        },
        DependencyDescriptor {
            tool: "multimon-ng".to_string(),
            package_hint: "multimon-ng".to_string(),
        },
        DependencyDescriptor {
            tool: "rtl_fm".to_string(),
            package_hint: "rtl-sdr".to_string(),
        },
        DependencyDescriptor {
            tool: "iridium-extractor".to_string(),
            package_hint: "iridium-toolkit".to_string(),
        },
        DependencyDescriptor {
            tool: "grgsm_livemon_headless/cell_search".to_string(),
            package_hint: "gr-gsm".to_string(),
        },
        DependencyDescriptor {
            tool: "hackrf_info".to_string(),
            package_hint: "hackrf".to_string(),
        },
        DependencyDescriptor {
            tool: "bladeRF-cli".to_string(),
            package_hint: "bladerf".to_string(),
        },
        DependencyDescriptor {
            tool: "uhd_find_devices".to_string(),
            package_hint: "uhd-host".to_string(),
        },
        DependencyDescriptor {
            tool: "gqrx".to_string(),
            package_hint: "gqrx-sdr".to_string(),
        },
        DependencyDescriptor {
            tool: "direwolf".to_string(),
            package_hint: "direwolf".to_string(),
        },
        DependencyDescriptor {
            tool: "dsd".to_string(),
            package_hint: "dsd".to_string(),
        },
        DependencyDescriptor {
            tool: "welle-cli".to_string(),
            package_hint: "welle.io".to_string(),
        },
        DependencyDescriptor {
            tool: "sox".to_string(),
            package_hint: "sox".to_string(),
        },
        DependencyDescriptor {
            tool: "csdr".to_string(),
            package_hint: "csdr".to_string(),
        },
        DependencyDescriptor {
            tool: "satdump".to_string(),
            package_hint: "satdump".to_string(),
        },
        DependencyDescriptor {
            tool: "radiosonde_auto_rx".to_string(),
            package_hint: "radiosonde-auto-rx".to_string(),
        },
        DependencyDescriptor {
            tool: "op25".to_string(),
            package_hint: "op25".to_string(),
        },
        DependencyDescriptor {
            tool: "dsd-fme".to_string(),
            package_hint: "dsd-fme".to_string(),
        },
        DependencyDescriptor {
            tool: "multipsk/fldigi".to_string(),
            package_hint: "fldigi".to_string(),
        },
        DependencyDescriptor {
            tool: "DJI DroneID decoder".to_string(),
            package_hint: "gr-droneid".to_string(),
        },
        DependencyDescriptor {
            tool: "OpenDroneID decoder".to_string(),
            package_hint: "opendroneid".to_string(),
        },
        DependencyDescriptor {
            tool: "Inmarsat STD-C decoder".to_string(),
            package_hint: "stdc-decoder".to_string(),
        },
    ]
}

fn plugin_dependency_descriptors(plugin_defs: &[SdrPluginDefinition]) -> Vec<DependencyDescriptor> {
    plugin_defs
        .iter()
        .flat_map(|plugin| match plugin.id.as_str() {
            "freedv" => vec![DependencyDescriptor {
                tool: "FreeDV decoder".to_string(),
                package_hint: "freedv".to_string(),
            }],
            "dvb_s" | "dvb_s2" => vec![DependencyDescriptor {
                tool: "DVB-S/DVB-S2 decoder".to_string(),
                package_hint: "leandvb".to_string(),
            }],
            "ntsc" | "pal" => vec![DependencyDescriptor {
                tool: "Analog video monitor".to_string(),
                package_hint: "tvheadend".to_string(),
            }],
            "lora" => vec![DependencyDescriptor {
                tool: "LoRa decoder".to_string(),
                package_hint: "gr-lora".to_string(),
            }],
            "m17" => vec![DependencyDescriptor {
                tool: "M17 decoder".to_string(),
                package_hint: "m17-tools".to_string(),
            }],
            "inmarsat_aero" => vec![DependencyDescriptor {
                tool: "JAERO".to_string(),
                package_hint: "jaero".to_string(),
            }],
            "tetra" => vec![DependencyDescriptor {
                tool: "TETRA metadata decoder".to_string(),
                package_hint: "osmo-tetra".to_string(),
            }],
            "adsb_uat978" => vec![DependencyDescriptor {
                tool: "dump978".to_string(),
                package_hint: "dump978".to_string(),
            }],
            "lte_meta" => vec![DependencyDescriptor {
                tool: "LTE metadata scanner".to_string(),
                package_hint: "srsran".to_string(),
            }],
            _ => Vec::new(),
        })
        .collect()
}

fn dependency_descriptors_with_plugins(
    plugin_defs: &[SdrPluginDefinition],
) -> Vec<DependencyDescriptor> {
    let mut descriptors = base_sdr_dependency_definitions();
    descriptors.extend(plugin_dependency_descriptors(plugin_defs));
    let mut unique = Vec::with_capacity(descriptors.len());
    for descriptor in descriptors {
        if !unique.iter().any(|existing: &DependencyDescriptor| {
            existing.tool == descriptor.tool && existing.package_hint == descriptor.package_hint
        }) {
            unique.push(descriptor);
        }
    }
    unique
}

fn check_dependencies_for_plugins(plugin_defs: &[SdrPluginDefinition]) -> Vec<SdrDependencyStatus> {
    dependency_descriptors_with_plugins(plugin_defs)
        .into_iter()
        .map(|descriptor| {
            let plan = dependency_install_plan(&descriptor.package_hint);
            SdrDependencyStatus {
                tool: descriptor.tool,
                package_hint: descriptor.package_hint,
                installed: command_exists_any_owned(&plan.verify_commands),
            }
        })
        .collect()
}

fn command_output_reason(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }
    "unknown error".to_string()
}

fn source_install_timeout_secs() -> u64 {
    std::env::var("WIRELESSEXPLORER_SDR_SOURCE_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map(|secs| secs.clamp(30, 3600))
        .unwrap_or(300)
}

fn install_dependency_packages(packages: &[String]) -> Result<Vec<String>> {
    let mut normalized = packages
        .iter()
        .map(|pkg| pkg.trim())
        .filter(|pkg| !pkg.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();

    if normalized.is_empty() {
        return Ok(Vec::new());
    }

    let update_output = Command::new("bash")
        .arg("-lc")
        .arg("sudo -n env DEBIAN_FRONTEND=noninteractive apt-get update")
        .output()
        .context("failed to execute apt-get update for dependency installer")?;
    if !update_output.status.success() {
        let stderr = String::from_utf8_lossy(&update_output.stderr);
        anyhow::bail!("apt-get update failed: {}", stderr.trim());
    }

    let mut failures = Vec::new();
    for package in normalized {
        let plan = dependency_install_plan(&package);
        if command_exists_any_owned(&plan.verify_commands) {
            continue;
        }

        let mut attempts = Vec::new();
        let mut installed = false;

        for candidate in &plan.apt_candidates {
            let install_command = format!(
                "sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y {}",
                candidate
            );
            let output = Command::new("bash")
                .arg("-lc")
                .arg(&install_command)
                .output()
                .with_context(|| {
                    format!(
                        "failed to execute dependency installer apt command for {}",
                        candidate
                    )
                })?;

            if output.status.success() {
                attempts.push(format!("{candidate}: apt install ok"));
                if command_exists_any_owned(&plan.verify_commands) {
                    installed = true;
                    break;
                }
            } else {
                attempts.push(format!(
                    "{candidate}: apt install failed ({})",
                    command_output_reason(&output)
                ));
            }
        }

        if !installed {
            for pip_spec in &plan.pip_candidates {
                let pip_command = format!("python3 -m pip install --user --upgrade {}", pip_spec);
                let output = Command::new("bash")
                    .arg("-lc")
                    .arg(&pip_command)
                    .output()
                    .with_context(|| {
                        format!(
                            "failed to execute dependency installer pip command for {}",
                            pip_spec
                        )
                    })?;
                if output.status.success() {
                    attempts.push(format!("{pip_spec}: pip install ok"));
                    if command_exists_any_owned(&plan.verify_commands) {
                        installed = true;
                        break;
                    }
                } else {
                    attempts.push(format!(
                        "{pip_spec}: pip install failed ({})",
                        command_output_reason(&output)
                    ));
                }
            }
        }

        if !installed {
            if let Some(source_command) = &plan.source_install_command {
                let output = if command_exists("timeout") {
                    Command::new("timeout")
                        .arg("-k")
                        .arg("15s")
                        .arg(format!("{}s", source_install_timeout_secs()))
                        .arg("bash")
                        .arg("-lc")
                        .arg(source_command)
                        .output()
                        .context("failed to execute source fallback installer command")?
                } else {
                    Command::new("bash")
                        .arg("-lc")
                        .arg(source_command)
                        .output()
                        .context("failed to execute source fallback installer command")?
                };
                if output.status.success() {
                    attempts.push("source fallback install ok".to_string());
                    if command_exists_any_owned(&plan.verify_commands) {
                        installed = true;
                    }
                } else {
                    attempts.push(format!(
                        "source fallback install failed ({})",
                        command_output_reason(&output)
                    ));
                }
            }
        }

        if !installed && command_exists_any_owned(&plan.verify_commands) {
            installed = true;
        }

        if !installed {
            if attempts.is_empty() {
                attempts.push("no installer candidates available".to_string());
            }
            failures.push(format!("{package} ({})", attempts.join(" | ")));
        }
    }

    Ok(failures)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn sample_row(freq_hz: u64, protocol: &str, message: &str) -> SdrDecodeRow {
        SdrDecodeRow {
            timestamp: Utc::now(),
            decoder: "TestDecoder".to_string(),
            freq_hz,
            protocol: protocol.to_string(),
            message: message.to_string(),
            raw: message.to_string(),
        }
    }

    #[test]
    fn satcom_band_from_l_band_frequency() {
        assert_eq!(satcom_band_from_freq(1_626_000_000), Some("L-Band"));
    }

    #[test]
    fn satcom_posture_detects_unencrypted_markers() {
        assert_eq!(
            satcom_encryption_posture("frame status: clear plaintext"),
            "unencrypted"
        );
    }

    #[test]
    fn satcom_observation_is_emitted_for_sat_protocol_keyword() {
        let row = sample_row(137_100_000, "inmarsat_c", "message mmsi=123456789");
        let observation = derive_satcom_observation(&row, None).expect("satcom observation");
        assert_eq!(observation.band, "Unknown Satcom Band");
        assert_eq!(observation.encryption_posture, "unknown");
        assert!(observation.identifier_hints.contains(&"mmsi".to_string()));
        assert_eq!(observation.message, "message mmsi=123456789");
        assert_eq!(observation.raw, "message mmsi=123456789");
    }

    #[test]
    fn satcom_redaction_rewrites_message_and_raw() {
        let mut row = sample_row(
            1_541_000_000,
            "inmarsat_c",
            "clear payload sample mmsi=123456789",
        );
        redact_satcom_decode_row(&mut row);
        assert_eq!(row.message, "[redacted: satcom payload disabled]");
        assert_eq!(row.raw, "[redacted: satcom payload disabled]");
    }

    #[test]
    fn satcom_observation_uses_map_point_coordinates_flag() {
        let row = sample_row(1_626_000_000, "iridium", "mmsi=123456789");
        let map_point = SdrMapPoint {
            timestamp: Utc::now(),
            decoder: "Iridium".to_string(),
            protocol: "iridium".to_string(),
            freq_hz: 1_626_000_000,
            latitude: 35.1453957,
            longitude: -79.4747181,
            altitude_m: None,
            label: "test".to_string(),
            message: "message".to_string(),
            raw: "raw".to_string(),
        };
        let observation =
            derive_satcom_observation(&row, Some(&map_point)).expect("satcom observation");
        assert_eq!(observation.band, "L-Band");
        assert!(observation.has_coordinates);
        assert!(observation
            .identifier_hints
            .iter()
            .any(|hint| hint == "mmsi"));
    }

    #[test]
    fn parse_map_point_preserves_message_and_raw_fields() {
        let row = sample_row(
            162_025_000,
            "ais",
            "lat=35.1453957 lon=-79.4747181 callsign=TEST",
        );
        let point = parse_map_point(&row).expect("map point");
        assert_eq!(point.message, row.message);
        assert_eq!(point.raw, row.raw);
    }

    #[test]
    fn parse_map_point_converts_altitude_feet_to_meters() {
        let row = sample_row(
            162_025_000,
            "ais",
            "lat=35.1453957 lon=-79.4747181 alt=1000ft callsign=TEST",
        );
        let point = parse_map_point(&row).expect("map point");
        let altitude_m = point.altitude_m.expect("altitude");
        assert!((altitude_m - 304.8).abs() < 0.1);
    }

    #[test]
    fn parse_map_point_rejects_out_of_range_coordinates() {
        let row = sample_row(162_025_000, "ais", "lat=95.0 lon=-79.4747181");
        assert!(parse_map_point(&row).is_none());
    }

    #[test]
    fn sync_map_point_payload_from_row_updates_message_and_raw() {
        let mut row = sample_row(
            1_541_000_000,
            "inmarsat_c",
            "clear payload sample mmsi=123456789",
        );
        let mut point = parse_map_point(&sample_row(
            162_025_000,
            "ais",
            "lat=35.1453957 lon=-79.4747181 callsign=TEST",
        ))
        .expect("map point");
        redact_satcom_decode_row(&mut row);
        sync_map_point_payload_from_row(&mut point, &row);
        assert_eq!(point.message, "[redacted: satcom payload disabled]");
        assert_eq!(point.raw, "[redacted: satcom payload disabled]");
    }

    #[test]
    fn satcom_observation_uses_redacted_payload_when_row_is_redacted() {
        let mut row = sample_row(
            1_541_000_000,
            "inmarsat_c",
            "clear payload sample mmsi=123456789",
        );
        redact_satcom_decode_row(&mut row);
        let observation = derive_satcom_observation(&row, None).expect("satcom observation");
        assert_eq!(observation.message, "[redacted: satcom payload disabled]");
        assert_eq!(observation.raw, "[redacted: satcom payload disabled]");
    }

    #[test]
    fn satcom_observation_serialization_includes_message_and_raw_fields() {
        let row = sample_row(1_626_000_000, "iridium", "mmsi=123456789");
        let observation = derive_satcom_observation(&row, None).expect("satcom observation");
        let payload = serde_json::to_value(&observation).expect("serialize satcom observation");
        assert_eq!(payload["message"], "mmsi=123456789");
        assert_eq!(payload["raw"], "mmsi=123456789");
    }

    #[test]
    fn append_map_point_log_writes_message_and_raw_fields() {
        let path =
            std::env::temp_dir().join(format!("wirelessexplorer-map-log-{}.jsonl", Uuid::new_v4()));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open temp map log");
        let log = Arc::new(Mutex::new(file));

        let point = SdrMapPoint {
            timestamp: Utc::now(),
            decoder: "TestDecoder".to_string(),
            protocol: "iridium".to_string(),
            freq_hz: 1_626_000_000,
            latitude: 35.1453957,
            longitude: -79.4747181,
            altitude_m: None,
            label: "test".to_string(),
            message: "decoded message".to_string(),
            raw: "raw payload".to_string(),
        };
        append_map_point_log(&log, &point);

        let raw = fs::read_to_string(&path).expect("read map log file");
        let line = raw.lines().next().expect("map log line");
        let payload: serde_json::Value = serde_json::from_str(line).expect("parse map jsonl");
        assert_eq!(payload["message"], "decoded message");
        assert_eq!(payload["raw"], "raw payload");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn append_satcom_observation_log_writes_message_and_raw_fields() {
        let path = std::env::temp_dir().join(format!(
            "wirelessexplorer-satcom-log-{}.jsonl",
            Uuid::new_v4()
        ));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open temp satcom log");
        let log = Arc::new(Mutex::new(file));

        let observation = SdrSatcomObservation {
            timestamp: Utc::now(),
            decoder: "TestDecoder".to_string(),
            protocol: "inmarsat_c".to_string(),
            freq_hz: 1_541_000_000,
            band: "L-Band".to_string(),
            encryption_posture: "unknown".to_string(),
            has_coordinates: false,
            identifier_hints: vec!["mmsi".to_string()],
            summary: "band=L-Band posture=unknown coords=no identifiers=mmsi".to_string(),
            message: "decoded message".to_string(),
            raw: "raw payload".to_string(),
        };
        append_satcom_observation_log(&log, &observation);

        let raw = fs::read_to_string(&path).expect("read satcom log file");
        let line = raw.lines().next().expect("satcom log line");
        let payload: serde_json::Value = serde_json::from_str(line).expect("parse satcom jsonl");
        assert_eq!(payload["message"], "decoded message");
        assert_eq!(payload["raw"], "raw payload");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn dependency_plan_dump1090_includes_binary_fallbacks() {
        let plan = dependency_install_plan("dump1090-mutability");
        assert!(plan
            .apt_candidates
            .iter()
            .any(|candidate| candidate == "dump1090-mutability"));
        assert!(plan
            .apt_candidates
            .iter()
            .any(|candidate| candidate == "readsb"));
        assert!(plan
            .verify_commands
            .iter()
            .any(|command| command == "dump1090"));
    }

    #[test]
    fn dependency_plan_op25_has_source_fallback() {
        let plan = dependency_install_plan("op25");
        assert!(plan
            .verify_commands
            .iter()
            .any(|command| command == "rx.py" || command == "op25-rx"));
        let source = plan
            .source_install_command
            .expect("op25 should provide source fallback");
        assert!(
            source.contains("github.com/boatbod/op25"),
            "unexpected source fallback command: {source}"
        );
    }

    #[test]
    fn dependency_plan_unknown_uses_hint_as_default() {
        let plan = dependency_install_plan("example-tool");
        assert_eq!(plan.apt_candidates, vec!["example-tool".to_string()]);
        assert_eq!(plan.verify_commands, vec!["example-tool".to_string()]);
        assert!(plan.pip_candidates.is_empty());
    }

    #[test]
    fn acarsdec_command_line_uses_rtl_mode_with_mhz_frequency() {
        let command = resolve_acarsdec_command_line(131_550_000, SdrHardware::RtlSdr)
            .expect("rtl acars command");
        assert_eq!(command, "acarsdec -o 4 -r 0 131.550");
    }

    #[test]
    fn acarsdec_command_line_is_not_assumed_for_non_rtl_hardware() {
        assert!(resolve_acarsdec_command_line(131_550_000, SdrHardware::HackRf).is_none());
    }

    #[test]
    fn acars_decoder_requires_acarsdec_when_resolving_command_line() {
        let command = resolve_decoder_command_line(
            &SdrDecoderKind::Acars,
            131_550_000,
            2_400_000,
            SdrHardware::HackRf,
            &[],
        );
        assert!(command.is_none());
    }

    #[test]
    fn decoder_unavailability_reason_is_none_for_non_acars() {
        assert!(decoder_unavailability_reason(&SdrDecoderKind::Ais, SdrHardware::HackRf).is_none());
    }

    #[test]
    fn dependency_plan_leandvb_has_expected_verify_command() {
        let plan = dependency_install_plan("leandvb");
        assert!(plan
            .verify_commands
            .iter()
            .any(|command| command == "leandvb"));
        assert!(plan
            .source_install_command
            .as_deref()
            .unwrap_or_default()
            .contains("github.com/pabr/leansdr"));
    }

    #[test]
    fn plugin_dependency_descriptors_add_plugin_specific_tools() {
        let plugin_defs = vec![
            SdrPluginDefinition {
                id: "freedv".to_string(),
                label: "FreeDV".to_string(),
                command_template: "freedv_rx".to_string(),
                protocol: Some("freedv".to_string()),
            },
            SdrPluginDefinition {
                id: "dvb_s".to_string(),
                label: "DVB-S".to_string(),
                command_template: "leandvb".to_string(),
                protocol: Some("dvb_s".to_string()),
            },
            SdrPluginDefinition {
                id: "lte_meta".to_string(),
                label: "LTE".to_string(),
                command_template: "srsue".to_string(),
                protocol: Some("lte".to_string()),
            },
        ];
        let descriptors = dependency_descriptors_with_plugins(&plugin_defs);
        assert!(descriptors.iter().any(|descriptor| {
            descriptor.tool == "FreeDV decoder" && descriptor.package_hint == "freedv"
        }));
        assert!(descriptors.iter().any(|descriptor| {
            descriptor.tool == "DVB-S/DVB-S2 decoder" && descriptor.package_hint == "leandvb"
        }));
        assert!(descriptors.iter().any(|descriptor| {
            descriptor.tool == "LTE metadata scanner" && descriptor.package_hint == "srsran"
        }));
    }

    #[test]
    fn duplicate_plugin_dependency_descriptors_are_deduplicated() {
        let plugin_defs = vec![
            SdrPluginDefinition {
                id: "dvb_s".to_string(),
                label: "DVB-S".to_string(),
                command_template: "leandvb".to_string(),
                protocol: Some("dvb_s".to_string()),
            },
            SdrPluginDefinition {
                id: "dvb_s2".to_string(),
                label: "DVB-S2".to_string(),
                command_template: "leandvb".to_string(),
                protocol: Some("dvb_s2".to_string()),
            },
        ];
        let descriptors = dependency_descriptors_with_plugins(&plugin_defs);
        let count = descriptors
            .iter()
            .filter(|descriptor| descriptor.package_hint == "leandvb")
            .count();
        assert_eq!(count, 1);
    }
}

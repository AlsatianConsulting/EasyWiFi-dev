use anyhow::Result;
use chrono::{DateTime, Utc};
use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SdrHardware {
    Disabled,
}

impl Default for SdrHardware {
    fn default() -> Self {
        Self::Disabled
    }
}

impl SdrHardware {
    pub fn label(&self) -> &'static str {
        "Disabled"
    }

    pub fn id(&self) -> &'static str {
        "disabled"
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
    pub satcom_parse_denylist: Vec<String>,
    pub use_zulu_time: bool,
}

impl Default for SdrConfig {
    fn default() -> Self {
        Self {
            hardware: SdrHardware::Disabled,
            center_freq_hz: 433_920_000,
            sample_rate_hz: 2_400_000,
            fft_bins: 256,
            refresh_ms: 260,
            log_output_enabled: false,
            log_output_dir: std::env::temp_dir().join("easywifi-disabled-radio"),
            plugin_config_path: default_plugin_config_path(),
            scan_range_enabled: false,
            scan_start_hz: 0,
            scan_end_hz: 0,
            scan_step_hz: 0,
            scan_steps_per_sec: 0.0,
            squelch_dbm: -90.0,
            auto_tune_decoders: false,
            bias_tee_enabled: false,
            no_payload_satcom: true,
            satcom_parse_denylist: Vec::new(),
            use_zulu_time: false,
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
pub struct SdrAircraftCorrelation {
    pub key: String,
    pub icao_hex: Option<String>,
    pub callsign: Option<String>,
    pub adsb_rows: u64,
    pub acars_rows: u64,
    pub total_rows: u64,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub frequencies_hz: Vec<u64>,
    pub decoders: Vec<String>,
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
    pub payload_capture_mode: String,
    pub has_coordinates: bool,
    pub identifier_hints: Vec<String>,
    pub payload_parse_state: String,
    pub payload_fields: HashMap<String, String>,
    pub summary: String,
    pub message: String,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SdrDecoderTelemetry {
    pub timestamp: DateTime<Utc>,
    pub decoder: String,
    pub decoded_rows: u64,
    pub map_points: u64,
    pub satcom_rows: u64,
    pub stderr_lines: u64,
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
    DecoderTelemetry(SdrDecoderTelemetry),
}

#[derive(Debug, Clone)]
pub enum SdrDecoderKind {
    Rtl433,
    Adsb,
    Acars,
    Ais,
    AprsAx25,
    Pocsag,
    Iridium,
    InmarsatStdc,
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
            Self::AprsAx25 => "aprs_ax25".to_string(),
            Self::Pocsag => "pocsag".to_string(),
            Self::Iridium => "iridium".to_string(),
            Self::InmarsatStdc => "inmarsat_stdc".to_string(),
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
            Self::AprsAx25 => "APRS / AX.25".to_string(),
            Self::Pocsag => "POCSAG".to_string(),
            Self::Iridium => "Iridium".to_string(),
            Self::InmarsatStdc => "Inmarsat STD-C".to_string(),
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
            Self::AprsAx25 => "aprs_ax25",
            Self::Pocsag => "pocsag",
            Self::Iridium => "iridium",
            Self::InmarsatStdc => "inmarsat_c",
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

pub struct SdrRuntime {
    stop_flag: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl SdrRuntime {
    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    pub fn set_center_freq(&self, _freq_hz: u64) {}
    pub fn set_sweep_paused(&self, _paused: bool) {}
    pub fn set_logging(&self, _enabled: bool, _output_dir: PathBuf) {}
    pub fn set_scan_range(
        &self,
        _enabled: bool,
        _start_hz: u64,
        _end_hz: u64,
        _step_hz: u64,
        _steps_per_sec: f64,
    ) {
    }
    pub fn set_squelch(&self, _squelch_dbm: f32) {}
    pub fn set_auto_tune(&self, _enabled: bool) {}
    pub fn set_bias_tee(&self, _enabled: bool) {}
    pub fn set_no_payload_satcom(&self, _enabled: bool) {}
    pub fn set_satcom_payload_capture(&self, _enabled: bool) {}
    pub fn set_satcom_parse_denylist(&self, _denylist: Vec<String>) {}
    pub fn capture_sample(&self, _duration_secs: u32, _output_dir: PathBuf) {}
    pub fn start_decode(&self, _decoder: SdrDecoderKind) {}
    pub fn stop_decode(&self) {}
    pub fn refresh_dependencies(&self) {}
    pub fn install_missing_dependencies(&self) {}
}

pub fn default_plugin_config_path() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate = root.join("sdr-plugins.json");
    candidate.exists().then_some(candidate)
}

pub fn load_plugin_definitions(_path: Option<&Path>) -> Vec<SdrPluginDefinition> {
    Vec::new()
}

pub fn builtin_decoders_in_priority_order() -> Vec<SdrDecoderKind> {
    vec![
        SdrDecoderKind::Rtl433,
        SdrDecoderKind::Adsb,
        SdrDecoderKind::Acars,
        SdrDecoderKind::Ais,
        SdrDecoderKind::AprsAx25,
        SdrDecoderKind::Pocsag,
        SdrDecoderKind::Iridium,
        SdrDecoderKind::InmarsatStdc,
        SdrDecoderKind::Dect,
        SdrDecoderKind::GsmLte,
    ]
}

pub fn dependency_status_snapshot(
    _plugin_defs: &[SdrPluginDefinition],
) -> Vec<SdrDependencyStatus> {
    vec![SdrDependencyStatus {
        tool: "disabled".to_string(),
        package_hint: "not applicable".to_string(),
        installed: true,
    }]
}

pub fn decoder_command_preview(
    _decoder: &SdrDecoderKind,
    _freq_hz: u64,
    _sample_rate_hz: u32,
    _hardware: SdrHardware,
    _plugins: &[SdrPluginDefinition],
) -> Option<String> {
    None
}

pub fn decoder_unavailability_hint(
    _decoder: &SdrDecoderKind,
    _hardware: SdrHardware,
) -> Option<String> {
    Some("radio features are removed from this build".to_string())
}

pub fn start_runtime(config: SdrConfig, sender: Sender<SdrEvent>) -> SdrRuntime {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&stop_flag);

    let handle = thread::spawn(move || {
        let _ = sender.send(SdrEvent::Log(
            "radio features are disabled in this build".to_string(),
        ));
        let _ = sender.send(SdrEvent::FrequencyChanged(config.center_freq_hz));
        while !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(250));
        }
    });

    SdrRuntime {
        stop_flag,
        handle: Some(handle),
    }
}

pub fn decoder_launch_unavailable_reason(
    _decoder: &SdrDecoderKind,
    _freq_hz: u64,
    _sample_rate_hz: u32,
    _hardware: SdrHardware,
    _plugin_defs: &[SdrPluginDefinition],
) -> Option<String> {
    Some("radio features are removed from this build".to_string())
}

pub fn correlate_aircraft(_rows: &[SdrDecodeRow]) -> Vec<SdrAircraftCorrelation> {
    Vec::new()
}

pub fn refresh_dependency_state() -> Result<()> {
    Ok(())
}

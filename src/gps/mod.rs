use crate::settings::{GpsSettings, StreamProtocol};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct GpsFix {
    pub timestamp: DateTime<Utc>,
    pub latitude: f64,
    pub longitude: f64,
    pub altitude_m: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct GpsProviderStatus {
    pub mode: String,
    pub connected: bool,
    pub last_fix_timestamp: Option<DateTime<Utc>>,
    pub detail: String,
}

pub trait GpsProvider: Send + Sync {
    fn current_fix(&self) -> Option<GpsFix>;
    fn shutdown(&self);
    fn status(&self) -> GpsProviderStatus;
}

pub struct DisabledGps;

impl GpsProvider for DisabledGps {
    fn current_fix(&self) -> Option<GpsFix> {
        None
    }

    fn shutdown(&self) {}

    fn status(&self) -> GpsProviderStatus {
        GpsProviderStatus {
            mode: "Disabled".to_string(),
            connected: false,
            last_fix_timestamp: None,
            detail: "GPS disabled".to_string(),
        }
    }
}

pub struct StaticGps {
    fix: GpsFix,
}

impl StaticGps {
    pub fn new(latitude: f64, longitude: f64, altitude_m: Option<f64>) -> Self {
        Self {
            fix: GpsFix {
                timestamp: Utc::now(),
                latitude,
                longitude,
                altitude_m,
            },
        }
    }
}

impl GpsProvider for StaticGps {
    fn current_fix(&self) -> Option<GpsFix> {
        Some(self.fix.clone())
    }

    fn shutdown(&self) {}

    fn status(&self) -> GpsProviderStatus {
        GpsProviderStatus {
            mode: "Static".to_string(),
            connected: true,
            last_fix_timestamp: Some(self.fix.timestamp),
            detail: "Using configured static location".to_string(),
        }
    }
}

pub struct NmeaStreamGps {
    latest_fix: Arc<Mutex<Option<GpsFix>>>,
    connected: Arc<AtomicBool>,
    mode: String,
    detail: String,
    running: Arc<AtomicBool>,
    handle: Mutex<Option<thread::JoinHandle<()>>>,
}

impl NmeaStreamGps {
    pub fn from_interface(device_path: &str) -> Result<Self> {
        let file = File::open(device_path)
            .with_context(|| format!("failed to open GPS interface {}", device_path))?;
        Self::spawn_reader(
            move || Ok(Box::new(BufReader::new(file)) as Box<dyn BufRead + Send>),
            "Interface".to_string(),
            device_path.to_string(),
            true,
        )
    }

    pub fn from_tcp(host: &str, port: u16) -> Result<Self> {
        let addr = format!("{}:{}", host, port);
        let addr_for_reader = addr.clone();
        Self::spawn_reader(
            move || {
                let stream = TcpStream::connect(&addr_for_reader).with_context(|| {
                    format!("failed to connect to TCP NMEA stream {}", addr_for_reader)
                })?;
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .context("failed to set TCP read timeout")?;
                Ok(Box::new(BufReader::new(stream)) as Box<dyn BufRead + Send>)
            },
            "Stream/TCP".to_string(),
            addr,
            true,
        )
    }

    pub fn from_udp(host: &str, port: u16) -> Result<Self> {
        let addr = format!("{}:{}", host, port);
        let socket = UdpSocket::bind("0.0.0.0:0").context("failed to bind UDP socket")?;
        socket
            .connect(&addr)
            .with_context(|| format!("failed to connect UDP stream {}", addr))?;
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .context("failed to set UDP read timeout")?;

        let latest_fix = Arc::new(Mutex::new(None));
        let connected = Arc::new(AtomicBool::new(true));
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);
        let latest_clone = Arc::clone(&latest_fix);
        let connected_clone = Arc::clone(&connected);

        let handle = thread::spawn(move || {
            let mut buf = [0_u8; 4096];
            while running_clone.load(Ordering::Relaxed) {
                if let Ok(len) = socket.recv(&mut buf) {
                    connected_clone.store(true, Ordering::Relaxed);
                    let text = String::from_utf8_lossy(&buf[..len]).to_string();
                    for line in text.lines() {
                        if let Some(fix) = parse_nmea_line(line) {
                            *latest_clone.lock() = Some(fix);
                        }
                    }
                }
            }
        });

        Ok(Self {
            latest_fix,
            connected,
            mode: "Stream/UDP".to_string(),
            detail: addr,
            running,
            handle: Mutex::new(Some(handle)),
        })
    }

    pub fn from_gpsd(host: &str, port: u16) -> Result<Self> {
        // Fallback path when GPSD is configured for raw NMEA relay.
        Self::from_tcp(host, port)
    }

    fn spawn_reader<F>(
        reader_factory: F,
        mode: String,
        detail: String,
        initially_connected: bool,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<Box<dyn BufRead + Send>> + Send + 'static,
    {
        let latest_fix = Arc::new(Mutex::new(None));
        let connected = Arc::new(AtomicBool::new(initially_connected));
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);
        let latest_clone = Arc::clone(&latest_fix);
        let connected_clone = Arc::clone(&connected);

        let handle = thread::spawn(move || {
            let mut reader = match reader_factory() {
                Ok(r) => {
                    connected_clone.store(true, Ordering::Relaxed);
                    r
                }
                Err(_) => {
                    connected_clone.store(false, Ordering::Relaxed);
                    return;
                }
            };

            let mut line = String::new();
            while running_clone.load(Ordering::Relaxed) {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        connected_clone.store(false, Ordering::Relaxed);
                        thread::sleep(Duration::from_millis(100));
                    }
                    Ok(_) => {
                        connected_clone.store(true, Ordering::Relaxed);
                        if let Some(fix) = parse_nmea_line(line.trim()) {
                            *latest_clone.lock() = Some(fix);
                        }
                    }
                    Err(_) => {
                        connected_clone.store(false, Ordering::Relaxed);
                        thread::sleep(Duration::from_millis(250));
                    }
                }
            }
        });

        Ok(Self {
            latest_fix,
            connected,
            mode,
            detail,
            running,
            handle: Mutex::new(Some(handle)),
        })
    }
}

impl GpsProvider for NmeaStreamGps {
    fn current_fix(&self) -> Option<GpsFix> {
        self.latest_fix.lock().clone()
    }

    fn shutdown(&self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.lock().take() {
            let _ = handle.join();
        }
    }

    fn status(&self) -> GpsProviderStatus {
        let last_fix_timestamp = self.latest_fix.lock().as_ref().map(|f| f.timestamp);
        GpsProviderStatus {
            mode: self.mode.clone(),
            connected: self.connected.load(Ordering::Relaxed),
            last_fix_timestamp,
            detail: self.detail.clone(),
        }
    }
}

impl Drop for NmeaStreamGps {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub struct GpsdJsonGps {
    latest_fix: Arc<Mutex<Option<GpsFix>>>,
    connected: Arc<AtomicBool>,
    endpoint: String,
    last_error: Arc<Mutex<Option<String>>>,
    running: Arc<AtomicBool>,
    handle: Mutex<Option<thread::JoinHandle<()>>>,
}

impl GpsdJsonGps {
    pub fn connect(host: &str, port: u16) -> Result<Self> {
        let addr = format!("{}:{}", host, port);
        let stream = TcpStream::connect(&addr)
            .with_context(|| format!("failed to connect to GPSD {}", addr))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .context("failed to set GPSD read timeout")?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .context("failed to set GPSD write timeout")?;

        let latest_fix = Arc::new(Mutex::new(None));
        let connected = Arc::new(AtomicBool::new(false));
        let endpoint = addr.clone();
        let last_error = Arc::new(Mutex::new(None));
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);
        let latest_clone = Arc::clone(&latest_fix);
        let connected_clone = Arc::clone(&connected);
        let last_error_clone = Arc::clone(&last_error);

        let handle = thread::spawn(move || {
            let mut stream = stream;
            if stream
                .write_all(b"?WATCH={\"enable\":true,\"json\":true};\n")
                .is_err()
            {
                connected_clone.store(false, Ordering::Relaxed);
                *last_error_clone.lock() = Some("failed to send GPSD WATCH command".to_string());
                return;
            }
            let _ = stream.flush();
            connected_clone.store(true, Ordering::Relaxed);

            let mut reader = BufReader::new(stream);
            let mut line = String::new();

            while running_clone.load(Ordering::Relaxed) {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        connected_clone.store(false, Ordering::Relaxed);
                        *last_error_clone.lock() = Some("GPSD stream closed".to_string());
                        thread::sleep(Duration::from_millis(100));
                    }
                    Ok(_) => {
                        connected_clone.store(true, Ordering::Relaxed);
                        let text = line.trim();
                        if let Some(fix) = parse_gpsd_json_line(text) {
                            *latest_clone.lock() = Some(fix);
                            *last_error_clone.lock() = None;
                        }
                    }
                    Err(err) => {
                        connected_clone.store(false, Ordering::Relaxed);
                        *last_error_clone.lock() = Some(format!("GPSD read error: {}", err));
                        thread::sleep(Duration::from_millis(250));
                    }
                }
            }
        });

        Ok(Self {
            latest_fix,
            connected,
            endpoint,
            last_error,
            running,
            handle: Mutex::new(Some(handle)),
        })
    }
}

impl GpsProvider for GpsdJsonGps {
    fn current_fix(&self) -> Option<GpsFix> {
        self.latest_fix.lock().clone()
    }

    fn shutdown(&self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.lock().take() {
            let _ = handle.join();
        }
    }

    fn status(&self) -> GpsProviderStatus {
        let last_fix_timestamp = self.latest_fix.lock().as_ref().map(|f| f.timestamp);
        let error = self.last_error.lock().clone();
        GpsProviderStatus {
            mode: "GPSD".to_string(),
            connected: self.connected.load(Ordering::Relaxed),
            last_fix_timestamp,
            detail: match error {
                Some(e) => format!("{} ({})", self.endpoint, e),
                None => self.endpoint.clone(),
            },
        }
    }
}

impl Drop for GpsdJsonGps {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub struct FailedGps {
    mode: String,
    detail: String,
}

impl FailedGps {
    pub fn new(mode: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            mode: mode.into(),
            detail: detail.into(),
        }
    }
}

impl GpsProvider for FailedGps {
    fn current_fix(&self) -> Option<GpsFix> {
        None
    }

    fn shutdown(&self) {}

    fn status(&self) -> GpsProviderStatus {
        GpsProviderStatus {
            mode: self.mode.clone(),
            connected: false,
            last_fix_timestamp: None,
            detail: self.detail.clone(),
        }
    }
}

pub fn create_provider(settings: &GpsSettings) -> Box<dyn GpsProvider> {
    match settings {
        GpsSettings::Disabled => Box::new(DisabledGps),
        GpsSettings::Static {
            latitude,
            longitude,
            altitude_m,
        } => Box::new(StaticGps::new(*latitude, *longitude, *altitude_m)),
        GpsSettings::Interface { device_path } => {
            match NmeaStreamGps::from_interface(device_path) {
                Ok(provider) => Box::new(provider),
                Err(err) => Box::new(FailedGps::new(
                    "Interface",
                    format!("init failed for {}: {}", device_path, err),
                )),
            }
        }
        GpsSettings::Gpsd { host, port } => match GpsdJsonGps::connect(host, *port) {
            Ok(provider) => Box::new(provider),
            Err(err) => Box::new(FailedGps::new(
                "GPSD",
                format!("init failed for {}:{}: {}", host, port, err),
            )),
        },
        GpsSettings::Stream {
            protocol,
            host,
            port,
        } => match protocol {
            StreamProtocol::Tcp => match NmeaStreamGps::from_tcp(host, *port) {
                Ok(provider) => Box::new(provider),
                Err(err) => Box::new(FailedGps::new(
                    "Stream/TCP",
                    format!("init failed for {}:{}: {}", host, port, err),
                )),
            },
            StreamProtocol::Udp => match NmeaStreamGps::from_udp(host, *port) {
                Ok(provider) => Box::new(provider),
                Err(err) => Box::new(FailedGps::new(
                    "Stream/UDP",
                    format!("init failed for {}:{}: {}", host, port, err),
                )),
            },
        },
    }
}

fn parse_nmea_line(line: &str) -> Option<GpsFix> {
    if line.starts_with("$GPGGA") || line.starts_with("$GNGGA") {
        return parse_gga(line);
    }
    if line.starts_with("$GPRMC") || line.starts_with("$GNRMC") {
        return parse_rmc(line);
    }
    None
}

fn parse_gpsd_json_line(line: &str) -> Option<GpsFix> {
    let json: serde_json::Value = serde_json::from_str(line).ok()?;
    let class = json.get("class")?.as_str()?;
    if class != "TPV" {
        return None;
    }

    let latitude = json.get("lat").and_then(serde_json::Value::as_f64)?;
    let longitude = json.get("lon").and_then(serde_json::Value::as_f64)?;
    let altitude_m = json
        .get("altMSL")
        .or_else(|| json.get("alt"))
        .and_then(serde_json::Value::as_f64);

    let timestamp = json
        .get("time")
        .and_then(serde_json::Value::as_str)
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    Some(GpsFix {
        timestamp,
        latitude,
        longitude,
        altitude_m,
    })
}

fn parse_gga(line: &str) -> Option<GpsFix> {
    let parts: Vec<&str> = line.split(',').collect();
    if parts.len() < 10 {
        return None;
    }

    let lat = parse_nmea_lat(parts.get(2).copied()?, parts.get(3).copied()?)?;
    let lon = parse_nmea_lon(parts.get(4).copied()?, parts.get(5).copied()?)?;
    let altitude_m = parts.get(9).and_then(|v| v.parse::<f64>().ok());

    Some(GpsFix {
        timestamp: Utc::now(),
        latitude: lat,
        longitude: lon,
        altitude_m,
    })
}

fn parse_rmc(line: &str) -> Option<GpsFix> {
    let parts: Vec<&str> = line.split(',').collect();
    if parts.len() < 7 {
        return None;
    }

    let lat = parse_nmea_lat(parts.get(3).copied()?, parts.get(4).copied()?)?;
    let lon = parse_nmea_lon(parts.get(5).copied()?, parts.get(6).copied()?)?;

    Some(GpsFix {
        timestamp: Utc::now(),
        latitude: lat,
        longitude: lon,
        altitude_m: None,
    })
}

fn parse_nmea_lat(value: &str, hemisphere: &str) -> Option<f64> {
    if value.len() < 4 {
        return None;
    }
    let (deg, min) = value.split_at(2);
    let deg = deg.parse::<f64>().ok()?;
    let min = min.parse::<f64>().ok()?;
    let mut result = deg + min / 60.0;
    if hemisphere.eq_ignore_ascii_case("S") {
        result = -result;
    }
    Some(result)
}

fn parse_nmea_lon(value: &str, hemisphere: &str) -> Option<f64> {
    if value.len() < 5 {
        return None;
    }
    let (deg, min) = value.split_at(3);
    let deg = deg.parse::<f64>().ok()?;
    let min = min.parse::<f64>().ok()?;
    let mut result = deg + min / 60.0;
    if hemisphere.eq_ignore_ascii_case("W") {
        result = -result;
    }
    Some(result)
}

pub fn read_nmea_from_reader<R: Read>(reader: R) -> Option<GpsFix> {
    let mut buffered = BufReader::new(reader);
    let mut line = String::new();
    while buffered.read_line(&mut line).ok()? > 0 {
        if let Some(fix) = parse_nmea_line(line.trim()) {
            return Some(fix);
        }
        line.clear();
    }
    None
}

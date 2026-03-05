use dirs::data_local_dir;
use maxminddb::Reader;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct GeoIpLookup {
    cache: HashMap<String, Option<String>>,
    reader: Option<Reader<Vec<u8>>>,
    source_path: Option<PathBuf>,
}

impl GeoIpLookup {
    pub fn new() -> Self {
        Self::with_preferred_path(None)
    }

    pub fn with_preferred_path(path: Option<&Path>) -> Self {
        for candidate in candidate_paths(path) {
            if !candidate.is_file() {
                continue;
            }
            if let Ok(reader) = Reader::open_readfile(&candidate) {
                return Self {
                    cache: HashMap::new(),
                    reader: Some(reader),
                    source_path: Some(candidate),
                };
            }
        }

        Self::default()
    }

    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    pub fn lookup_city_label(&mut self, ip: &str) -> Option<String> {
        if let Some(cached) = self.cache.get(ip) {
            return cached.clone();
        }

        let label = lookup_city_label_impl(self.reader.as_ref(), ip);
        self.cache.insert(ip.to_string(), label.clone());
        label
    }
}

fn candidate_paths(preferred_path: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = preferred_path {
        candidates.push(path.to_path_buf());
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("GeoLite2-City.mmdb"));
    if let Ok(path) = env::var("SIMPLESTG_GEOIP_CITY_DB") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(base) = data_local_dir() {
        candidates.push(base.join("SimpleSTG").join("GeoLite2-City.mmdb"));
        candidates.push(base.join("simplestg").join("GeoLite2-City.mmdb"));
    }
    candidates.push(PathBuf::from("/usr/share/GeoIP/GeoLite2-City.mmdb"));
    candidates.push(PathBuf::from("/usr/share/GeoIP/GeoIP2-City.mmdb"));
    candidates.push(PathBuf::from("/usr/local/share/GeoIP/GeoLite2-City.mmdb"));
    candidates.push(PathBuf::from("/usr/local/share/GeoIP/GeoIP2-City.mmdb"));
    candidates
}

#[cfg(test)]
mod tests {
    use super::GeoIpLookup;
    use std::path::PathBuf;

    #[test]
    fn project_root_mmdb_is_preferred_when_present() {
        let project_mmdb = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("GeoLite2-City.mmdb");
        let lookup = GeoIpLookup::with_preferred_path(Some(&project_mmdb));
        assert_eq!(lookup.source_path(), Some(project_mmdb.as_path()));
    }
}

fn lookup_city_label_impl(reader: Option<&Reader<Vec<u8>>>, ip: &str) -> Option<String> {
    let ip_addr: IpAddr = ip.parse().ok()?;
    if is_local_ip(ip_addr) {
        return Some("Private / local".to_string());
    }

    let reader = reader?;
    let record = reader.lookup::<GeoCityRecord>(ip_addr).ok()?;
    let city = record.city.and_then(|v| v.english_name());
    let region = record
        .subdivisions
        .and_then(|mut v| v.drain(..).next())
        .and_then(|v| v.english_name());
    let country = record.country.and_then(|v| v.iso_or_english_name());

    match (city, region, country) {
        (Some(city), _, Some(country)) => Some(format!("{city}, {country}")),
        (Some(city), _, None) => Some(city),
        (None, Some(region), Some(country)) => Some(format!("{region}, {country}")),
        (None, Some(region), None) => Some(region),
        (None, None, Some(country)) => Some(country),
        _ => None,
    }
}

fn is_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            ipv4.is_private()
                || ipv4.is_loopback()
                || ipv4.is_link_local()
                || ipv4.is_multicast()
                || ipv4.is_unspecified()
        }
        IpAddr::V6(ipv6) => {
            ipv6.is_loopback()
                || ipv6.is_multicast()
                || ipv6.is_unspecified()
                || ipv6.is_unique_local()
                || ipv6.is_unicast_link_local()
        }
    }
}

#[derive(Debug, Deserialize)]
struct GeoCityRecord {
    city: Option<GeoNameSet>,
    country: Option<GeoCountry>,
    subdivisions: Option<Vec<GeoNameSet>>,
}

#[derive(Debug, Deserialize)]
struct GeoNameSet {
    names: Option<HashMap<String, String>>,
}

impl GeoNameSet {
    fn english_name(self) -> Option<String> {
        self.names.and_then(|names| names.get("en").cloned())
    }
}

#[derive(Debug, Deserialize)]
struct GeoCountry {
    iso_code: Option<String>,
    names: Option<HashMap<String, String>>,
}

impl GeoCountry {
    fn iso_or_english_name(self) -> Option<String> {
        self.iso_code
            .or_else(|| self.names.and_then(|names| names.get("en").cloned()))
    }
}

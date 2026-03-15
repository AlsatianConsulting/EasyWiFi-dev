use crate::settings::default_oui_source_path;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct OuiDatabase {
    by_prefix_len: BTreeMap<usize, HashMap<String, String>>,
    entry_count: usize,
}

impl OuiDatabase {
    pub fn empty() -> Self {
        Self {
            by_prefix_len: BTreeMap::new(),
            entry_count: 0,
        }
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read OUI database at {}", path.display()))?;

        let first_meaningful = raw
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default();

        if first_meaningful.starts_with('#') || first_meaningful.contains('\t') {
            Self::load_from_manuf_text(&raw)
        } else {
            Self::load_from_csv_text(&raw)
        }
    }

    pub fn load_default() -> Result<Self> {
        Self::load_with_override(None)
    }

    pub fn load_with_override(path: Option<&Path>) -> Result<Self> {
        let mut candidates = Vec::new();
        if let Some(path) = path {
            candidates.push(path.to_path_buf());
        }
        candidates.extend(default_candidate_paths());

        for candidate in candidates {
            if !candidate.exists() {
                continue;
            }
            if let Ok(db) = Self::load_from_path(&candidate) {
                if db.entry_count > 0 {
                    return Ok(db);
                }
            }
        }

        Ok(Self::empty())
    }

    pub fn default_source_path() -> PathBuf {
        default_oui_source_path()
    }

    pub fn lookup(&self, mac_address: &str) -> Option<&str> {
        let normalized = normalize_hex(mac_address);
        for (prefix_len, bucket) in self.by_prefix_len.iter().rev() {
            if normalized.len() < *prefix_len {
                continue;
            }
            let key = normalized.chars().take(*prefix_len).collect::<String>();
            if let Some(vendor) = bucket.get(&key) {
                return Some(vendor.as_str());
            }
        }
        None
    }

    pub fn count(&self) -> usize {
        self.entry_count
    }

    pub fn refresh_from_ieee(&self, destination_path: &Path) -> Result<()> {
        let parent = destination_path.parent().with_context(|| {
            format!(
                "invalid OUI destination path {}",
                destination_path.display()
            )
        })?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;

        let url = "https://standards-oui.ieee.org/oui/oui.csv";
        let response = ureq::get(url)
            .set(
                "User-Agent",
                "WirelessExplorer/0.1 (+offline OUI cache update)",
            )
            .set("Accept", "text/csv,text/plain;q=0.9,*/*;q=0.8")
            .call()
            .context("failed to fetch latest IEEE OUI list")?;

        let text = response
            .into_string()
            .context("failed to read IEEE OUI response body")?;

        fs::write(destination_path, text)
            .with_context(|| format!("failed to write {}", destination_path.display()))?;

        Ok(())
    }

    pub fn persistent_cache_path() -> Option<PathBuf> {
        persistent_oui_path()
    }

    fn load_from_csv_text(raw: &str) -> Result<Self> {
        let mut by_prefix_len: BTreeMap<usize, HashMap<String, String>> = BTreeMap::new();
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(true)
            .from_reader(raw.as_bytes());

        let mut entry_count = 0usize;
        for row in rdr.records() {
            let row = row?;
            if row.len() < 2 {
                continue;
            }
            let prefix = normalize_hex(&row[0]);
            let vendor = row[1].trim().to_string();
            if prefix.is_empty() || vendor.is_empty() {
                continue;
            }
            by_prefix_len
                .entry(prefix.len())
                .or_default()
                .insert(prefix, vendor);
            entry_count += 1;
        }

        Ok(Self {
            by_prefix_len,
            entry_count,
        })
    }

    fn load_from_manuf_text(raw: &str) -> Result<Self> {
        let mut by_prefix_len: BTreeMap<usize, HashMap<String, String>> = BTreeMap::new();
        let mut entry_count = 0usize;

        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            let columns = trimmed.split('\t').collect::<Vec<_>>();
            if columns.len() < 2 {
                continue;
            }

            let prefix_token = columns[0].trim();
            let vendor = columns
                .get(2)
                .copied()
                .or_else(|| columns.get(1).copied())
                .unwrap_or("")
                .trim();
            if vendor.is_empty() {
                continue;
            }

            let (prefix_hex, prefix_len) = parse_manuf_prefix(prefix_token);
            if prefix_hex.is_empty() || prefix_len == 0 {
                continue;
            }

            by_prefix_len
                .entry(prefix_len)
                .or_default()
                .insert(prefix_hex, vendor.to_string());
            entry_count += 1;
        }

        Ok(Self {
            by_prefix_len,
            entry_count,
        })
    }
}

fn normalize_hex(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .flat_map(|c| c.to_uppercase())
        .collect::<String>()
}

fn parse_manuf_prefix(token: &str) -> (String, usize) {
    let (raw_prefix, prefix_bits) = match token.split_once('/') {
        Some((prefix, bits)) => (prefix, bits.trim().parse::<usize>().ok()),
        None => (token, None),
    };

    let normalized = normalize_hex(raw_prefix);
    if normalized.is_empty() {
        return (String::new(), 0);
    }

    let prefix_len = prefix_bits
        .map(|bits| bits.div_ceil(4))
        .unwrap_or(normalized.len())
        .min(normalized.len());

    (
        normalized.chars().take(prefix_len).collect::<String>(),
        prefix_len,
    )
}

fn persistent_oui_path() -> Option<PathBuf> {
    let base = dirs::data_local_dir()?;
    Some(base.join("WirelessExplorer").join("oui.csv"))
}

fn default_candidate_paths() -> Vec<PathBuf> {
    let mut candidates = vec![
        default_oui_source_path(),
        PathBuf::from("/usr/share/wirelessexplorer/manuf"),
        PathBuf::from("/usr/share/wirelessexplorer/oui.csv"),
        PathBuf::from("/usr/share/wirelessexplorer/assets/oui.csv"),
        PathBuf::from("/usr/share/WirelessExplorer/manuf"),
        PathBuf::from("/usr/share/WirelessExplorer/oui.csv"),
        PathBuf::from("/usr/share/WirelessExplorer/assets/oui.csv"),
        PathBuf::from("/usr/share/wireshark/manuf"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/oui.csv"),
    ];
    if let Some(cache) = persistent_oui_path() {
        candidates.push(cache);
    }
    if let Some(base) = dirs::data_local_dir() {
        candidates.push(base.join("SimpleSTG").join("oui.csv"));
        candidates.push(base.join("simplestg").join("oui.csv"));
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::OuiDatabase;
    use crate::settings::default_oui_source_path;

    #[test]
    fn parses_csv_fallback_and_looks_up_vendor() {
        let raw = "prefix,vendor\n00163E,Cisco-Linksys\nF4F5E8,Intel Corporate\n";
        let db = OuiDatabase::load_from_csv_text(raw).expect("csv load");
        assert_eq!(db.count(), 2);
        assert_eq!(db.lookup("00:16:3E:AA:BB:CC"), Some("Cisco-Linksys"));
        assert_eq!(db.lookup("F4-F5-E8-01-02-03"), Some("Intel Corporate"));
    }

    #[test]
    fn prefers_longest_prefix_from_manuf_format() {
        let raw = "\
# comment
00:1B:C5\tGenericVendor\tGeneric Vendor Inc.\n\
00:1B:C5:00:10:00/36\tOpenRBco\tOpenRB.com, Direct SIA\n";
        let db = OuiDatabase::load_from_manuf_text(raw).expect("manuf load");
        assert_eq!(db.count(), 2);
        assert_eq!(
            db.lookup("00:1B:C5:00:10:AA"),
            Some("OpenRB.com, Direct SIA")
        );
        assert_eq!(db.lookup("00:1B:C5:AA:BB:CC"), Some("Generic Vendor Inc."));
    }

    #[test]
    fn loads_default_project_source_override() {
        let path = default_oui_source_path();
        let db = OuiDatabase::load_with_override(Some(&path)).expect("default oui load");
        assert!(
            db.count() > 1000,
            "expected a real OUI source at {}",
            path.display()
        );
    }
}

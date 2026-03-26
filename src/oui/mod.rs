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
        if let Some(vendor) = self.lookup_normalized(&normalized) {
            return Some(vendor);
        }

        // Locally administered/derived BSSIDs may flip bit 1 in the first octet.
        // Try the globally administered variant as a fallback for OUI vendor mapping.
        if normalized.len() >= 2 {
            if let Ok(first_octet) = u8::from_str_radix(&normalized[..2], 16) {
                if first_octet & 0x02 != 0 {
                    let fallback = format!("{:02X}{}", first_octet & !0x02, &normalized[2..]);
                    if let Some(vendor) = self.lookup_normalized(&fallback) {
                        return Some(vendor);
                    }
                }
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

        let urls = [
            "https://standards-oui.ieee.org/oui/oui.csv",
            "http://standards-oui.ieee.org/oui/oui.csv",
        ];
        let mut text = None::<String>;
        let mut fetch_errors = Vec::new();
        for url in urls {
            match ureq::get(url)
                .set("User-Agent", "EasyWiFi/0.1 (+offline OUI cache update)")
                .set("Accept", "text/csv,text/plain;q=0.9,*/*;q=0.8")
                .call()
            {
                Ok(response) => match response.into_string() {
                    Ok(body) if !body.trim().is_empty() => {
                        text = Some(body);
                        break;
                    }
                    Ok(_) => fetch_errors.push(format!("{url}: empty response body")),
                    Err(err) => fetch_errors.push(format!("{url}: {err}")),
                },
                Err(err) => fetch_errors.push(format!("{url}: {err}")),
            }
        }
        let text = text.ok_or_else(|| {
            anyhow::anyhow!(
                "failed to fetch latest IEEE OUI list via known endpoints: {}",
                fetch_errors.join(" | ")
            )
        })?;

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
        let headers = rdr
            .headers()
            .cloned()
            .unwrap_or_else(|_| csv::StringRecord::new());
        let prefix_index = csv_column_index(
            &headers,
            &[
                "assignment",
                "prefix",
                "oui",
                "macprefix",
                "ouiprefix",
                "ouiassignment",
            ],
        );
        let vendor_index = csv_column_index(
            &headers,
            &[
                "organizationname",
                "organisationname",
                "vendor",
                "manufacturer",
                "company",
                "companyname",
                "name",
            ],
        );

        let mut fallback_prefix = 0usize;
        let mut fallback_vendor = if headers.len() > 2 { 2 } else { 1 };
        if headers.is_empty() {
            fallback_prefix = 0;
            fallback_vendor = 1;
        } else if headers.len() == 1 {
            fallback_prefix = 0;
            fallback_vendor = 0;
        } else if fallback_vendor >= headers.len() {
            fallback_vendor = headers.len().saturating_sub(1);
        }
        let prefix_index = prefix_index.unwrap_or(fallback_prefix);
        let vendor_index = vendor_index.unwrap_or(fallback_vendor);

        let mut entry_count = 0usize;
        for row in rdr.records() {
            let row = row?;
            if row.len() < 2 || prefix_index >= row.len() || vendor_index >= row.len() {
                continue;
            }
            let prefix = normalize_hex(row.get(prefix_index).unwrap_or_default());
            let vendor = row
                .get(vendor_index)
                .unwrap_or_default()
                .trim()
                .trim_matches('"')
                .to_string();
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

    fn lookup_normalized(&self, normalized: &str) -> Option<&str> {
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

fn csv_column_index(headers: &csv::StringRecord, aliases: &[&str]) -> Option<usize> {
    headers.iter().enumerate().find_map(|(index, header)| {
        let normalized = normalize_csv_header(header);
        aliases.contains(&normalized.as_str()).then_some(index)
    })
}

fn normalize_csv_header(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect::<String>()
}

fn persistent_oui_path() -> Option<PathBuf> {
    let base = dirs::data_local_dir()?;
    Some(base.join("EasyWiFi").join("oui.csv"))
}

fn default_candidate_paths() -> Vec<PathBuf> {
    let mut candidates = vec![
        default_oui_source_path(),
        PathBuf::from("/usr/share/easywifi/manuf"),
        PathBuf::from("/usr/share/easywifi/oui.csv"),
        PathBuf::from("/usr/share/easywifi/assets/oui.csv"),
        PathBuf::from("/usr/share/EasyWiFi/manuf"),
        PathBuf::from("/usr/share/EasyWiFi/oui.csv"),
        PathBuf::from("/usr/share/EasyWiFi/assets/oui.csv"),
        PathBuf::from("/usr/share/wireshark/manuf"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/oui.csv"),
    ];
    if let Some(cache) = persistent_oui_path() {
        candidates.push(cache);
    }
    if let Some(base) = dirs::data_local_dir() {
        candidates.push(base.join("EasyWiFi").join("oui.csv"));
        candidates.push(base.join("easywifi").join("oui.csv"));
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
    fn parses_ieee_csv_assignment_and_org_name_columns() {
        let raw = "\
Registry,Assignment,Organization Name,Organization Address\n\
MA-L,286FB9,\"Nokia Shanghai Bell Co.,Ltd.\",\n\
MA-M,70B3D57,Acme Mid Prefix Labs,\n";
        let db = OuiDatabase::load_from_csv_text(raw).expect("ieee csv load");
        assert_eq!(db.count(), 2);
        assert_eq!(
            db.lookup("28:6F:B9:AA:BB:CC"),
            Some("Nokia Shanghai Bell Co.,Ltd.")
        );
        assert_eq!(db.lookup("70:B3:D5:7A:11:22"), Some("Acme Mid Prefix Labs"));
    }

    #[test]
    fn lookup_falls_back_to_globally_administered_prefix_for_local_mac() {
        let raw = "prefix,vendor\nC89E43,Example AP Vendor\n";
        let db = OuiDatabase::load_from_csv_text(raw).expect("csv load");
        assert_eq!(db.lookup("CA:9E:43:12:34:56"), Some("Example AP Vendor"));
    }

    #[test]
    fn loads_default_project_source_override() {
        let path = default_oui_source_path();
        let db = OuiDatabase::load_with_override(Some(&path)).expect("default oui load");
        assert!(db.count() > 0, "expected OUI entries at {}", path.display());
    }
}

use crate::model::{
    observation_highlights, AccessPointRecord, BluetoothDeviceRecord, ClientRecord, GeoObservation,
};
use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use pcap_file::pcapng::blocks::enhanced_packet::EnhancedPacketOption;
use pcap_file::pcapng::Block as PcapNgBlock;
use pcap_file::pcapng::{PcapNgReader, PcapNgWriter};
use serde_json::json;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use zip::write::FileOptions;

#[derive(Debug, Clone)]
pub struct ExportPaths {
    pub root: PathBuf,
    pub session_dir: PathBuf,
    pub csv_dir: PathBuf,
    pub json_dir: PathBuf,
    pub kml_dir: PathBuf,
    pub pcap_dir: PathBuf,
    pub handshakes_dir: PathBuf,
    pub logs_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ExportManager {
    pub paths: ExportPaths,
}

impl ExportManager {
    pub fn new(root: &Path, session_id: &str) -> Result<Self> {
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let session_dir = root.join(format!("session_{}_{}", timestamp, session_id));
        let csv_dir = session_dir.join("csv");
        let json_dir = session_dir.join("json");
        let kml_dir = session_dir.join("kml");
        let pcap_dir = session_dir.join("pcap");
        let handshakes_dir = pcap_dir.join("handshakes");
        let logs_dir = session_dir.join("logs");

        for dir in [
            &session_dir,
            &csv_dir,
            &json_dir,
            &kml_dir,
            &pcap_dir,
            &handshakes_dir,
            &logs_dir,
        ] {
            fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }

        Ok(Self {
            paths: ExportPaths {
                root: root.to_path_buf(),
                session_dir,
                csv_dir,
                json_dir,
                kml_dir,
                pcap_dir,
                handshakes_dir,
                logs_dir,
            },
        })
    }

    pub fn create_initial_outputs(&self) -> Result<()> {
        let ap_csv = self.paths.csv_dir.join("access_points.csv");
        let client_csv = self.paths.csv_dir.join("clients.csv");
        let ap_loc_csv = self.paths.csv_dir.join("access_point_locations.csv");
        let client_loc_csv = self.paths.csv_dir.join("client_locations.csv");
        let bt_loc_csv = self.paths.csv_dir.join("bluetooth_locations.csv");
        let summary_json = self.paths.json_dir.join("summary.json");
        let session_log = self.paths.logs_dir.join("session.log");
        let session_pcap = self.paths.pcap_dir.join("consolidated_capture.pcapng");
        let kml_ap = self
            .paths
            .kml_dir
            .join("access_points")
            .join("observations.kml");
        let kml_client = self.paths.kml_dir.join("clients").join("observations.kml");
        let kml_bt = self
            .paths
            .kml_dir
            .join("bluetooth")
            .join("observations.kml");

        if !ap_csv.exists() {
            let mut w = csv::Writer::from_path(&ap_csv)?;
            w.write_record([
                "SSID",
                "BSSID",
                "OUI Manufacturer",
                "802.11d Country",
                "Channel",
                "Encryption Type",
                "Number of Clients",
                "First Seen",
                "Last Seen",
                "Handshake Count",
                "Frequency MHz",
                "Full Encryption",
                "Notes",
                "Uptime Beacons",
            ])?;
            w.flush()?;
        }

        if !client_csv.exists() {
            let mut w = csv::Writer::from_path(&client_csv)?;
            w.write_record([
                "MAC",
                "OUI",
                "Associated AP",
                "RSSI",
                "Data Transferred",
                "Probes",
                "First Heard",
                "Last Heard",
            ])?;
            w.flush()?;
        }

        if !ap_loc_csv.exists() {
            let mut w = csv::Writer::from_path(&ap_loc_csv)?;
            w.write_record([
                "Date",
                "BSSID",
                "OUI",
                "SSID",
                "Channel",
                "Encryption",
                "RSSI",
                "Associated Clients",
                "Latitude",
                "Longitude",
                "Altitude M",
            ])?;
            w.flush()?;
        }

        if !client_loc_csv.exists() {
            let mut w = csv::Writer::from_path(&client_loc_csv)?;
            w.write_record([
                "Date",
                "MAC",
                "OUI",
                "Associated BSSID",
                "RSSI",
                "Latitude",
                "Longitude",
                "Altitude M",
            ])?;
            w.flush()?;
        }

        if !bt_loc_csv.exists() {
            let mut w = csv::Writer::from_path(&bt_loc_csv)?;
            w.write_record([
                "Date",
                "MAC",
                "OUI",
                "BT/BLE",
                "Device Type",
                "RSSI",
                "Latitude",
                "Longitude",
                "Altitude M",
            ])?;
            w.flush()?;
        }

        if !session_log.exists() {
            fs::write(&session_log, "session initialized\n")?;
        }

        if !summary_json.exists() {
            fs::write(
                &summary_json,
                serde_json::to_string_pretty(&json!({
                    "generated_at": Utc::now().to_rfc3339(),
                    "access_points": [],
                    "clients": [],
                    "bluetooth": [],
                }))?,
            )?;
        }

        if !session_pcap.exists() {
            // Placeholder file for startup output consistency.
            fs::write(&session_pcap, "")?;
        }

        for path in [&kml_ap, &kml_client, &kml_bt] {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            if !path.exists() {
                fs::write(path, kml_document(""))?;
            }
        }

        Ok(())
    }

    pub fn export_access_points_csv(&self, aps: &[AccessPointRecord]) -> Result<PathBuf> {
        let path = self.paths.csv_dir.join("access_points.csv");
        let mut wtr = csv::Writer::from_path(&path)?;
        wtr.write_record([
            "SSID",
            "BSSID",
            "OUI Manufacturer",
            "802.11d Country",
            "Channel",
            "Encryption Type",
            "Number of Clients",
            "First Seen",
            "Last Seen",
            "Handshake Count",
            "Frequency MHz",
            "Full Encryption",
            "Notes",
            "Uptime Beacons",
        ])?;

        for ap in aps {
            wtr.write_record(&[
                ap.ssid.clone().unwrap_or_else(|| "<hidden>".to_string()),
                ap.bssid.clone(),
                ap.oui_manufacturer.clone().unwrap_or_default(),
                ap.country_code_80211d.clone().unwrap_or_default(),
                ap.channel.map(|v| v.to_string()).unwrap_or_default(),
                ap.encryption_short.clone(),
                ap.number_of_clients.to_string(),
                ap.first_seen.to_rfc3339(),
                ap.last_seen.to_rfc3339(),
                ap.handshake_count.to_string(),
                ap.frequency_mhz.map(|v| v.to_string()).unwrap_or_default(),
                ap.encryption_full.clone(),
                ap.notes.clone().unwrap_or_default(),
                ap.uptime_beacons.map(|v| v.to_string()).unwrap_or_default(),
            ])?;
        }

        wtr.flush()?;
        Ok(path)
    }

    pub fn export_clients_csv(&self, clients: &[ClientRecord]) -> Result<PathBuf> {
        let path = self.paths.csv_dir.join("clients.csv");
        let mut wtr = csv::Writer::from_path(&path)?;
        wtr.write_record([
            "MAC",
            "OUI",
            "Associated AP",
            "RSSI",
            "Data Transferred",
            "Probes",
            "First Heard",
            "Last Heard",
        ])?;

        for client in clients {
            wtr.write_record(&[
                client.mac.clone(),
                client.oui_manufacturer.clone().unwrap_or_default(),
                client.associated_ap.clone().unwrap_or_default(),
                client.rssi_dbm.map(|v| v.to_string()).unwrap_or_default(),
                client.data_transferred_bytes.to_string(),
                client.probes.join(";"),
                client.first_seen.to_rfc3339(),
                client.last_seen.to_rfc3339(),
            ])?;
        }

        wtr.flush()?;
        Ok(path)
    }

    pub fn export_ap_detail_csv(&self, ap: &AccessPointRecord) -> Result<PathBuf> {
        let path = self
            .paths
            .csv_dir
            .join(format!("ap_{}_detail.csv", sanitize_name(&ap.bssid)));
        let locations = summarize_locations(&ap.observations);
        let mut w = csv::Writer::from_path(&path)?;
        w.write_record(["field", "value"])?;

        write_kv(&mut w, "ssid", ap.ssid.clone().unwrap_or_default())?;
        write_kv(&mut w, "bssid", ap.bssid.clone())?;
        write_kv(
            &mut w,
            "oui_manufacturer",
            ap.oui_manufacturer.clone().unwrap_or_default(),
        )?;
        write_kv(
            &mut w,
            "country_code_80211d",
            ap.country_code_80211d.clone().unwrap_or_default(),
        )?;
        write_kv(
            &mut w,
            "channel",
            ap.channel.map(|v| v.to_string()).unwrap_or_default(),
        )?;
        write_kv(
            &mut w,
            "frequency_mhz",
            ap.frequency_mhz.map(|v| v.to_string()).unwrap_or_default(),
        )?;
        write_kv(&mut w, "encryption_short", ap.encryption_short.clone())?;
        write_kv(&mut w, "encryption_full", ap.encryption_full.clone())?;
        write_kv(&mut w, "first_seen", ap.first_seen.to_rfc3339())?;
        write_kv(&mut w, "last_seen", ap.last_seen.to_rfc3339())?;
        write_kv(&mut w, "handshake_count", ap.handshake_count.to_string())?;
        write_kv(
            &mut w,
            "notes",
            ap.notes.clone().unwrap_or_else(String::new),
        )?;
        write_kv(
            &mut w,
            "uptime_beacons",
            ap.uptime_beacons.map(|v| v.to_string()).unwrap_or_default(),
        )?;
        write_kv(
            &mut w,
            "first_location",
            location_coords_string(locations.first.as_ref()),
        )?;
        write_kv(
            &mut w,
            "first_location_timestamp",
            location_timestamp_string(locations.first.as_ref()),
        )?;
        write_kv(
            &mut w,
            "last_location",
            location_coords_string(locations.last.as_ref()),
        )?;
        write_kv(
            &mut w,
            "last_location_timestamp",
            location_timestamp_string(locations.last.as_ref()),
        )?;
        write_kv(
            &mut w,
            "strongest_location",
            location_coords_string(locations.strongest.as_ref()),
        )?;
        write_kv(
            &mut w,
            "strongest_location_timestamp",
            location_timestamp_string(locations.strongest.as_ref()),
        )?;
        write_kv(
            &mut w,
            "strongest_location_rssi_dbm",
            locations
                .strongest
                .as_ref()
                .and_then(|obs| obs.rssi_dbm)
                .map(|v| v.to_string())
                .unwrap_or_default(),
        )?;

        w.flush()?;
        Ok(path)
    }

    pub fn export_client_detail_csv(&self, client: &ClientRecord) -> Result<PathBuf> {
        let path = self
            .paths
            .csv_dir
            .join(format!("client_{}_detail.csv", sanitize_name(&client.mac)));
        let locations = summarize_locations(&client.observations);
        let mut w = csv::Writer::from_path(&path)?;
        w.write_record(["field", "value"])?;

        write_kv(&mut w, "mac", client.mac.clone())?;
        write_kv(
            &mut w,
            "oui_manufacturer",
            client.oui_manufacturer.clone().unwrap_or_default(),
        )?;
        write_kv(
            &mut w,
            "associated_ap",
            client.associated_ap.clone().unwrap_or_default(),
        )?;
        write_kv(
            &mut w,
            "data_transferred_bytes",
            client.data_transferred_bytes.to_string(),
        )?;
        write_kv(
            &mut w,
            "rssi_dbm",
            client.rssi_dbm.map(|v| v.to_string()).unwrap_or_default(),
        )?;
        write_kv(&mut w, "probes", client.probes.join(";"))?;
        write_kv(&mut w, "first_seen", client.first_seen.to_rfc3339())?;
        write_kv(&mut w, "last_seen", client.last_seen.to_rfc3339())?;
        write_kv(
            &mut w,
            "band",
            client.network_intel.band.label().to_string(),
        )?;
        write_kv(
            &mut w,
            "last_channel",
            client
                .network_intel
                .last_channel
                .map(|v| v.to_string())
                .unwrap_or_default(),
        )?;
        write_kv(
            &mut w,
            "last_frequency_mhz",
            client
                .network_intel
                .last_frequency_mhz
                .map(|v| v.to_string())
                .unwrap_or_default(),
        )?;
        write_kv(
            &mut w,
            "uplink_bytes",
            client.network_intel.uplink_bytes.to_string(),
        )?;
        write_kv(
            &mut w,
            "downlink_bytes",
            client.network_intel.downlink_bytes.to_string(),
        )?;
        write_kv(
            &mut w,
            "retry_frame_count",
            client.network_intel.retry_frame_count.to_string(),
        )?;
        write_kv(
            &mut w,
            "power_save_observed",
            if client.network_intel.power_save_observed {
                "true".to_string()
            } else {
                "false".to_string()
            },
        )?;
        write_kv(
            &mut w,
            "qos_priorities",
            client
                .network_intel
                .qos_priorities
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(";"),
        )?;
        write_kv(
            &mut w,
            "eapol_frame_count",
            client.network_intel.eapol_frame_count.to_string(),
        )?;
        write_kv(
            &mut w,
            "pmkid_count",
            client.network_intel.pmkid_count.to_string(),
        )?;
        write_kv(
            &mut w,
            "first_location",
            location_coords_string(locations.first.as_ref()),
        )?;
        write_kv(
            &mut w,
            "first_location_timestamp",
            location_timestamp_string(locations.first.as_ref()),
        )?;
        write_kv(
            &mut w,
            "last_location",
            location_coords_string(locations.last.as_ref()),
        )?;
        write_kv(
            &mut w,
            "last_location_timestamp",
            location_timestamp_string(locations.last.as_ref()),
        )?;
        write_kv(
            &mut w,
            "strongest_location",
            location_coords_string(locations.strongest.as_ref()),
        )?;
        write_kv(
            &mut w,
            "strongest_location_timestamp",
            location_timestamp_string(locations.strongest.as_ref()),
        )?;
        write_kv(
            &mut w,
            "strongest_location_rssi_dbm",
            locations
                .strongest
                .as_ref()
                .and_then(|obs| obs.rssi_dbm)
                .map(|v| v.to_string())
                .unwrap_or_default(),
        )?;

        w.flush()?;
        Ok(path)
    }

    pub fn export_location_logs_csv(
        &self,
        aps: &[AccessPointRecord],
        clients: &[ClientRecord],
        bluetooth: &[BluetoothDeviceRecord],
    ) -> Result<Vec<PathBuf>> {
        let ap_path = self.paths.csv_dir.join("access_point_locations.csv");
        let mut ap_w = csv::Writer::from_path(&ap_path)?;
        ap_w.write_record([
            "Date",
            "BSSID",
            "OUI",
            "SSID",
            "Channel",
            "Encryption",
            "RSSI",
            "Associated Clients",
            "Latitude",
            "Longitude",
            "Altitude M",
        ])?;
        for ap in aps {
            for obs in &ap.observations {
                ap_w.write_record(&[
                    obs.timestamp.to_rfc3339(),
                    ap.bssid.clone(),
                    ap.oui_manufacturer.clone().unwrap_or_default(),
                    ap.ssid.clone().unwrap_or_else(|| "<hidden>".to_string()),
                    ap.channel.map(|v| v.to_string()).unwrap_or_default(),
                    ap.encryption_full.clone(),
                    obs.rssi_dbm
                        .or(ap.rssi_dbm)
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    ap.number_of_clients.to_string(),
                    format!("{:.8}", obs.latitude),
                    format!("{:.8}", obs.longitude),
                    obs.altitude_m
                        .map(|v| format!("{:.2}", v))
                        .unwrap_or_default(),
                ])?;
            }
        }
        ap_w.flush()?;

        let client_path = self.paths.csv_dir.join("client_locations.csv");
        let mut client_w = csv::Writer::from_path(&client_path)?;
        client_w.write_record([
            "Date",
            "MAC",
            "OUI",
            "Associated BSSID",
            "RSSI",
            "Latitude",
            "Longitude",
            "Altitude M",
        ])?;
        for client in clients {
            for obs in &client.observations {
                client_w.write_record(&[
                    obs.timestamp.to_rfc3339(),
                    client.mac.clone(),
                    client.oui_manufacturer.clone().unwrap_or_default(),
                    client.associated_ap.clone().unwrap_or_default(),
                    obs.rssi_dbm
                        .or(client.rssi_dbm)
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    format!("{:.8}", obs.latitude),
                    format!("{:.8}", obs.longitude),
                    obs.altitude_m
                        .map(|v| format!("{:.2}", v))
                        .unwrap_or_default(),
                ])?;
            }
        }
        client_w.flush()?;

        let bt_path = self.paths.csv_dir.join("bluetooth_locations.csv");
        let mut bt_w = csv::Writer::from_path(&bt_path)?;
        bt_w.write_record([
            "Date",
            "MAC",
            "OUI",
            "BT/BLE",
            "Device Type",
            "RSSI",
            "Latitude",
            "Longitude",
            "Altitude M",
        ])?;
        for dev in bluetooth {
            for obs in &dev.observations {
                bt_w.write_record(&[
                    obs.timestamp.to_rfc3339(),
                    dev.mac.clone(),
                    dev.oui_manufacturer.clone().unwrap_or_default(),
                    dev.transport.clone(),
                    dev.device_type.clone().unwrap_or_default(),
                    obs.rssi_dbm
                        .or(dev.rssi_dbm)
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    format!("{:.8}", obs.latitude),
                    format!("{:.8}", obs.longitude),
                    obs.altitude_m
                        .map(|v| format!("{:.2}", v))
                        .unwrap_or_default(),
                ])?;
            }
        }
        bt_w.flush()?;

        Ok(vec![ap_path, client_path, bt_path])
    }

    pub fn export_location_logs_kml(
        &self,
        aps: &[AccessPointRecord],
        clients: &[ClientRecord],
        bluetooth: &[BluetoothDeviceRecord],
    ) -> Result<Vec<PathBuf>> {
        let ap_dir = self.paths.kml_dir.join("access_points");
        let client_dir = self.paths.kml_dir.join("clients");
        let bt_dir = self.paths.kml_dir.join("bluetooth");
        fs::create_dir_all(&ap_dir)?;
        fs::create_dir_all(&client_dir)?;
        fs::create_dir_all(&bt_dir)?;

        let ap_path = ap_dir.join("observations.kml");
        let client_path = client_dir.join("observations.kml");
        let bt_path = bt_dir.join("observations.kml");

        let ap_ssid_by_bssid = access_point_ssid_map(aps);
        let ap_marks = build_ap_kml_placemarks(aps);
        let client_marks = build_client_kml_placemarks(clients, &ap_ssid_by_bssid);
        let bt_marks = build_bluetooth_kml_placemarks(bluetooth);

        fs::write(&ap_path, kml_document(&ap_marks))?;
        fs::write(&client_path, kml_document(&client_marks))?;
        fs::write(&bt_path, kml_document(&bt_marks))?;

        Ok(vec![ap_path, client_path, bt_path])
    }

    pub fn export_location_logs_kmz(
        &self,
        aps: &[AccessPointRecord],
        clients: &[ClientRecord],
        bluetooth: &[BluetoothDeviceRecord],
    ) -> Result<PathBuf> {
        let ap_ssid_by_bssid = access_point_ssid_map(aps);
        let ap_marks = build_ap_kml_placemarks(aps);
        let client_marks = build_client_kml_placemarks(clients, &ap_ssid_by_bssid);
        let bt_marks = build_bluetooth_kml_placemarks(bluetooth);

        let doc_kml = kml_document_with_folders(&[
            ("Access Points".to_string(), ap_marks),
            ("Clients".to_string(), client_marks),
            ("Bluetooth".to_string(), bt_marks),
        ]);

        let kmz_path = self.paths.kml_dir.join("observations.kmz");
        let kmz_file = File::create(&kmz_path)
            .with_context(|| format!("failed to create {}", kmz_path.display()))?;
        let mut zip = zip::ZipWriter::new(BufWriter::new(kmz_file));
        let options = FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        zip.start_file("doc.kml", options)?;
        zip.write_all(doc_kml.as_bytes())?;
        zip.finish()?;
        Ok(kmz_path)
    }

    pub fn export_summary_json(
        &self,
        aps: &[AccessPointRecord],
        clients: &[ClientRecord],
        bluetooth: &[BluetoothDeviceRecord],
    ) -> Result<PathBuf> {
        let path = self.paths.json_dir.join("summary.json");
        let ap_ssid_by_bssid = access_point_ssid_map(aps);

        let aps_json = aps
            .iter()
            .map(|ap| {
                let locations = summarize_locations(&ap.observations);
                let class = classify_ap_encryption(&ap.encryption_short, &ap.encryption_full);
                json!({
                    "ssid": ap.ssid.clone().unwrap_or_else(|| "<hidden>".to_string()),
                    "bssid": ap.bssid.clone(),
                    "oui_manufacturer": ap.oui_manufacturer.clone().unwrap_or_default(),
                    "source_adapters": ap.source_adapters.clone(),
                    "country_code_80211d": ap.country_code_80211d.clone().unwrap_or_default(),
                    "channel": ap.channel,
                    "frequency_mhz": ap.frequency_mhz,
                    "band": ap.band.label(),
                    "encryption_short": ap.encryption_short.clone(),
                    "encryption_full": ap.encryption_full.clone(),
                    "encryption_class": class.label(),
                    "rssi_dbm": ap.rssi_dbm,
                    "number_of_clients": ap.number_of_clients,
                    "first_seen": ap.first_seen.to_rfc3339(),
                    "last_seen": ap.last_seen.to_rfc3339(),
                    "handshake_count": ap.handshake_count,
                    "uptime_beacons": ap.uptime_beacons,
                    "wps": ap.wps.clone(),
                    "packet_mix": ap.packet_mix.clone(),
                    "observation_count": ap.observations.len(),
                    "first_location": location_summary_json(locations.first.as_ref()),
                    "last_location": location_summary_json(locations.last.as_ref()),
                    "strongest_location": location_summary_json(locations.strongest.as_ref()),
                })
            })
            .collect::<Vec<_>>();

        let clients_json = clients
            .iter()
            .map(|client| {
                let locations = summarize_locations(&client.observations);
                let associated_bssid = client.associated_ap.clone().unwrap_or_default();
                let associated_ssid = ap_ssid_by_bssid
                    .get(&associated_bssid)
                    .cloned()
                    .unwrap_or_default();
                let associated_state = if associated_bssid.trim().is_empty() {
                    "unassociated"
                } else {
                    "associated"
                };
                json!({
                    "mac": client.mac.clone(),
                    "oui_manufacturer": client.oui_manufacturer.clone().unwrap_or_default(),
                    "source_adapters": client.source_adapters.clone(),
                    "associated_state": associated_state,
                    "associated_bssid": associated_bssid,
                    "associated_ssid": associated_ssid,
                    "wps": client.wps.clone(),
                    "rssi_dbm": client.rssi_dbm,
                    "data_transferred_bytes": client.data_transferred_bytes,
                    "probes": client.probes.clone(),
                    "seen_access_points": client.seen_access_points.clone(),
                    "handshake_networks": client.handshake_networks.clone(),
                    "first_seen": client.first_seen.to_rfc3339(),
                    "last_seen": client.last_seen.to_rfc3339(),
                    "network_intel": {
                        "packet_mix": client.network_intel.packet_mix.clone(),
                        "uplink_bytes": client.network_intel.uplink_bytes,
                        "downlink_bytes": client.network_intel.downlink_bytes,
                        "retry_frame_count": client.network_intel.retry_frame_count,
                        "power_save_observed": client.network_intel.power_save_observed,
                        "qos_priorities": client.network_intel.qos_priorities.clone(),
                        "eapol_frame_count": client.network_intel.eapol_frame_count,
                        "pmkid_count": client.network_intel.pmkid_count,
                        "last_frame_type": client.network_intel.last_frame_type,
                        "last_frame_subtype": client.network_intel.last_frame_subtype,
                        "last_channel": client.network_intel.last_channel,
                        "last_frequency_mhz": client.network_intel.last_frequency_mhz,
                        "band": client.network_intel.band.label(),
                        "last_reason_code": client.network_intel.last_reason_code,
                        "last_status_code": client.network_intel.last_status_code,
                        "listen_interval": client.network_intel.listen_interval,
                    },
                    "observation_count": client.observations.len(),
                    "first_location": location_summary_json(locations.first.as_ref()),
                    "last_location": location_summary_json(locations.last.as_ref()),
                    "strongest_location": location_summary_json(locations.strongest.as_ref()),
                })
            })
            .collect::<Vec<_>>();

        let bluetooth_json = bluetooth
            .iter()
            .map(|dev| {
                let locations = summarize_locations(&dev.observations);
                json!({
                    "mac": dev.mac.clone(),
                    "transport": dev.transport.clone(),
                    "transport_class": bluetooth_transport_class(&dev.transport),
                    "source_adapters": dev.source_adapters.clone(),
                    "address_type": dev.address_type.clone(),
                    "name": dev.advertised_name.clone().unwrap_or_default(),
                    "alias": dev.alias.clone().unwrap_or_default(),
                    "oui_manufacturer": dev.oui_manufacturer.clone().unwrap_or_default(),
                    "device_type": dev.device_type.clone().unwrap_or_default(),
                    "class_of_device": dev.class_of_device.clone().unwrap_or_default(),
                    "rssi_dbm": dev.rssi_dbm,
                    "mfgr_ids": dev.mfgr_ids.clone(),
                    "mfgr_names": dev.mfgr_names.clone(),
                    "uuids": dev.uuids.clone(),
                    "uuid_names": dev.uuid_names.clone(),
                    "active_enumeration": dev.active_enumeration.clone(),
                    "first_seen": dev.first_seen.to_rfc3339(),
                    "last_seen": dev.last_seen.to_rfc3339(),
                    "observation_count": dev.observations.len(),
                    "first_location": location_summary_json(locations.first.as_ref()),
                    "last_location": location_summary_json(locations.last.as_ref()),
                    "strongest_location": location_summary_json(locations.strongest.as_ref()),
                })
            })
            .collect::<Vec<_>>();

        let payload = json!({
            "generated_at": Utc::now().to_rfc3339(),
            "session_dir": self.paths.session_dir.display().to_string(),
            "counts": {
                "access_points": aps.len(),
                "clients": clients.len(),
                "bluetooth": bluetooth.len(),
            },
            "access_points": aps_json,
            "clients": clients_json,
            "bluetooth": bluetooth_json,
        });

        fs::write(&path, serde_json::to_string_pretty(&payload)?)?;
        Ok(path)
    }

    pub fn export_filtered_pcap(
        &self,
        source_pcap: &Path,
        output_name: &str,
        display_filter: &str,
        gps_track: &[GeoObservation],
    ) -> Result<PathBuf> {
        let output_path = self.paths.pcap_dir.join(output_name);
        self.export_filtered_pcap_to_path(source_pcap, &output_path, display_filter, gps_track)?;
        Ok(output_path)
    }

    pub fn export_handshake_pcap(
        &self,
        source_pcap: &Path,
        bssid: &str,
        gps_track: &[GeoObservation],
    ) -> Result<PathBuf> {
        // On-demand AP handshake export: all EAPOL for this BSSID plus one beacon frame.
        let output_name = format!("{}.pcapng", sanitize_name(bssid));
        let output_path = self.paths.handshakes_dir.join(output_name);
        let eapol_filter = format!("wlan.bssid == {} && eapol", bssid);
        let filter = match self.first_beacon_frame_number(source_pcap, bssid) {
            Some(frame_no) => format!("({}) || frame.number == {}", eapol_filter, frame_no),
            None => eapol_filter,
        };
        self.export_filtered_pcap_to_path(source_pcap, &output_path, &filter, gps_track)?;
        Ok(output_path)
    }

    pub fn export_handshake_capture(
        &self,
        source_pcap: &Path,
        ap_ssid: Option<&str>,
        bssid: &str,
        client_mac: &str,
        timestamp: DateTime<Utc>,
        gps_track: &[GeoObservation],
    ) -> Result<PathBuf> {
        // Timestamp encoded in UTC Zulu time (military zone code Z).
        let timestamp_utc = timestamp.with_timezone(&Utc);
        let ts_part = timestamp_utc.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let ssid_part = sanitize_filename_component(ap_ssid.unwrap_or("hidden"), false);
        let bssid_part = sanitize_filename_component(bssid, true);
        let client_part = sanitize_filename_component(client_mac, true);
        let file_name = format!(
            "{}_{}_{}_{}.pcapng",
            ssid_part, bssid_part, client_part, ts_part
        );
        let output_path = self.paths.handshakes_dir.join(file_name);

        let eapol_filter = format!(
            "wlan.bssid == {} && eapol && (wlan.sa == {} || wlan.da == {} || wlan.addr == {})",
            bssid, client_mac, client_mac, client_mac
        );

        let filter = match self.first_beacon_frame_number(source_pcap, bssid) {
            Some(frame_no) => format!("({}) || frame.number == {}", eapol_filter, frame_no),
            None => eapol_filter,
        };

        self.export_filtered_pcap_to_path(source_pcap, &output_path, &filter, gps_track)?;
        Ok(output_path)
    }

    pub fn export_session_pcap_with_gps(
        &self,
        source_pcap: &Path,
        gps_track: &[GeoObservation],
    ) -> Result<PathBuf> {
        let output_path = self
            .paths
            .pcap_dir
            .join("consolidated_capture_with_gps.pcapng");
        fs::copy(source_pcap, &output_path).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source_pcap.display(),
                output_path.display()
            )
        })?;
        self.annotate_pcapng_with_gps(&output_path, gps_track)?;
        Ok(output_path)
    }

    fn annotate_pcapng_with_gps(
        &self,
        pcapng_path: &Path,
        gps_track: &[GeoObservation],
    ) -> Result<()> {
        if gps_track.is_empty() {
            return Ok(());
        }

        let tmp_path = pcapng_path.with_extension("pcapng.gps_tmp");
        let input = File::open(pcapng_path)
            .with_context(|| format!("failed to open {}", pcapng_path.display()))?;
        let mut reader = PcapNgReader::new(BufReader::new(input))
            .with_context(|| format!("failed to read pcapng {}", pcapng_path.display()))?;

        let output = File::create(&tmp_path)
            .with_context(|| format!("failed to create {}", tmp_path.display()))?;
        let section = reader.section().clone();
        let mut writer = PcapNgWriter::with_section_header(BufWriter::new(output), section)
            .context("failed to initialize pcapng writer")?;

        let mut sorted_track = gps_track.to_vec();
        sorted_track.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        while let Some(block) = reader.next_block() {
            let block = block.context("failed while reading pcapng block")?;
            let mut owned = block.into_owned();

            if let PcapNgBlock::EnhancedPacket(ref mut epb) = owned {
                let pkt_ts = Utc
                    .timestamp_opt(epb.timestamp.as_secs() as i64, epb.timestamp.subsec_nanos())
                    .single();

                if let Some(pkt_ts) = pkt_ts {
                    if let Some(fix) = nearest_fix(&sorted_track, pkt_ts) {
                        let comment = format_gps_comment(fix);
                        epb.options
                            .push(EnhancedPacketOption::Comment(Cow::Owned(comment)));
                    }
                }
            }

            writer.write_block(&owned)?;
        }

        drop(writer);
        fs::rename(&tmp_path, pcapng_path).with_context(|| {
            format!(
                "failed replacing {} with GPS-annotated version",
                pcapng_path.display()
            )
        })?;
        Ok(())
    }

    fn export_filtered_pcap_to_path(
        &self,
        source_pcap: &Path,
        output_path: &Path,
        display_filter: &str,
        gps_track: &[GeoObservation],
    ) -> Result<()> {
        let filter = display_filter.trim();
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let export_result = if filter.is_empty() {
            fs::copy(source_pcap, output_path)
                .with_context(|| {
                    format!(
                        "failed to copy {} to {}",
                        source_pcap.display(),
                        output_path.display()
                    )
                })
                .map(|_| ())
        } else {
            let status = std::process::Command::new("tshark")
                .arg("-r")
                .arg(source_pcap)
                .arg("-Y")
                .arg(filter)
                .arg("-w")
                .arg(output_path)
                .status();

            match status {
                Ok(s) if s.success() => Ok(()),
                Ok(_) | Err(_) => {
                    fs::write(
                        output_path,
                        format!(
                            "Could not run tshark filter. source={} filter={}\n",
                            source_pcap.display(),
                            filter
                        ),
                    )?;
                    Ok(())
                }
            }
        };

        export_result?;
        if !gps_track.is_empty() {
            self.annotate_pcapng_with_gps(output_path, gps_track)?;
        }
        Ok(())
    }

    fn first_beacon_frame_number(&self, source_pcap: &Path, bssid: &str) -> Option<u64> {
        let filter = format!("wlan.bssid == {} && wlan.fc.type_subtype == 8", bssid);
        let output = std::process::Command::new("tshark")
            .arg("-r")
            .arg(source_pcap)
            .arg("-Y")
            .arg(filter)
            .arg("-c")
            .arg("1")
            .arg("-T")
            .arg("fields")
            .arg("-e")
            .arg("frame.number")
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let text = String::from_utf8_lossy(&output.stdout);
        text.lines().next()?.trim().parse::<u64>().ok()
    }
}

fn write_kv(writer: &mut csv::Writer<File>, field: &str, value: String) -> Result<()> {
    writer.write_record([field, &value])?;
    Ok(())
}

#[derive(Debug, Clone)]
struct LocationObservationSummary {
    latitude: f64,
    longitude: f64,
    timestamp: String,
    rssi_dbm: Option<i32>,
}

#[derive(Debug, Clone, Default)]
struct LocationSummary {
    first: Option<LocationObservationSummary>,
    last: Option<LocationObservationSummary>,
    strongest: Option<LocationObservationSummary>,
}

fn summarize_locations(observations: &[GeoObservation]) -> LocationSummary {
    let highlights = observation_highlights(observations);
    LocationSummary {
        first: highlights.first.as_ref().map(location_summary_point),
        last: highlights.last.as_ref().map(location_summary_point),
        strongest: highlights.strongest.as_ref().map(location_summary_point),
    }
}

fn location_summary_point(obs: &GeoObservation) -> LocationObservationSummary {
    LocationObservationSummary {
        latitude: obs.latitude,
        longitude: obs.longitude,
        timestamp: obs.timestamp.to_rfc3339(),
        rssi_dbm: obs.rssi_dbm,
    }
}

fn location_coords_string(obs: Option<&LocationObservationSummary>) -> String {
    obs.map(|point| format!("{:.6},{:.6}", point.latitude, point.longitude))
        .unwrap_or_default()
}

fn location_timestamp_string(obs: Option<&LocationObservationSummary>) -> String {
    obs.map(|point| point.timestamp.clone()).unwrap_or_default()
}

fn location_summary_json(obs: Option<&LocationObservationSummary>) -> serde_json::Value {
    match obs {
        Some(point) => json!({
            "latitude": point.latitude,
            "longitude": point.longitude,
            "timestamp": point.timestamp,
            "rssi_dbm": point.rssi_dbm,
        }),
        None => serde_json::Value::Null,
    }
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn sanitize_filename_component(value: &str, keep_colon: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        let keep = ch.is_ascii_alphanumeric()
            || ch == '_'
            || ch == '-'
            || ch == '.'
            || (keep_colon && ch == ':');
        if keep {
            out.push(ch);
        } else {
            out.push('_');
        }
    }

    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

fn format_gps_comment(fix: &GeoObservation) -> String {
    format!(
        "gps.lat={:.8};gps.lon={:.8};gps.alt_m={};gps.ts={}",
        fix.latitude,
        fix.longitude,
        fix.altitude_m
            .map(|v| format!("{:.2}", v))
            .unwrap_or_else(|| "NA".to_string()),
        fix.timestamp.to_rfc3339()
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApEncryptionClass {
    Open,
    Legacy,
    Wpa2,
    Wpa3,
}

impl ApEncryptionClass {
    fn style_id(self) -> &'static str {
        match self {
            Self::Open => "ap-open",
            Self::Legacy => "ap-legacy",
            Self::Wpa2 => "ap-wpa2",
            Self::Wpa3 => "ap-wpa3",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Legacy => "wep_or_wpa",
            Self::Wpa2 => "wpa2",
            Self::Wpa3 => "wpa3",
        }
    }
}

fn classify_ap_encryption(encryption_short: &str, encryption_full: &str) -> ApEncryptionClass {
    let normalized = format!(
        "{} {}",
        encryption_short.to_ascii_lowercase(),
        encryption_full.to_ascii_lowercase()
    );
    if normalized.contains("open")
        || normalized.contains("unencrypted")
        || normalized.contains("none")
    {
        return ApEncryptionClass::Open;
    }
    if normalized.contains("wpa3") || normalized.contains("sae") {
        return ApEncryptionClass::Wpa3;
    }
    if normalized.contains("wpa2") || normalized.contains("rsn") {
        return ApEncryptionClass::Wpa2;
    }
    if normalized.contains("wep") || normalized.contains("wpa") || normalized.contains("protected")
    {
        return ApEncryptionClass::Legacy;
    }
    ApEncryptionClass::Legacy
}

fn access_point_ssid_map(aps: &[AccessPointRecord]) -> HashMap<String, String> {
    aps.iter()
        .map(|ap| {
            (
                ap.bssid.clone(),
                ap.ssid.clone().unwrap_or_else(|| "<hidden>".to_string()),
            )
        })
        .collect()
}

fn build_ap_kml_placemarks(aps: &[AccessPointRecord]) -> String {
    let mut marks = String::new();
    for ap in aps {
        let ssid = ap.ssid.clone().unwrap_or_else(|| "<hidden>".to_string());
        let class = classify_ap_encryption(&ap.encryption_short, &ap.encryption_full);
        let style_id = class.style_id();
        let label = format!("AP {} / {}", ssid, ap.bssid);
        for obs in &ap.observations {
            let rssi = obs.rssi_dbm.or(ap.rssi_dbm).unwrap_or(-99);
            let fields = vec![
                ("device_type".to_string(), "wifi_ap".to_string()),
                ("ssid".to_string(), ssid.clone()),
                ("bssid".to_string(), ap.bssid.clone()),
                (
                    "oui_manufacturer".to_string(),
                    ap.oui_manufacturer.clone().unwrap_or_default(),
                ),
                ("source_adapters".to_string(), ap.source_adapters.join(";")),
                (
                    "country_code_80211d".to_string(),
                    ap.country_code_80211d.clone().unwrap_or_default(),
                ),
                (
                    "channel".to_string(),
                    ap.channel.map(|v| v.to_string()).unwrap_or_default(),
                ),
                (
                    "frequency_mhz".to_string(),
                    ap.frequency_mhz.map(|v| v.to_string()).unwrap_or_default(),
                ),
                ("encryption_short".to_string(), ap.encryption_short.clone()),
                ("encryption_full".to_string(), ap.encryption_full.clone()),
                ("encryption_class".to_string(), class.label().to_string()),
                ("clients".to_string(), ap.number_of_clients.to_string()),
                (
                    "handshake_count".to_string(),
                    ap.handshake_count.to_string(),
                ),
                ("rssi_dbm".to_string(), rssi.to_string()),
                ("first_seen".to_string(), ap.first_seen.to_rfc3339()),
                ("last_seen".to_string(), ap.last_seen.to_rfc3339()),
                ("observation_time".to_string(), obs.timestamp.to_rfc3339()),
                ("latitude".to_string(), format!("{:.8}", obs.latitude)),
                ("longitude".to_string(), format!("{:.8}", obs.longitude)),
                (
                    "altitude_m".to_string(),
                    obs.altitude_m
                        .map(|v| format!("{:.2}", v))
                        .unwrap_or_default(),
                ),
            ];
            let description = format!(
                "BSSID={} | SSID={} | Encryption={} | Channel={} | Clients={} | RSSI={} dBm",
                ap.bssid,
                ssid,
                ap.encryption_full,
                ap.channel.map(|v| v.to_string()).unwrap_or_default(),
                ap.number_of_clients,
                rssi,
            );
            marks.push_str(&kml_placemark(&label, obs, style_id, &description, &fields));
        }
    }
    marks
}

fn build_client_kml_placemarks(
    clients: &[ClientRecord],
    ap_ssid_by_bssid: &HashMap<String, String>,
) -> String {
    let mut marks = String::new();
    for client in clients {
        let associated_bssid = client.associated_ap.clone().unwrap_or_default();
        let associated_ssid = ap_ssid_by_bssid
            .get(&associated_bssid)
            .cloned()
            .unwrap_or_default();
        let style_id = client_style_id(&associated_bssid);
        let associated_state = if associated_bssid.trim().is_empty() {
            "unassociated"
        } else {
            "associated"
        };
        let label = format!("Client {}", client.mac);
        for obs in &client.observations {
            let rssi = obs.rssi_dbm.or(client.rssi_dbm).unwrap_or(-99);
            let fields = vec![
                ("device_type".to_string(), "wifi_client".to_string()),
                ("mac".to_string(), client.mac.clone()),
                (
                    "oui_manufacturer".to_string(),
                    client.oui_manufacturer.clone().unwrap_or_default(),
                ),
                (
                    "source_adapters".to_string(),
                    client.source_adapters.join(";"),
                ),
                ("associated_state".to_string(), associated_state.to_string()),
                ("associated_bssid".to_string(), associated_bssid.clone()),
                ("associated_ssid".to_string(), associated_ssid.clone()),
                ("rssi_dbm".to_string(), rssi.to_string()),
                (
                    "data_transferred_bytes".to_string(),
                    client.data_transferred_bytes.to_string(),
                ),
                ("probes".to_string(), client.probes.join(";")),
                ("first_seen".to_string(), client.first_seen.to_rfc3339()),
                ("last_seen".to_string(), client.last_seen.to_rfc3339()),
                ("observation_time".to_string(), obs.timestamp.to_rfc3339()),
                ("latitude".to_string(), format!("{:.8}", obs.latitude)),
                ("longitude".to_string(), format!("{:.8}", obs.longitude)),
                (
                    "altitude_m".to_string(),
                    obs.altitude_m
                        .map(|v| format!("{:.2}", v))
                        .unwrap_or_default(),
                ),
            ];
            let description = format!(
                "MAC={} | Associated BSSID={} | Associated SSID={} | RSSI={} dBm",
                client.mac, associated_bssid, associated_ssid, rssi
            );
            marks.push_str(&kml_placemark(&label, obs, style_id, &description, &fields));
        }
    }
    marks
}

fn client_style_id(associated_bssid: &str) -> &'static str {
    if associated_bssid.trim().is_empty() {
        "client-unassociated"
    } else {
        "client-associated"
    }
}

fn build_bluetooth_kml_placemarks(bluetooth: &[BluetoothDeviceRecord]) -> String {
    let mut marks = String::new();
    for dev in bluetooth {
        let device_name = dev
            .advertised_name
            .clone()
            .or_else(|| dev.alias.clone())
            .unwrap_or_else(|| dev.mac.clone());
        let label = format!("Bluetooth {}", device_name);
        for obs in &dev.observations {
            let rssi = obs.rssi_dbm.or(dev.rssi_dbm).unwrap_or(-99);
            let fields = vec![
                ("device_type".to_string(), "bluetooth".to_string()),
                ("name".to_string(), device_name.clone()),
                ("alias".to_string(), dev.alias.clone().unwrap_or_default()),
                ("mac".to_string(), dev.mac.clone()),
                (
                    "address_type".to_string(),
                    dev.address_type.clone().unwrap_or_default(),
                ),
                ("transport".to_string(), dev.transport.clone()),
                (
                    "transport_class".to_string(),
                    bluetooth_transport_class(&dev.transport).to_string(),
                ),
                (
                    "oui_manufacturer".to_string(),
                    dev.oui_manufacturer.clone().unwrap_or_default(),
                ),
                ("source_adapters".to_string(), dev.source_adapters.join(";")),
                (
                    "bluetooth_type".to_string(),
                    dev.device_type.clone().unwrap_or_default(),
                ),
                (
                    "class_of_device".to_string(),
                    dev.class_of_device.clone().unwrap_or_default(),
                ),
                ("rssi_dbm".to_string(), rssi.to_string()),
                ("mfgr_ids".to_string(), dev.mfgr_ids.join(";")),
                ("mfgr_names".to_string(), dev.mfgr_names.join(";")),
                ("uuids".to_string(), dev.uuids.join(";")),
                ("uuid_names".to_string(), dev.uuid_names.join(";")),
                ("first_seen".to_string(), dev.first_seen.to_rfc3339()),
                ("last_seen".to_string(), dev.last_seen.to_rfc3339()),
                ("observation_time".to_string(), obs.timestamp.to_rfc3339()),
                ("latitude".to_string(), format!("{:.8}", obs.latitude)),
                ("longitude".to_string(), format!("{:.8}", obs.longitude)),
                (
                    "altitude_m".to_string(),
                    obs.altitude_m
                        .map(|v| format!("{:.2}", v))
                        .unwrap_or_default(),
                ),
            ];
            let mut fields = fields;
            if let Some(active) = &dev.active_enumeration {
                fields.push(("active_connected".to_string(), active.connected.to_string()));
                fields.push(("active_paired".to_string(), active.paired.to_string()));
                fields.push(("active_trusted".to_string(), active.trusted.to_string()));
                fields.push(("active_blocked".to_string(), active.blocked.to_string()));
                fields.push((
                    "active_services_resolved".to_string(),
                    active.services_resolved.to_string(),
                ));
                fields.push((
                    "active_battery_percent".to_string(),
                    active
                        .battery_percent
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                ));
                fields.push((
                    "active_tx_power_dbm".to_string(),
                    active
                        .tx_power_dbm
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                ));
                fields.push((
                    "active_last_enumerated".to_string(),
                    active
                        .last_enumerated
                        .map(|v| v.to_rfc3339())
                        .unwrap_or_default(),
                ));
            }
            let description = format!(
                "MAC={} | Name={} | Transport={} | Type={} | RSSI={} dBm",
                dev.mac,
                device_name,
                dev.transport,
                dev.device_type.clone().unwrap_or_default(),
                rssi,
            );
            let style_id = bluetooth_style_id(&dev.transport);
            marks.push_str(&kml_placemark(&label, obs, style_id, &description, &fields));
        }
    }
    marks
}

fn bluetooth_style_id(transport: &str) -> &'static str {
    match bluetooth_transport_class(transport) {
        "ble" => "bluetooth-ble",
        "classic" => "bluetooth-classic",
        _ => "bluetooth",
    }
}

fn bluetooth_transport_class(transport: &str) -> &'static str {
    let normalized = transport.trim().to_ascii_lowercase();
    if normalized == "ble"
        || normalized == "le"
        || normalized == "btle"
        || normalized.contains("bluetooth le")
        || normalized.contains("low energy")
    {
        "ble"
    } else if normalized == "bt"
        || normalized == "classic"
        || normalized == "br/edr"
        || normalized == "br-edr"
        || normalized == "bredr"
        || normalized.contains("bluetooth classic")
    {
        "classic"
    } else {
        "unknown"
    }
}

fn kml_document(placemarks: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<kml xmlns=\"http://www.opengis.net/kml/2.2\">\n\
<Document>\n\
{}\
{}\n\
</Document>\n\
</kml>\n",
        kml_styles_block(),
        placemarks
    )
}

fn kml_document_with_folders(folders: &[(String, String)]) -> String {
    let mut folder_xml = String::new();
    for (name, placemarks) in folders {
        folder_xml.push_str(&format!(
            "<Folder><name>{}</name>{}</Folder>\n",
            xml_escape(name),
            placemarks
        ));
    }

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<kml xmlns=\"http://www.opengis.net/kml/2.2\">\n\
<Document>\n\
{}\
{}\n\
</Document>\n\
</kml>\n",
        kml_styles_block(),
        folder_xml
    )
}

fn kml_styles_block() -> &'static str {
    "<Style id=\"ap-open\"><IconStyle><color>ff0000ff</color><scale>1.1</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/triangle.png</href></Icon></IconStyle></Style>\n\
<Style id=\"ap-legacy\"><IconStyle><color>ff00ffff</color><scale>1.1</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/triangle.png</href></Icon></IconStyle></Style>\n\
<Style id=\"ap-wpa2\"><IconStyle><color>ff00ff00</color><scale>1.1</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/triangle.png</href></Icon></IconStyle></Style>\n\
<Style id=\"ap-wpa3\"><IconStyle><color>ffff0000</color><scale>1.1</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/triangle.png</href></Icon></IconStyle></Style>\n\
<Style id=\"ap\"><IconStyle><color>ff00ff00</color><scale>1.1</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/triangle.png</href></Icon></IconStyle></Style>\n\
<Style id=\"client\"><IconStyle><color>ffffffff</color><scale>1.1</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/placemark_circle.png</href></Icon></IconStyle></Style>\n\
<Style id=\"client-associated\"><IconStyle><color>ffffffff</color><scale>1.1</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/placemark_circle.png</href></Icon></IconStyle></Style>\n\
<Style id=\"client-unassociated\"><IconStyle><color>ffb4b4b4</color><scale>1.0</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/placemark_circle.png</href></Icon></IconStyle></Style>\n\
<Style id=\"bluetooth\"><IconStyle><color>ffff00ff</color><scale>1.0</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/shaded_dot.png</href></Icon></IconStyle></Style>\n\
<Style id=\"bluetooth-ble\"><IconStyle><color>ffff00ff</color><scale>1.0</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/shaded_dot.png</href></Icon></IconStyle></Style>\n\
<Style id=\"bluetooth-classic\"><IconStyle><color>ffffff00</color><scale>1.0</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/shaded_dot.png</href></Icon></IconStyle></Style>\n"
}

fn kml_placemark(
    name: &str,
    obs: &GeoObservation,
    style: &str,
    description: &str,
    properties: &[(String, String)],
) -> String {
    let extended_data = kml_extended_data(properties);
    format!(
        "<Placemark><name>{}</name><description>{}</description><styleUrl>#{}</styleUrl>{}<Point><coordinates>{},{},{}</coordinates></Point></Placemark>\n",
        xml_escape(name),
        xml_escape(description),
        style,
        extended_data,
        obs.longitude,
        obs.latitude,
        obs.altitude_m.unwrap_or(0.0),
    )
}

fn kml_extended_data(properties: &[(String, String)]) -> String {
    if properties.is_empty() {
        return String::new();
    }

    let mut out = String::from("<ExtendedData>");
    for (name, value) in properties {
        out.push_str(&format!(
            "<Data name=\"{}\"><value>{}</value></Data>",
            xml_escape(name),
            xml_escape(value)
        ));
    }
    out.push_str("</ExtendedData>");
    out
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn nearest_fix(track: &[GeoObservation], packet_ts: DateTime<Utc>) -> Option<&GeoObservation> {
    if track.is_empty() {
        return None;
    }

    let idx = track.partition_point(|fix| fix.timestamp <= packet_ts);
    let left = idx.checked_sub(1).and_then(|i| track.get(i));
    let right = track.get(idx);

    let best = match (left, right) {
        (Some(l), Some(r)) => {
            let dl = (packet_ts - l.timestamp).num_milliseconds().abs();
            let dr = (r.timestamp - packet_ts).num_milliseconds().abs();
            if dl <= dr {
                l
            } else {
                r
            }
        }
        (Some(l), None) => l,
        (None, Some(r)) => r,
        (None, None) => return None,
    };

    let delta_ms = (packet_ts - best.timestamp).num_milliseconds().abs();
    if delta_ms > 20_000 {
        return None;
    }

    if best.latitude.abs() > 90.0 || best.longitude.abs() > 180.0 {
        return None;
    }

    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SpectrumBand;
    use std::fs;
    use std::io::Read;

    fn sample_observation(now: DateTime<Utc>) -> GeoObservation {
        GeoObservation {
            timestamp: now,
            latitude: 37.0,
            longitude: -122.0,
            altitude_m: Some(10.0),
            rssi_dbm: Some(-55),
        }
    }

    #[test]
    fn classify_ap_encryption_maps_expected_classes() {
        assert_eq!(
            classify_ap_encryption("Open", "Open"),
            ApEncryptionClass::Open
        );
        assert_eq!(
            classify_ap_encryption("WEP", "WEP"),
            ApEncryptionClass::Legacy
        );
        assert_eq!(
            classify_ap_encryption("WPA2", "WPA2-PSK"),
            ApEncryptionClass::Wpa2
        );
        assert_eq!(
            classify_ap_encryption("WPA3", "SAE"),
            ApEncryptionClass::Wpa3
        );
    }

    #[test]
    fn ap_kml_placemarks_use_encryption_style_ids() {
        let now = Utc::now();
        let mut open_ap = AccessPointRecord::new("11:22:33:44:55:66", now);
        open_ap.ssid = Some("OpenNet".to_string());
        open_ap.encryption_short = "Open".to_string();
        open_ap.encryption_full = "Open".to_string();
        open_ap.source_adapters = vec!["wlan0mon".to_string()];
        open_ap.observations.push(sample_observation(now));

        let mut wpa3_ap = AccessPointRecord::new("AA:BB:CC:DD:EE:FF", now);
        wpa3_ap.ssid = Some("SecureNet".to_string());
        wpa3_ap.encryption_short = "WPA3".to_string();
        wpa3_ap.encryption_full = "SAE".to_string();
        wpa3_ap.source_adapters = vec!["wlan1mon".to_string()];
        wpa3_ap.observations.push(sample_observation(now));

        let xml = build_ap_kml_placemarks(&[open_ap, wpa3_ap]);
        assert!(xml.contains("<styleUrl>#ap-open</styleUrl>"));
        assert!(xml.contains("<styleUrl>#ap-wpa3</styleUrl>"));
        assert!(xml.contains("<Data name=\"source_adapters\"><value>wlan0mon</value></Data>"));
        assert!(xml.contains("<Data name=\"source_adapters\"><value>wlan1mon</value></Data>"));
    }

    #[test]
    fn kml_styles_block_contains_required_style_ids() {
        let styles = kml_styles_block();
        for required in [
            "id=\"ap-open\"",
            "id=\"ap-legacy\"",
            "id=\"ap-wpa2\"",
            "id=\"ap-wpa3\"",
            "id=\"client\"",
            "id=\"client-associated\"",
            "id=\"client-unassociated\"",
            "id=\"bluetooth\"",
            "id=\"bluetooth-ble\"",
            "id=\"bluetooth-classic\"",
        ] {
            assert!(styles.contains(required));
        }
    }

    #[test]
    fn xml_escape_encodes_special_characters() {
        let input = "A&B <C> \"D\" 'E'";
        let escaped = xml_escape(input);
        assert_eq!(escaped, "A&amp;B &lt;C&gt; &quot;D&quot; &apos;E&apos;");
    }

    fn test_export_manager() -> ExportManager {
        let root = std::env::temp_dir().join(format!(
            "wirelessexplorer-export-tests-{}",
            uuid::Uuid::new_v4()
        ));
        let manager = ExportManager::new(&root, "unit-test").expect("create export manager");
        manager
    }

    #[test]
    fn export_summary_json_contains_expected_counts_and_links() {
        let now = Utc::now();
        let manager = test_export_manager();

        let mut ap = AccessPointRecord::new("11:22:33:44:55:66", now);
        ap.ssid = Some("SecureNet".to_string());
        ap.encryption_short = "WPA3".to_string();
        ap.encryption_full = "SAE".to_string();
        ap.band = SpectrumBand::Ghz5;
        ap.source_adapters = vec!["wlan0mon".to_string()];
        ap.wps = Some(crate::model::WpsInfo {
            version: Some("2.0".to_string()),
            ..crate::model::WpsInfo::default()
        });
        ap.observations.push(sample_observation(now));

        let mut client = ClientRecord::new("AA:BB:CC:DD:EE:FF", now);
        client.associated_ap = Some(ap.bssid.clone());
        client.source_adapters = vec!["wlan0mon".to_string()];
        client.wps = Some(crate::model::WpsInfo {
            state: Some("configured".to_string()),
            ..crate::model::WpsInfo::default()
        });
        client.observations.push(sample_observation(now));

        let mut bluetooth = BluetoothDeviceRecord::new("22:33:44:55:66:77", now);
        bluetooth.transport = "BLE".to_string();
        bluetooth.source_adapters = vec!["bluez:hci0".to_string()];
        bluetooth.active_enumeration = Some(crate::model::BluetoothActiveEnumeration {
            connected: true,
            battery_percent: Some(84),
            ..crate::model::BluetoothActiveEnumeration::default()
        });
        bluetooth.observations.push(sample_observation(now));

        let summary_path = manager
            .export_summary_json(&[ap.clone()], &[client.clone()], &[bluetooth.clone()])
            .expect("export summary json");
        let raw = fs::read_to_string(&summary_path).expect("read summary json");
        let payload: serde_json::Value =
            serde_json::from_str(&raw).expect("parse summary json payload");

        assert_eq!(payload["counts"]["access_points"], 1);
        assert_eq!(payload["counts"]["clients"], 1);
        assert_eq!(payload["counts"]["bluetooth"], 1);
        assert_eq!(payload["access_points"][0]["encryption_class"], "wpa3");
        assert_eq!(
            payload["access_points"][0]["source_adapters"][0],
            "wlan0mon"
        );
        assert_eq!(payload["access_points"][0]["band"], "5 GHz");
        assert_eq!(payload["access_points"][0]["wps"]["version"], "2.0");
        assert_eq!(payload["clients"][0]["associated_state"], "associated");
        assert_eq!(payload["clients"][0]["associated_bssid"], ap.bssid);
        assert_eq!(payload["clients"][0]["associated_ssid"], "SecureNet");
        assert_eq!(payload["clients"][0]["source_adapters"][0], "wlan0mon");
        assert_eq!(payload["clients"][0]["wps"]["state"], "configured");
        assert_eq!(payload["bluetooth"][0]["transport"], "BLE");
        assert_eq!(payload["bluetooth"][0]["transport_class"], "ble");
        assert_eq!(payload["bluetooth"][0]["source_adapters"][0], "bluez:hci0");
        assert_eq!(
            payload["bluetooth"][0]["active_enumeration"]["connected"],
            true
        );
        assert_eq!(
            payload["bluetooth"][0]["active_enumeration"]["battery_percent"],
            84
        );
    }

    #[test]
    fn export_filtered_pcap_to_path_copies_source_when_filter_is_empty() {
        let manager = test_export_manager();
        let source = manager.paths.pcap_dir.join("source.pcapng");
        let output = manager.paths.pcap_dir.join("copied.pcapng");
        fs::write(&source, b"pcapng-test-payload").expect("write source test pcap");

        manager
            .export_filtered_pcap_to_path(&source, &output, "   ", &[])
            .expect("export filtered pcap copy");

        let source_bytes = fs::read(&source).expect("read source bytes");
        let output_bytes = fs::read(&output).expect("read output bytes");
        assert_eq!(source_bytes, output_bytes);
    }

    #[test]
    fn export_kmz_contains_expected_folders_and_extended_data_fields() {
        let now = Utc::now();
        let manager = test_export_manager();

        let mut ap = AccessPointRecord::new("11:22:33:44:55:66", now);
        ap.ssid = Some("SecureNet".to_string());
        ap.encryption_short = "WPA3".to_string();
        ap.encryption_full = "SAE".to_string();
        ap.oui_manufacturer = Some("ExampleVendor".to_string());
        ap.source_adapters = vec!["wlan0mon".to_string(), "wlan1mon".to_string()];
        ap.channel = Some(11);
        ap.frequency_mhz = Some(2462);
        ap.observations.push(sample_observation(now));

        let mut client = ClientRecord::new("AA:BB:CC:DD:EE:FF", now);
        client.associated_ap = Some(ap.bssid.clone());
        client.oui_manufacturer = Some("ClientVendor".to_string());
        client.source_adapters = vec!["wlan0mon".to_string()];
        client.observations.push(sample_observation(now));
        let mut client_unassociated = ClientRecord::new("AA:BB:CC:DD:EE:00", now);
        client_unassociated.associated_ap = None;
        client_unassociated.oui_manufacturer = Some("ClientVendor".to_string());
        client_unassociated.source_adapters = vec!["wlan1mon".to_string()];
        client_unassociated
            .observations
            .push(sample_observation(now));

        let mut bluetooth_ble = BluetoothDeviceRecord::new("22:33:44:55:66:77", now);
        bluetooth_ble.transport = "BLE".to_string();
        bluetooth_ble.device_type = Some("tag".to_string());
        bluetooth_ble.advertised_name = Some("BeaconTag".to_string());
        bluetooth_ble.alias = Some("TagAlias".to_string());
        bluetooth_ble.address_type = Some("random".to_string());
        bluetooth_ble.class_of_device = Some("0x000000".to_string());
        bluetooth_ble.mfgr_names = vec!["Acme".to_string()];
        bluetooth_ble.uuid_names = vec!["Battery Service".to_string()];
        bluetooth_ble.active_enumeration = Some(crate::model::BluetoothActiveEnumeration {
            connected: true,
            paired: true,
            trusted: true,
            blocked: false,
            services_resolved: true,
            battery_percent: Some(82),
            tx_power_dbm: Some(-4),
            last_enumerated: Some(now),
            ..crate::model::BluetoothActiveEnumeration::default()
        });
        bluetooth_ble.source_adapters = vec!["bluez:hci0".to_string()];
        bluetooth_ble.observations.push(sample_observation(now));
        let mut bluetooth_classic = BluetoothDeviceRecord::new("22:33:44:55:66:88", now);
        bluetooth_classic.transport = "BT".to_string();
        bluetooth_classic.device_type = Some("headset".to_string());
        bluetooth_classic.advertised_name = Some("ClassicHeadset".to_string());
        bluetooth_classic.source_adapters = vec!["bluez:hci1".to_string()];
        bluetooth_classic.observations.push(sample_observation(now));

        let kmz_path = manager
            .export_location_logs_kmz(
                &[ap],
                &[client, client_unassociated],
                &[bluetooth_ble, bluetooth_classic],
            )
            .expect("export kmz");
        let file = File::open(&kmz_path).expect("open kmz");
        let mut archive = zip::ZipArchive::new(file).expect("open zip archive");
        assert_eq!(archive.len(), 1, "expected KMZ to contain only doc.kml");
        let entry_name = archive
            .by_index(0)
            .expect("read first kmz entry")
            .name()
            .to_string();
        assert_eq!(entry_name, "doc.kml");
        let mut doc = archive.by_name("doc.kml").expect("read doc.kml");
        let mut xml = String::new();
        doc.read_to_string(&mut xml).expect("read kml string");

        let ap_idx = xml
            .find("<Folder><name>Access Points</name>")
            .expect("find AP folder");
        let client_idx = xml
            .find("<Folder><name>Clients</name>")
            .expect("find client folder");
        let bt_idx = xml
            .find("<Folder><name>Bluetooth</name>")
            .expect("find bluetooth folder");
        assert!(ap_idx < client_idx && client_idx < bt_idx);

        for expected in [
            "<Folder><name>Access Points</name>",
            "<Folder><name>Clients</name>",
            "<Folder><name>Bluetooth</name>",
            "<Data name=\"encryption_class\"><value>wpa3</value></Data>",
            "<Data name=\"bssid\"><value>11:22:33:44:55:66</value></Data>",
            "<Data name=\"associated_bssid\"><value>11:22:33:44:55:66</value></Data>",
            "<Data name=\"mac\"><value>22:33:44:55:66:77</value></Data>",
            "<Data name=\"source_adapters\"><value>wlan0mon;wlan1mon</value></Data>",
            "<Data name=\"source_adapters\"><value>wlan0mon</value></Data>",
            "<Data name=\"source_adapters\"><value>bluez:hci0</value></Data>",
            "<Data name=\"source_adapters\"><value>wlan1mon</value></Data>",
            "<Data name=\"source_adapters\"><value>bluez:hci1</value></Data>",
            "<Data name=\"alias\"><value>TagAlias</value></Data>",
            "<Data name=\"address_type\"><value>random</value></Data>",
            "<Data name=\"class_of_device\"><value>0x000000</value></Data>",
            "<Data name=\"mfgr_names\"><value>Acme</value></Data>",
            "<Data name=\"uuid_names\"><value>Battery Service</value></Data>",
            "<Data name=\"active_connected\"><value>true</value></Data>",
            "<Data name=\"active_paired\"><value>true</value></Data>",
            "<Data name=\"active_services_resolved\"><value>true</value></Data>",
            "<Data name=\"active_battery_percent\"><value>82</value></Data>",
            "<Data name=\"active_tx_power_dbm\"><value>-4</value></Data>",
            "<Data name=\"device_type\"><value>wifi_ap</value></Data>",
            "<Data name=\"device_type\"><value>wifi_client</value></Data>",
            "<Data name=\"device_type\"><value>bluetooth</value></Data>",
            "<styleUrl>#ap-wpa3</styleUrl>",
            "<styleUrl>#client-associated</styleUrl>",
            "<styleUrl>#client-unassociated</styleUrl>",
            "<styleUrl>#bluetooth-ble</styleUrl>",
            "<styleUrl>#bluetooth-classic</styleUrl>",
        ] {
            assert!(
                xml.contains(expected),
                "kmz doc.kml missing expected fragment: {expected}"
            );
        }
        assert!(xml.contains("<Data name=\"active_last_enumerated\"><value>"));
    }

    #[test]
    fn client_kml_placemark_style_tracks_association_presence() {
        let now = Utc::now();
        let mut associated = ClientRecord::new("AA:BB:CC:DD:EE:01", now);
        associated.associated_ap = Some("11:22:33:44:55:66".to_string());
        associated.observations.push(sample_observation(now));

        let mut unassociated = ClientRecord::new("AA:BB:CC:DD:EE:02", now);
        unassociated.associated_ap = None;
        unassociated.observations.push(sample_observation(now));

        let mut map = HashMap::new();
        map.insert("11:22:33:44:55:66".to_string(), "SecureNet".to_string());
        let xml = build_client_kml_placemarks(&[associated, unassociated], &map);
        assert!(xml.contains("<styleUrl>#client-associated</styleUrl>"));
        assert!(xml.contains("<styleUrl>#client-unassociated</styleUrl>"));
        assert!(xml.contains("<Data name=\"associated_state\"><value>associated</value></Data>"));
        assert!(xml.contains("<Data name=\"associated_state\"><value>unassociated</value></Data>"));
    }

    #[test]
    fn bluetooth_kml_placemark_style_tracks_transport() {
        let now = Utc::now();
        let mut ble = BluetoothDeviceRecord::new("22:33:44:55:66:77", now);
        ble.transport = "BLE".to_string();
        ble.observations.push(sample_observation(now));

        let mut classic = BluetoothDeviceRecord::new("22:33:44:55:66:88", now);
        classic.transport = "BT".to_string();
        classic.observations.push(sample_observation(now));

        let xml = build_bluetooth_kml_placemarks(&[ble, classic]);
        assert!(xml.contains("<styleUrl>#bluetooth-ble</styleUrl>"));
        assert!(xml.contains("<styleUrl>#bluetooth-classic</styleUrl>"));
    }

    #[test]
    fn bluetooth_transport_class_normalizes_common_variants() {
        assert_eq!(bluetooth_transport_class("BLE"), "ble");
        assert_eq!(bluetooth_transport_class("le"), "ble");
        assert_eq!(bluetooth_transport_class("btle"), "ble");
        assert_eq!(bluetooth_transport_class("Bluetooth LE"), "ble");
        assert_eq!(bluetooth_transport_class("Low Energy"), "ble");
        assert_eq!(bluetooth_transport_class("BT"), "classic");
        assert_eq!(bluetooth_transport_class("BR/EDR"), "classic");
        assert_eq!(bluetooth_transport_class("BR-EDR"), "classic");
        assert_eq!(bluetooth_transport_class("BREDR"), "classic");
        assert_eq!(bluetooth_transport_class("Bluetooth Classic"), "classic");
        assert_eq!(bluetooth_transport_class("Unknown"), "unknown");
    }

    #[test]
    fn nearest_fix_picks_closest_timestamp() {
        let base = Utc::now();
        let earlier = GeoObservation {
            timestamp: base - chrono::Duration::seconds(2),
            latitude: 37.0,
            longitude: -122.0,
            altitude_m: Some(10.0),
            rssi_dbm: Some(-60),
        };
        let later = GeoObservation {
            timestamp: base + chrono::Duration::seconds(5),
            latitude: 38.0,
            longitude: -121.0,
            altitude_m: Some(11.0),
            rssi_dbm: Some(-61),
        };
        let track = vec![earlier.clone(), later];
        let chosen = nearest_fix(&track, base).expect("nearest fix");
        assert_eq!(chosen.latitude, earlier.latitude);
        assert_eq!(chosen.longitude, earlier.longitude);
    }

    #[test]
    fn nearest_fix_rejects_points_outside_time_window() {
        let base = Utc::now();
        let far = GeoObservation {
            timestamp: base - chrono::Duration::seconds(30),
            latitude: 37.0,
            longitude: -122.0,
            altitude_m: None,
            rssi_dbm: None,
        };
        assert!(nearest_fix(&[far], base).is_none());
    }

    #[test]
    fn nearest_fix_rejects_invalid_coordinates() {
        let base = Utc::now();
        let invalid = GeoObservation {
            timestamp: base,
            latitude: 95.0,
            longitude: -122.0,
            altitude_m: None,
            rssi_dbm: None,
        };
        assert!(nearest_fix(&[invalid], base).is_none());
    }
}

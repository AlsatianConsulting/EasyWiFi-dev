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
                    "country_code_80211d": ap.country_code_80211d.clone().unwrap_or_default(),
                    "channel": ap.channel,
                    "frequency_mhz": ap.frequency_mhz,
                    "encryption_short": ap.encryption_short.clone(),
                    "encryption_full": ap.encryption_full.clone(),
                    "encryption_class": class.label(),
                    "rssi_dbm": ap.rssi_dbm,
                    "number_of_clients": ap.number_of_clients,
                    "first_seen": ap.first_seen.to_rfc3339(),
                    "last_seen": ap.last_seen.to_rfc3339(),
                    "handshake_count": ap.handshake_count,
                    "uptime_beacons": ap.uptime_beacons,
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
                json!({
                    "mac": client.mac.clone(),
                    "oui_manufacturer": client.oui_manufacturer.clone().unwrap_or_default(),
                    "associated_bssid": associated_bssid,
                    "associated_ssid": associated_ssid,
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

#[derive(Debug, Clone, Copy)]
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
            marks.push_str(&kml_placemark(&label, obs, "client", &description, &fields));
        }
    }
    marks
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
                ("mac".to_string(), dev.mac.clone()),
                ("transport".to_string(), dev.transport.clone()),
                (
                    "oui_manufacturer".to_string(),
                    dev.oui_manufacturer.clone().unwrap_or_default(),
                ),
                (
                    "bluetooth_type".to_string(),
                    dev.device_type.clone().unwrap_or_default(),
                ),
                ("rssi_dbm".to_string(), rssi.to_string()),
                ("mfgr_ids".to_string(), dev.mfgr_ids.join(";")),
                ("uuids".to_string(), dev.uuids.join(";")),
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
            let description = format!(
                "MAC={} | Name={} | Transport={} | Type={} | RSSI={} dBm",
                dev.mac,
                device_name,
                dev.transport,
                dev.device_type.clone().unwrap_or_default(),
                rssi,
            );
            marks.push_str(&kml_placemark(
                &label,
                obs,
                "bluetooth",
                &description,
                &fields,
            ));
        }
    }
    marks
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
<Style id=\"bluetooth\"><IconStyle><color>ffff00ff</color><scale>1.0</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/shaded_dot.png</href></Icon></IconStyle></Style>\n"
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

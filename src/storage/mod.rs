use crate::model::{
    AccessPointRecord, BluetoothActiveEnumeration, BluetoothDeviceRecord, ChannelUsagePoint,
    ClientRecord, GeoObservation, HandshakeRecord, PacketTypeBreakdown, SpectrumBand, WpsInfo,
};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
pub struct StorageEngine {
    conn: Arc<Mutex<Connection>>,
}

impl StorageEngine {
    pub fn open(database_path: &Path) -> Result<Self> {
        let conn = Connection::open(database_path)
            .with_context(|| format!("failed to open sqlite at {}", database_path.display()))?;
        let engine = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        engine.init_schema()?;
        Ok(engine)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock();

        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                started_at TEXT NOT NULL,
                output_dir TEXT NOT NULL,
                selected_interfaces_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS access_points (
                bssid TEXT PRIMARY KEY,
                ssid TEXT,
                oui_manufacturer TEXT,
                source_adapters_json TEXT NOT NULL DEFAULT '[]',
                country_code_80211d TEXT,
                channel INTEGER,
                frequency_mhz INTEGER,
                band TEXT NOT NULL,
                encryption_short TEXT NOT NULL,
                encryption_full TEXT NOT NULL,
                rssi_dbm INTEGER,
                number_of_clients INTEGER NOT NULL DEFAULT 0,
                first_seen TEXT NOT NULL,
                last_seen TEXT NOT NULL,
                handshake_count INTEGER NOT NULL DEFAULT 0,
                notes TEXT,
                uptime_beacons INTEGER,
                wps_json TEXT,
                packet_mix_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS clients (
                mac TEXT PRIMARY KEY,
                oui_manufacturer TEXT,
                source_adapters_json TEXT NOT NULL DEFAULT '[]',
                associated_ap TEXT,
                data_transferred_bytes INTEGER NOT NULL DEFAULT 0,
                rssi_dbm INTEGER,
                probes_json TEXT NOT NULL,
                first_seen TEXT NOT NULL,
                last_seen TEXT NOT NULL,
                seen_access_points_json TEXT NOT NULL,
                wps_json TEXT,
                handshake_networks_json TEXT NOT NULL,
                network_intel_json TEXT NOT NULL DEFAULT '{}'
            );

            CREATE TABLE IF NOT EXISTS bluetooth_devices (
                mac TEXT PRIMARY KEY,
                address_type TEXT,
                transport TEXT NOT NULL,
                oui_manufacturer TEXT,
                source_adapters_json TEXT NOT NULL DEFAULT '[]',
                advertised_name TEXT,
                alias TEXT,
                device_type TEXT,
                class_of_device TEXT,
                rssi_dbm INTEGER,
                first_seen TEXT NOT NULL,
                last_seen TEXT NOT NULL,
                mfgr_ids_json TEXT NOT NULL,
                mfgr_names_json TEXT NOT NULL,
                uuids_json TEXT NOT NULL,
                uuid_names_json TEXT NOT NULL,
                active_enum_json TEXT NOT NULL DEFAULT '{}'
            );

            CREATE TABLE IF NOT EXISTS ap_client_edges (
                bssid TEXT NOT NULL,
                client_mac TEXT NOT NULL,
                last_seen TEXT NOT NULL,
                PRIMARY KEY (bssid, client_mac),
                FOREIGN KEY (bssid) REFERENCES access_points(bssid) ON DELETE CASCADE,
                FOREIGN KEY (client_mac) REFERENCES clients(mac) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS observations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                device_type TEXT NOT NULL,
                device_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                latitude REAL NOT NULL,
                longitude REAL NOT NULL,
                altitude_m REAL,
                rssi_dbm INTEGER
            );

            CREATE TABLE IF NOT EXISTS handshakes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                bssid TEXT NOT NULL,
                client_mac TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                full_wpa2_4way INTEGER NOT NULL,
                pcap_path TEXT
            );

            CREATE TABLE IF NOT EXISTS channel_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                channel INTEGER NOT NULL,
                band TEXT NOT NULL,
                utilization_percent REAL NOT NULL,
                packets INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS gps_track (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                latitude REAL NOT NULL,
                longitude REAL NOT NULL,
                altitude_m REAL
            );
            "#,
        )
        .context("failed to initialize sqlite schema")?;

        ensure_column_exists(&conn, "access_points", "country_code_80211d", "TEXT")?;
        ensure_column_exists(&conn, "access_points", "rssi_dbm", "INTEGER")?;
        ensure_column_exists(
            &conn,
            "access_points",
            "source_adapters_json",
            "TEXT NOT NULL DEFAULT '[]'",
        )?;
        ensure_column_exists(
            &conn,
            "clients",
            "source_adapters_json",
            "TEXT NOT NULL DEFAULT '[]'",
        )?;
        ensure_column_exists(
            &conn,
            "clients",
            "network_intel_json",
            "TEXT NOT NULL DEFAULT '{}'",
        )?;
        ensure_column_exists(
            &conn,
            "bluetooth_devices",
            "active_enum_json",
            "TEXT NOT NULL DEFAULT '{}'",
        )?;
        ensure_column_exists(
            &conn,
            "bluetooth_devices",
            "source_adapters_json",
            "TEXT NOT NULL DEFAULT '[]'",
        )?;

        Ok(())
    }

    pub fn save_session(&self, metadata: &crate::model::SessionMetadata) -> Result<()> {
        let conn = self.conn.lock();
        let interfaces = serde_json::to_string(&metadata.selected_interfaces)?;
        conn.execute(
            r#"
            INSERT INTO sessions (id, started_at, output_dir, selected_interfaces_json)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(id) DO UPDATE SET
                started_at=excluded.started_at,
                output_dir=excluded.output_dir,
                selected_interfaces_json=excluded.selected_interfaces_json
            "#,
            params![
                metadata.id,
                metadata.started_at.to_rfc3339(),
                metadata.output_dir,
                interfaces
            ],
        )?;
        Ok(())
    }

    pub fn upsert_access_point(&self, ap: &AccessPointRecord) -> Result<()> {
        let conn = self.conn.lock();
        let wps_json = ap
            .wps
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?
            .unwrap_or_default();
        let source_adapters_json = serde_json::to_string(&ap.source_adapters)?;
        let packet_mix_json = serde_json::to_string(&ap.packet_mix)?;

        conn.execute(
            r#"
            INSERT INTO access_points (
                bssid, ssid, oui_manufacturer, source_adapters_json, country_code_80211d, channel, frequency_mhz, band,
                encryption_short, encryption_full, rssi_dbm, number_of_clients,
                first_seen, last_seen, handshake_count, notes, uptime_beacons,
                wps_json, packet_mix_json
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17,
                ?18, ?19
            )
            ON CONFLICT(bssid) DO UPDATE SET
                ssid=excluded.ssid,
                oui_manufacturer=excluded.oui_manufacturer,
                source_adapters_json=excluded.source_adapters_json,
                country_code_80211d=excluded.country_code_80211d,
                channel=excluded.channel,
                frequency_mhz=excluded.frequency_mhz,
                band=excluded.band,
                encryption_short=excluded.encryption_short,
                encryption_full=excluded.encryption_full,
                rssi_dbm=excluded.rssi_dbm,
                number_of_clients=excluded.number_of_clients,
                first_seen=MIN(access_points.first_seen, excluded.first_seen),
                last_seen=MAX(access_points.last_seen, excluded.last_seen),
                handshake_count=excluded.handshake_count,
                notes=excluded.notes,
                uptime_beacons=excluded.uptime_beacons,
                wps_json=excluded.wps_json,
                packet_mix_json=excluded.packet_mix_json
            "#,
            params![
                ap.bssid,
                ap.ssid,
                ap.oui_manufacturer,
                source_adapters_json,
                ap.country_code_80211d,
                ap.channel.map(|v| v as i64),
                ap.frequency_mhz.map(|v| v as i64),
                ap.band.label(),
                ap.encryption_short,
                ap.encryption_full,
                ap.rssi_dbm,
                ap.number_of_clients as i64,
                ap.first_seen.to_rfc3339(),
                ap.last_seen.to_rfc3339(),
                ap.handshake_count as i64,
                ap.notes,
                ap.uptime_beacons.map(|v| v as i64),
                wps_json,
                packet_mix_json,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_client(&self, client: &ClientRecord) -> Result<()> {
        let conn = self.conn.lock();
        let probes_json = serde_json::to_string(&client.probes)?;
        let seen_aps_json = serde_json::to_string(&client.seen_access_points)?;
        let handshake_networks_json = serde_json::to_string(&client.handshake_networks)?;
        let source_adapters_json = serde_json::to_string(&client.source_adapters)?;
        let network_intel_json = serde_json::to_string(&client.network_intel)?;
        let wps_json = client
            .wps
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?
            .unwrap_or_default();

        conn.execute(
            r#"
            INSERT INTO clients (
                mac, oui_manufacturer, source_adapters_json, associated_ap, data_transferred_bytes,
                rssi_dbm, probes_json, first_seen, last_seen,
                seen_access_points_json, wps_json, handshake_networks_json, network_intel_json
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8, ?9,
                ?10, ?11, ?12, ?13
            )
            ON CONFLICT(mac) DO UPDATE SET
                oui_manufacturer=excluded.oui_manufacturer,
                source_adapters_json=excluded.source_adapters_json,
                associated_ap=excluded.associated_ap,
                data_transferred_bytes=excluded.data_transferred_bytes,
                rssi_dbm=excluded.rssi_dbm,
                probes_json=excluded.probes_json,
                first_seen=MIN(clients.first_seen, excluded.first_seen),
                last_seen=excluded.last_seen,
                seen_access_points_json=excluded.seen_access_points_json,
                wps_json=excluded.wps_json,
                handshake_networks_json=excluded.handshake_networks_json,
                network_intel_json=excluded.network_intel_json
            "#,
            params![
                client.mac,
                client.oui_manufacturer,
                source_adapters_json,
                client.associated_ap,
                client.data_transferred_bytes as i64,
                client.rssi_dbm,
                probes_json,
                client.first_seen.to_rfc3339(),
                client.last_seen.to_rfc3339(),
                seen_aps_json,
                wps_json,
                handshake_networks_json,
                network_intel_json,
            ],
        )?;

        if let Some(ap) = &client.associated_ap {
            conn.execute(
                r#"
                INSERT INTO ap_client_edges (bssid, client_mac, last_seen)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(bssid, client_mac) DO UPDATE SET
                    last_seen=excluded.last_seen
                "#,
                params![ap, client.mac, client.last_seen.to_rfc3339()],
            )?;
        }

        Ok(())
    }

    pub fn upsert_bluetooth_device(&self, device: &BluetoothDeviceRecord) -> Result<()> {
        let conn = self.conn.lock();
        let mfgr_ids_json = serde_json::to_string(&device.mfgr_ids)?;
        let mfgr_names_json = serde_json::to_string(&device.mfgr_names)?;
        let uuids_json = serde_json::to_string(&device.uuids)?;
        let uuid_names_json = serde_json::to_string(&device.uuid_names)?;
        let source_adapters_json = serde_json::to_string(&device.source_adapters)?;
        let active_enum_json =
            serde_json::to_string(&device.active_enumeration.clone().unwrap_or_default())?;

        conn.execute(
            r#"
            INSERT INTO bluetooth_devices (
                mac, address_type, transport, oui_manufacturer, source_adapters_json, advertised_name, alias,
                device_type, class_of_device, rssi_dbm, first_seen, last_seen,
                mfgr_ids_json, mfgr_names_json, uuids_json, uuid_names_json, active_enum_json
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17
            )
            ON CONFLICT(mac) DO UPDATE SET
                address_type=excluded.address_type,
                transport=excluded.transport,
                oui_manufacturer=excluded.oui_manufacturer,
                source_adapters_json=excluded.source_adapters_json,
                advertised_name=excluded.advertised_name,
                alias=excluded.alias,
                device_type=excluded.device_type,
                class_of_device=excluded.class_of_device,
                rssi_dbm=excluded.rssi_dbm,
                first_seen=MIN(bluetooth_devices.first_seen, excluded.first_seen),
                last_seen=excluded.last_seen,
                mfgr_ids_json=excluded.mfgr_ids_json,
                mfgr_names_json=excluded.mfgr_names_json,
                uuids_json=excluded.uuids_json,
                uuid_names_json=excluded.uuid_names_json,
                active_enum_json=excluded.active_enum_json
            "#,
            params![
                device.mac,
                device.address_type,
                device.transport,
                device.oui_manufacturer,
                source_adapters_json,
                device.advertised_name,
                device.alias,
                device.device_type,
                device.class_of_device,
                device.rssi_dbm,
                device.first_seen.to_rfc3339(),
                device.last_seen.to_rfc3339(),
                mfgr_ids_json,
                mfgr_names_json,
                uuids_json,
                uuid_names_json,
                active_enum_json,
            ],
        )?;
        Ok(())
    }

    pub fn add_observation(
        &self,
        device_type: &str,
        device_id: &str,
        obs: &GeoObservation,
    ) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO observations (
                device_type, device_id, timestamp, latitude, longitude, altitude_m, rssi_dbm
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                device_type,
                device_id,
                obs.timestamp.to_rfc3339(),
                obs.latitude,
                obs.longitude,
                obs.altitude_m,
                obs.rssi_dbm,
            ],
        )?;
        Ok(())
    }

    pub fn add_handshake(&self, handshake: &HandshakeRecord) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO handshakes (bssid, client_mac, timestamp, full_wpa2_4way, pcap_path)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                handshake.bssid,
                handshake.client_mac,
                handshake.timestamp.to_rfc3339(),
                if handshake.full_wpa2_4way { 1 } else { 0 },
                handshake.pcap_path,
            ],
        )?;
        Ok(())
    }

    pub fn add_channel_usage(&self, usage: &ChannelUsagePoint) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO channel_usage (timestamp, channel, band, utilization_percent, packets)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                usage.timestamp.to_rfc3339(),
                usage.channel as i64,
                usage.band.label(),
                usage.utilization_percent,
                usage.packets as i64,
            ],
        )?;
        Ok(())
    }

    pub fn add_gps_track_point(&self, point: &GeoObservation) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO gps_track (timestamp, latitude, longitude, altitude_m)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                point.timestamp.to_rfc3339(),
                point.latitude,
                point.longitude,
                point.altitude_m,
            ],
        )?;
        Ok(())
    }

    pub fn load_gps_track(&self) -> Result<Vec<GeoObservation>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"
            SELECT timestamp, latitude, longitude, altitude_m
            FROM gps_track
            ORDER BY timestamp ASC
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            let ts: String = row.get(0)?;
            Ok(GeoObservation {
                timestamp: parse_ts(&ts).unwrap_or_else(|_| Utc::now()),
                latitude: row.get(1)?,
                longitude: row.get(2)?,
                altitude_m: row.get(3)?,
                rssi_dbm: None,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn load_access_points(&self) -> Result<Vec<AccessPointRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                bssid, ssid, oui_manufacturer, source_adapters_json, country_code_80211d, channel, frequency_mhz, band,
                encryption_short, encryption_full, rssi_dbm, number_of_clients,
                first_seen, last_seen, handshake_count, notes, uptime_beacons,
                wps_json, packet_mix_json
            FROM access_points
            ORDER BY last_seen DESC
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            let source_adapters_json: String = row.get(3)?;
            let channel_opt: Option<i64> = row.get(5)?;
            let frequency_opt: Option<i64> = row.get(6)?;
            let band_label: String = row.get(7)?;
            let first_seen: String = row.get(12)?;
            let last_seen: String = row.get(13)?;
            let wps_json: String = row.get(17)?;
            let packet_mix_json: String = row.get(18)?;

            let wps = if wps_json.trim().is_empty() {
                None
            } else {
                Some(serde_json::from_str::<WpsInfo>(&wps_json).unwrap_or_default())
            };

            let packet_mix = serde_json::from_str::<PacketTypeBreakdown>(&packet_mix_json)
                .unwrap_or_else(|_| PacketTypeBreakdown::default());

            Ok(AccessPointRecord {
                bssid: row.get(0)?,
                ssid: row.get(1)?,
                oui_manufacturer: row.get(2)?,
                source_adapters: serde_json::from_str(&source_adapters_json).unwrap_or_default(),
                country_code_80211d: row.get(4)?,
                channel: channel_opt.map(|v| v as u16),
                frequency_mhz: frequency_opt.map(|v| v as u32),
                band: parse_band_label(&band_label),
                encryption_short: row.get(8)?,
                encryption_full: row.get(9)?,
                rssi_dbm: row.get(10)?,
                number_of_clients: row.get::<_, i64>(11)? as u32,
                first_seen: parse_ts(&first_seen).unwrap_or_else(|_| Utc::now()),
                last_seen: parse_ts(&last_seen).unwrap_or_else(|_| Utc::now()),
                handshake_count: row.get::<_, i64>(14)? as u32,
                notes: row.get(15)?,
                uptime_beacons: row.get::<_, Option<i64>>(16)?.map(|v| v as u64),
                wps,
                packet_mix,
                observations: Vec::new(),
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            let mut ap = row?;
            ap.observations = self.load_observations("ap", &ap.bssid)?;
            ap.number_of_clients = self.client_count_for_ap(&ap.bssid)? as u32;
            result.push(ap);
        }

        Ok(result)
    }

    pub fn load_clients(&self) -> Result<Vec<ClientRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                mac, oui_manufacturer, source_adapters_json, associated_ap, data_transferred_bytes,
                rssi_dbm, probes_json, first_seen, last_seen,
                seen_access_points_json, wps_json, handshake_networks_json, network_intel_json
            FROM clients
            ORDER BY last_seen DESC
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            let source_adapters_json: String = row.get(2)?;
            let probes_json: String = row.get(6)?;
            let seen_aps_json: String = row.get(9)?;
            let wps_json: String = row.get(10)?;
            let handshake_networks_json: String = row.get(11)?;
            let network_intel_json: String = row.get(12)?;

            let probes = serde_json::from_str::<Vec<String>>(&probes_json).unwrap_or_default();
            let seen_access_points =
                serde_json::from_str::<Vec<String>>(&seen_aps_json).unwrap_or_default();
            let handshake_networks =
                serde_json::from_str::<Vec<String>>(&handshake_networks_json).unwrap_or_default();
            let network_intel = serde_json::from_str(&network_intel_json).unwrap_or_default();
            let wps = if wps_json.trim().is_empty() {
                None
            } else {
                Some(serde_json::from_str::<WpsInfo>(&wps_json).unwrap_or_default())
            };

            let first_seen: String = row.get(7)?;
            let last_seen: String = row.get(8)?;

            Ok(ClientRecord {
                mac: row.get(0)?,
                oui_manufacturer: row.get(1)?,
                source_adapters: serde_json::from_str(&source_adapters_json).unwrap_or_default(),
                associated_ap: row.get(3)?,
                data_transferred_bytes: row.get::<_, i64>(4)? as u64,
                rssi_dbm: row.get(5)?,
                probes,
                first_seen: parse_ts(&first_seen).unwrap_or_else(|_| Utc::now()),
                last_seen: parse_ts(&last_seen).unwrap_or_else(|_| Utc::now()),
                seen_access_points,
                wps,
                handshake_networks,
                network_intel,
                observations: Vec::new(),
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            let mut client = row?;
            client.observations = self.load_observations("client", &client.mac)?;
            result.push(client);
        }

        Ok(result)
    }

    pub fn load_clients_for_ap(&self, ap_bssid: &str) -> Result<Vec<ClientRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"
            SELECT c.mac
            FROM ap_client_edges e
            JOIN clients c ON c.mac = e.client_mac
            WHERE e.bssid = ?1
            ORDER BY e.last_seen DESC
            "#,
        )?;

        let macs = stmt
            .query_map(params![ap_bssid], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;

        drop(stmt);
        drop(conn);

        let all_clients = self.load_clients()?;
        let mut out = Vec::new();
        for mac in macs {
            if let Some(client) = all_clients.iter().find(|c| c.mac == mac) {
                out.push(client.clone());
            }
        }

        Ok(out)
    }

    pub fn load_bluetooth_devices(&self) -> Result<Vec<BluetoothDeviceRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                mac, address_type, transport, oui_manufacturer, source_adapters_json, advertised_name, alias,
                device_type, class_of_device, rssi_dbm, first_seen, last_seen,
                mfgr_ids_json, mfgr_names_json, uuids_json, uuid_names_json, active_enum_json
            FROM bluetooth_devices
            ORDER BY last_seen DESC
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            let source_adapters_json: String = row.get(4)?;
            let first_seen: String = row.get(10)?;
            let last_seen: String = row.get(11)?;
            let mfgr_ids_json: String = row.get(12)?;
            let mfgr_names_json: String = row.get(13)?;
            let uuids_json: String = row.get(14)?;
            let uuid_names_json: String = row.get(15)?;
            let active_enum_json: String = row.get(16)?;

            let mut record = BluetoothDeviceRecord {
                mac: row.get(0)?,
                address_type: row.get(1)?,
                transport: row.get(2)?,
                oui_manufacturer: row.get(3)?,
                source_adapters: serde_json::from_str(&source_adapters_json).unwrap_or_default(),
                advertised_name: row.get(5)?,
                alias: row.get(6)?,
                device_type: row.get(7)?,
                class_of_device: row.get(8)?,
                rssi_dbm: row.get(9)?,
                first_seen: parse_ts(&first_seen).unwrap_or_else(|_| Utc::now()),
                last_seen: parse_ts(&last_seen).unwrap_or_else(|_| Utc::now()),
                mfgr_ids: serde_json::from_str::<Vec<String>>(&mfgr_ids_json).unwrap_or_default(),
                mfgr_names: serde_json::from_str::<Vec<String>>(&mfgr_names_json)
                    .unwrap_or_default(),
                uuids: serde_json::from_str::<Vec<String>>(&uuids_json).unwrap_or_default(),
                uuid_names: serde_json::from_str::<Vec<String>>(&uuid_names_json)
                    .unwrap_or_default(),
                active_enumeration: serde_json::from_str::<BluetoothActiveEnumeration>(
                    &active_enum_json,
                )
                .ok()
                .filter(|enumeration| {
                    enumeration.last_enumerated.is_some()
                        || enumeration.connected
                        || enumeration.paired
                        || enumeration.trusted
                        || enumeration.blocked
                        || enumeration.services_resolved
                        || enumeration.tx_power_dbm.is_some()
                        || enumeration.battery_percent.is_some()
                        || enumeration.appearance_code.is_some()
                        || enumeration.icon.is_some()
                        || enumeration.modalias.is_some()
                        || !enumeration.services.is_empty()
                        || !enumeration.characteristics.is_empty()
                        || !enumeration.descriptors.is_empty()
                        || !enumeration.readable_attributes.is_empty()
                        || enumeration.last_error.is_some()
                }),
                observations: Vec::new(),
            };
            if record.last_seen < record.first_seen {
                std::mem::swap(&mut record.first_seen, &mut record.last_seen);
            }
            Ok(record)
        })?;

        let mut result = Vec::new();
        for row in rows {
            let mut device = row?;
            device.observations = self.load_observations("bluetooth", &device.mac)?;
            result.push(device);
        }

        Ok(result)
    }

    pub fn load_channel_usage(
        &self,
        band_filter: Option<SpectrumBand>,
    ) -> Result<Vec<ChannelUsagePoint>> {
        let conn = self.conn.lock();

        let rows = if let Some(band) = band_filter {
            let mut stmt = conn.prepare(
                r#"
                SELECT timestamp, channel, band, utilization_percent, packets
                FROM channel_usage
                WHERE band = ?1
                ORDER BY timestamp ASC
                "#,
            )?;
            let rows = stmt.query_map(params![band.label()], |row| {
                let ts: String = row.get(0)?;
                let band_label: String = row.get(2)?;
                Ok(ChannelUsagePoint {
                    timestamp: parse_ts(&ts).unwrap_or_else(|_| Utc::now()),
                    channel: row.get::<_, i64>(1)? as u16,
                    band: parse_band_label(&band_label),
                    utilization_percent: row.get(3)?,
                    packets: row.get::<_, i64>(4)? as u64,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            let mut stmt = conn.prepare(
                r#"
                SELECT timestamp, channel, band, utilization_percent, packets
                FROM channel_usage
                ORDER BY timestamp ASC
                "#,
            )?;
            let rows = stmt.query_map([], |row| {
                let ts: String = row.get(0)?;
                let band_label: String = row.get(2)?;
                Ok(ChannelUsagePoint {
                    timestamp: parse_ts(&ts).unwrap_or_else(|_| Utc::now()),
                    channel: row.get::<_, i64>(1)? as u16,
                    band: parse_band_label(&band_label),
                    utilization_percent: row.get(3)?,
                    packets: row.get::<_, i64>(4)? as u64,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        Ok(rows)
    }

    pub fn load_observations(
        &self,
        device_type: &str,
        device_id: &str,
    ) -> Result<Vec<GeoObservation>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"
            SELECT timestamp, latitude, longitude, altitude_m, rssi_dbm
            FROM observations
            WHERE device_type = ?1 AND device_id = ?2
            ORDER BY timestamp ASC
            "#,
        )?;

        let rows = stmt.query_map(params![device_type, device_id], |row| {
            let ts: String = row.get(0)?;
            Ok(GeoObservation {
                timestamp: parse_ts(&ts).unwrap_or_else(|_| Utc::now()),
                latitude: row.get(1)?,
                longitude: row.get(2)?,
                altitude_m: row.get(3)?,
                rssi_dbm: row.get(4)?,
            })
        })?;

        let mut observations = Vec::new();
        for row in rows {
            observations.push(row?);
        }
        Ok(observations)
    }

    pub fn load_handshakes_for_bssid(&self, bssid: &str) -> Result<Vec<HandshakeRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"
            SELECT bssid, client_mac, timestamp, full_wpa2_4way, pcap_path
            FROM handshakes
            WHERE bssid = ?1
            ORDER BY timestamp DESC
            "#,
        )?;

        let rows = stmt.query_map(params![bssid], |row| {
            let ts: String = row.get(2)?;
            Ok(HandshakeRecord {
                bssid: row.get(0)?,
                client_mac: row.get(1)?,
                timestamp: parse_ts(&ts).unwrap_or_else(|_| Utc::now()),
                full_wpa2_4way: row.get::<_, i64>(3)? == 1,
                pcap_path: row.get(4)?,
            })
        })?;

        let mut handshakes = Vec::new();
        for row in rows {
            handshakes.push(row?);
        }
        Ok(handshakes)
    }

    pub fn increment_handshake_count(&self, bssid: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            r#"
            UPDATE access_points
            SET handshake_count = handshake_count + 1
            WHERE bssid = ?1
            "#,
            params![bssid],
        )?;
        Ok(())
    }

    pub fn client_count_for_ap(&self, bssid: &str) -> Result<usize> {
        let conn = self.conn.lock();
        let count = conn
            .query_row(
                "SELECT COUNT(*) FROM ap_client_edges WHERE bssid = ?1",
                params![bssid],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        Ok(count as usize)
    }
}

fn parse_ts(value: &str) -> Result<DateTime<Utc>> {
    let ts = DateTime::parse_from_rfc3339(value)
        .map_err(|e| anyhow!("invalid timestamp {}: {}", value, e))?;
    Ok(ts.with_timezone(&Utc))
}

fn parse_band_label(label: &str) -> SpectrumBand {
    match label {
        "2.4 GHz" => SpectrumBand::Ghz2_4,
        "5 GHz" => SpectrumBand::Ghz5,
        "6 GHz" => SpectrumBand::Ghz6,
        _ => SpectrumBand::Unknown,
    }
}

fn ensure_column_exists(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let pragma = format!("PRAGMA table_info({})", table);
    let mut stmt = conn.prepare(&pragma)?;
    let cols = stmt.query_map([], |row| row.get::<_, String>(1))?;

    for col in cols {
        if col? == column {
            return Ok(());
        }
    }

    let alter = format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, definition);
    conn.execute(&alter, [])?;
    Ok(())
}

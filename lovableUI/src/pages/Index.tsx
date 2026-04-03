import { useState, useCallback, useEffect } from "react";
import HeaderBar from "@/components/HeaderBar";
import APTable from "@/components/APTable";
import DetailPanel from "@/components/DetailPanel";
import ClientsTab from "@/components/ClientsTab";
import ClientDetailPanel from "@/components/ClientDetailPanel";
import BluetoothTab from "@/components/BluetoothTab";
import BluetoothDetailPanel from "@/components/BluetoothDetailPanel";
import PreferencesDialog from "@/components/PreferencesDialog";
import {
  AccessPointRecord,
  ClientRecord,
  BluetoothDeviceRecord,
  AppSettings,
  defaultSettings,
} from "@/data/mockData";
import {
  LiveStateResponse,
  mapAccessPoint,
  mapBluetooth,
  mapClient,
} from "@/data/liveApi";
import { ResizablePanelGroup, ResizablePanel, ResizableHandle } from "@/components/ui/resizable";

const STORAGE_KEY = "easywifi-columns";

interface MetaInterface {
  name: string;
  if_type: string;
}

interface MetaWatchlistEntry {
  index: number;
  label: string;
  device_type: string;
  name: string;
  mac: string;
}

interface MetaResponse {
  interfaces: MetaInterface[];
  selected_interface: string | null;
  watchlist_entries: MetaWatchlistEntry[];
}

interface BtEnumerationStatus {
  message: string;
  is_error: boolean;
}

const loadColumns = (): { ap: string[]; client: string[]; bt: string[] } => {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved) return JSON.parse(saved);
  } catch {}
  return {
    ap: ["ssid", "bssid", "oui", "channel", "encryption", "rssi", "wps", "clients", "firstSeen", "lastSeen", "handshakes"],
    client: ["mac", "oui", "associatedAp", "rssi", "wps", "probes", "firstSeen", "lastSeen", "data"],
    bt: ["name", "mac", "oui", "rssi", "mfgrIds", "firstSeen", "lastSeen", "mfgrNames", "uuids"],
  };
};

const Index = () => {
  const [activeTab, setActiveTab] = useState("access-points");
  const [selectedAP, setSelectedAP] = useState<AccessPointRecord | null>(null);
  const [selectedClient, setSelectedClient] = useState<ClientRecord | null>(null);
  const [selectedBTDevice, setSelectedBTDevice] = useState<BluetoothDeviceRecord | null>(null);
  const [accessPoints, setAccessPoints] = useState<AccessPointRecord[]>([]);
  const [allClients, setAllClients] = useState<ClientRecord[]>([]);
  const [bluetoothDevices, setBluetoothDevices] = useState<BluetoothDeviceRecord[]>([]);
  const [scanningWifi, setScanningWifi] = useState(false);
  const [scanningBluetooth, setScanningBluetooth] = useState(false);
  const [apiError, setApiError] = useState<string | null>(null);
  const [prefsOpen, setPrefsOpen] = useState(false);
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [apFilter, setApFilter] = useState<string | null>(null);
  const [interfaces, setInterfaces] = useState<MetaInterface[]>([]);
  const [selectedInterface, setSelectedInterface] = useState<string | null>(null);
  const [watchlistEntries, setWatchlistEntries] = useState<MetaWatchlistEntry[]>([]);
  const [apActionStatus, setApActionStatus] = useState<BtEnumerationStatus | null>(null);
  const [btEnumerationStatus, setBtEnumerationStatus] = useState<Record<string, BtEnumerationStatus>>({});
  const [startWifiEnabled, setStartWifiEnabled] = useState(true);
  const [startBluetoothEnabled, setStartBluetoothEnabled] = useState(true);

  const [columns, setColumns] = useState(loadColumns);

  const saveColumns = (next: typeof columns) => {
    setColumns(next);
    localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  };

  const handleNavigateToClients = useCallback((apBssid: string) => {
    setApFilter(apBssid);
    setActiveTab("clients");
  }, []);

  const scanning = scanningWifi || scanningBluetooth;

  const refreshState = useCallback(async () => {
    try {
      const res = await fetch("/api/state");
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const body = (await res.json()) as LiveStateResponse;
      const aps = (body.access_points || []).map(mapAccessPoint);
      const clients = (body.clients || []).map(mapClient);
      const bts = (body.bluetooth_devices || []).map(mapBluetooth);
      setAccessPoints(aps);
      setAllClients(clients);
      setBluetoothDevices(bts);
      setScanningWifi(Boolean(body.scanning_wifi));
      setScanningBluetooth(Boolean(body.scanning_bluetooth));
      setBtEnumerationStatus(body.bt_enumeration_status ?? {});
      setApiError(null);
    } catch (err) {
      setApiError(String(err));
    }
  }, []);

  const postScan = useCallback(
    async (path: "/api/scan/start" | "/api/scan/stop") => {
      const res = await fetch(path, { method: "POST" });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      await refreshState();
    },
    [refreshState],
  );

  const postStartCustom = useCallback(
    async (wifiEnabled: boolean, bluetoothEnabled: boolean) => {
      const res = await fetch("/api/scan/start_custom", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ wifi_enabled: wifiEnabled, bluetooth_enabled: bluetoothEnabled }),
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      await refreshState();
    },
    [refreshState],
  );

  const refreshMeta = useCallback(async () => {
    try {
      const res = await fetch("/api/meta");
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const meta = (await res.json()) as MetaResponse;
      setInterfaces(meta.interfaces || []);
      setSelectedInterface(meta.selected_interface || null);
      setWatchlistEntries(meta.watchlist_entries || []);
    } catch (err) {
      setApiError(String(err));
    }
  }, []);

  useEffect(() => {
    refreshState();
    const t = window.setInterval(refreshState, 1200);
    return () => window.clearInterval(t);
  }, [refreshState]);

  useEffect(() => {
    refreshMeta();
  }, [refreshMeta]);

  useEffect(() => {
    if (selectedAP && !accessPoints.some((ap) => ap.bssid === selectedAP.bssid)) {
      setSelectedAP(accessPoints[0] ?? null);
    } else if (!selectedAP && accessPoints.length > 0) {
      setSelectedAP(accessPoints[0]);
    }
  }, [accessPoints, selectedAP]);

  useEffect(() => {
    if (selectedClient && !allClients.some((c) => c.mac === selectedClient.mac)) {
      setSelectedClient(allClients[0] ?? null);
    }
  }, [allClients, selectedClient]);

  useEffect(() => {
    if (
      selectedBTDevice &&
      !bluetoothDevices.some((d) => d.mac === selectedBTDevice.mac)
    ) {
      setSelectedBTDevice(bluetoothDevices[0] ?? null);
    }
  }, [bluetoothDevices, selectedBTDevice]);

  const renderMainContent = () => {
    switch (activeTab) {
      case "clients":
        return (
          <ClientsTab
            clients={allClients}
            selectedClient={selectedClient}
            onSelectClient={setSelectedClient}
            visibleColumns={columns.client}
            onVisibleColumnsChange={(cols) => saveColumns({ ...columns, client: cols })}
            apFilter={apFilter}
            onClearApFilter={() => setApFilter(null)}
          />
        );
      case "bluetooth":
        return (
          <BluetoothTab
            devices={bluetoothDevices}
            selectedDevice={selectedBTDevice}
            onSelectDevice={setSelectedBTDevice}
            visibleColumns={columns.bt}
            onVisibleColumnsChange={(cols) => saveColumns({ ...columns, bt: cols })}
          />
        );
      default:
        return (
          <APTable
            accessPoints={accessPoints}
            selectedAP={selectedAP}
            onSelectAP={setSelectedAP}
            visibleColumns={columns.ap}
            onVisibleColumnsChange={(cols) => saveColumns({ ...columns, ap: cols })}
          />
        );
    }
  };

  const renderDetailPanel = () => {
    if (!settings.showDetailPane) return null;
    switch (activeTab) {
      case "clients":
        return <ClientDetailPanel client={selectedClient} />;
      case "bluetooth":
        return (
          <BluetoothDetailPanel
            device={selectedBTDevice}
            enumerationStatus={selectedBTDevice ? (btEnumerationStatus[selectedBTDevice.mac] ?? null) : null}
            onEnumerateServices={(mac) => {
              setBtEnumerationStatus((prev) => ({
                ...prev,
                [mac]: { message: "enumeration running", is_error: false },
              }));
              fetch("/api/bluetooth/enumerate", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ mac }),
              })
                .then(async (res) => {
                  if (!res.ok) throw new Error(`HTTP ${res.status}`);
                  const body = (await res.json()) as BtEnumerationStatus;
                  setBtEnumerationStatus((prev) => ({ ...prev, [mac]: body }));
                  return refreshState();
                })
                .catch((err) => {
                  setBtEnumerationStatus((prev) => ({
                    ...prev,
                    [mac]: { message: String(err), is_error: true },
                  }));
                  setApiError(String(err));
                });
            }}
          />
        );
      default:
        return (
          <DetailPanel
            ap={selectedAP}
            onNavigateToClients={handleNavigateToClients}
            onLockToAp={(bssid) => {
              setApActionStatus({ message: "locking AP...", is_error: false });
              fetch("/api/ap/lock", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ bssid }),
              })
                .then((res) => {
                  if (!res.ok) throw new Error(`HTTP ${res.status}`);
                  setApActionStatus({ message: "AP lock applied", is_error: false });
                  return refreshState();
                })
                .catch((err) => {
                  setApActionStatus({ message: String(err), is_error: true });
                  setApiError(String(err));
                });
            }}
            actionStatus={apActionStatus}
          />
        );
    }
  };

  return (
    <div className="flex flex-col h-screen bg-background">
      <HeaderBar
        activeTab={activeTab}
        onTabChange={(tab) => { setActiveTab(tab); if (tab !== "clients") setApFilter(null); }}
        scanning={scanning}
        startWifiEnabled={startWifiEnabled}
        startBluetoothEnabled={startBluetoothEnabled}
        onStartWifiEnabledChange={setStartWifiEnabled}
        onStartBluetoothEnabledChange={setStartBluetoothEnabled}
        onToggleScan={() => {
          const action = scanning
            ? postScan("/api/scan/stop")
            : postStartCustom(startWifiEnabled, startBluetoothEnabled);
          action.catch((err) => {
            setApiError(String(err));
          });
        }}
        apCount={accessPoints.length}
        clientCount={allClients.length}
        onOpenPreferences={() => setPrefsOpen(true)}
      />

      <div className="flex-1 overflow-hidden">
        {settings.showDetailPane ? (
          <ResizablePanelGroup direction="horizontal">
            <ResizablePanel defaultSize={65} minSize={40}>
              {renderMainContent()}
            </ResizablePanel>
            <ResizableHandle withHandle />
            <ResizablePanel defaultSize={35} minSize={25}>
              {renderDetailPanel()}
            </ResizablePanel>
          </ResizablePanelGroup>
        ) : (
          renderMainContent()
        )}
      </div>

      {settings.showStatusBar && (
        <footer className="flex items-center justify-between border-t border-border bg-card px-4 py-1 text-[10px] text-muted-foreground">
          <div className="flex items-center gap-4">
            <span>Interface: <span className=" text-foreground">{accessPoints[0]?.sourceAdapters?.[0] ?? "—"}</span></span>
            <span>Mode: <span className=" text-foreground">{scanningWifi ? "Monitor" : "Idle"}</span></span>
          </div>
          <div className="flex items-center gap-4">
            {apiError && <span className=" text-destructive">API error: {apiError}</span>}
            <span className="flex items-center gap-1">
              <span className={`h-1.5 w-1.5 rounded-full ${scanning ? "bg-primary animate-pulse" : "bg-muted-foreground"}`} />
              {scanning ? "Scanning" : "Idle"} (Wi-Fi {scanningWifi ? "on" : "off"}, BT {scanningBluetooth ? "on" : "off"})
            </span>
          </div>
        </footer>
      )}

      <PreferencesDialog
        open={prefsOpen}
        onOpenChange={setPrefsOpen}
        settings={settings}
        onSettingsChange={setSettings}
        interfaces={interfaces.map((iface) => ({ name: iface.name, ifType: iface.if_type }))}
        selectedInterface={selectedInterface}
        onSelectInterface={async (name) => {
          const res = await fetch("/api/interface/select", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ interface_name: name }),
          });
          if (!res.ok) throw new Error(`HTTP ${res.status}`);
          await refreshMeta();
          await refreshState();
        }}
        watchlistEntries={watchlistEntries.map((entry) => ({
          index: entry.index,
          label: entry.label,
          deviceType: entry.device_type,
          name: entry.name,
          mac: entry.mac,
        }))}
        onAddWatchlistEntry={async (entry) => {
          const res = await fetch("/api/watchlist/add", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              label: entry.label,
              description: entry.description,
              name: entry.name,
              mac_or_bssid: entry.macOrBssid,
              oui: entry.oui,
              device_type: "wifi",
            }),
          });
          if (!res.ok) throw new Error(`HTTP ${res.status}`);
          await refreshMeta();
        }}
        onDeleteWatchlistEntry={async (index) => {
          const res = await fetch("/api/watchlist/delete", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ index }),
          });
          if (!res.ok) throw new Error(`HTTP ${res.status}`);
          await refreshMeta();
        }}
      />
    </div>
  );
};

export default Index;

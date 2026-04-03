import { Wifi, Activity, Bluetooth, Radio, Settings } from "lucide-react";

interface HeaderBarProps {
  activeTab: string;
  onTabChange: (tab: string) => void;
  scanning: boolean;
  onToggleScan: () => void;
  startWifiEnabled: boolean;
  startBluetoothEnabled: boolean;
  onStartWifiEnabledChange: (enabled: boolean) => void;
  onStartBluetoothEnabledChange: (enabled: boolean) => void;
  apCount: number;
  clientCount: number;
  onOpenPreferences: () => void;
}

const tabs = [
  { id: "access-points", label: "Access Points", icon: Wifi },
  { id: "clients", label: "Clients & Probes", icon: Activity },
  { id: "bluetooth", label: "Bluetooth", icon: Bluetooth },
];

const HeaderBar = ({
  activeTab,
  onTabChange,
  scanning,
  onToggleScan,
  startWifiEnabled,
  startBluetoothEnabled,
  onStartWifiEnabledChange,
  onStartBluetoothEnabledChange,
  apCount,
  clientCount,
  onOpenPreferences,
}: HeaderBarProps) => {
  return (
    <header className="flex items-center justify-between border-b border-border bg-card px-4 py-2">
      <div className="flex items-center gap-3">
        <div className="flex items-center gap-2">
          <Radio className="h-5 w-5 text-primary" />
          <span className="text-lg font-bold text-primary">EasyWiFi</span>
        </div>
        <span className="text-xs text-muted-foreground">Command Center</span>
      </div>

      <nav className="flex items-center gap-1">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            onClick={() => onTabChange(tab.id)}
            className={`flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors ${
              activeTab === tab.id
                ? "bg-primary text-primary-foreground"
                : "text-muted-foreground hover:bg-secondary hover:text-foreground"
            }`}
          >
            <tab.icon className="h-3.5 w-3.5" />
            {tab.label}
          </button>
        ))}
      </nav>

      <div className="flex items-center gap-3">
        <div className="flex items-center gap-4 text-xs text-muted-foreground">
          <span>APs: <span className=" text-foreground">{apCount}</span></span>
          <span>Clients: <span className=" text-foreground">{clientCount}</span></span>
        </div>

        <button
          onClick={onOpenPreferences}
          className="flex items-center gap-1 rounded-md px-2 py-1.5 text-xs text-muted-foreground hover:bg-secondary hover:text-foreground transition-colors"
          title="Preferences"
        >
          <Settings className="h-4 w-4" />
        </button>

        {!scanning && (
          <div className="flex items-center gap-1 rounded-md border border-border bg-secondary/40 p-1 text-[10px]">
            <button
              onClick={() => onStartWifiEnabledChange(!startWifiEnabled)}
              className={`rounded px-2 py-1 ${startWifiEnabled ? "bg-primary text-primary-foreground" : "text-muted-foreground"}`}
              title="Include Wi-Fi in next scan"
            >
              Wi-Fi
            </button>
            <button
              onClick={() => onStartBluetoothEnabledChange(!startBluetoothEnabled)}
              className={`rounded px-2 py-1 ${startBluetoothEnabled ? "bg-primary text-primary-foreground" : "text-muted-foreground"}`}
              title="Include Bluetooth in next scan"
            >
              Bluetooth
            </button>
          </div>
        )}

        <button
          onClick={onToggleScan}
          className={`flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors ${
            scanning
              ? "bg-destructive text-destructive-foreground"
              : "bg-primary text-primary-foreground"
          }`}
        >
          <span className={`h-2 w-2 rounded-full ${scanning ? "animate-pulse bg-primary-foreground" : "bg-primary-foreground/50"}`} />
          {scanning ? "Stop Scan" : "Start Scan"}
        </button>
      </div>
    </header>
  );
};

export default HeaderBar;

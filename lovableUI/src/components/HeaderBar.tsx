import { Wifi, Activity, Bluetooth, Radio, Settings } from "lucide-react";

interface HeaderBarProps {
  activeTab: string;
  onTabChange: (tab: string) => void;
  scanning: boolean;
  scanningWifi: boolean;
  currentHopChannel: number | null;
  onToggleScan: () => void;
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
  scanningWifi,
  currentHopChannel,
  onToggleScan,
  apCount,
  clientCount,
  onOpenPreferences,
}: HeaderBarProps) => {
  return (
    <header className="flex flex-col gap-2 border-b border-border bg-card px-2 py-2 md:flex-row md:items-center md:justify-between md:px-4">
      <div className="flex min-w-0 items-center justify-between gap-2 md:justify-start md:gap-3">
        <div className="flex items-center gap-2">
          <Radio className="h-4 w-4 text-primary md:h-5 md:w-5" />
          <span className="text-base font-bold text-primary md:text-lg">EasyWiFi</span>
        </div>
        <span className="text-[10px] text-muted-foreground md:text-xs">Command Center</span>
      </div>

      <nav className="flex w-full items-center gap-1 overflow-x-auto pb-1 md:w-auto md:pb-0">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            onClick={() => onTabChange(tab.id)}
            className={`flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-[10px] font-medium transition-colors md:gap-1.5 md:px-3 md:py-1.5 md:text-xs ${
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

      <div className="flex w-full items-center justify-between gap-2 md:w-auto md:justify-end md:gap-3">
        <div className="flex items-center gap-2 text-[10px] text-muted-foreground md:gap-4 md:text-xs">
          <span>APs: <span className=" text-foreground">{apCount}</span></span>
          <span>Clients: <span className=" text-foreground">{clientCount}</span></span>
          <span>
            Hop: <span className=" text-foreground">{scanningWifi ? (currentHopChannel ?? "—") : "—"}</span>
          </span>
        </div>

        <button
          onClick={onOpenPreferences}
          className="flex items-center gap-1 rounded-md px-1.5 py-1 text-[10px] text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground md:px-2 md:py-1.5 md:text-xs"
          title="Preferences"
        >
          <Settings className="h-3.5 w-3.5 md:h-4 md:w-4" />
        </button>

        <button
          onClick={onToggleScan}
          className={`flex items-center gap-1 rounded-md px-2 py-1 text-[10px] font-medium transition-colors md:gap-1.5 md:px-3 md:py-1.5 md:text-xs ${
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

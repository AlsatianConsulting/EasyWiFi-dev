import { useState } from "react";
import { AppSettings, defaultSettings } from "@/data/mockData";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";

interface PreferencesDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  settings: AppSettings;
  onSettingsChange: (settings: AppSettings) => void;
}

const Section = ({ title, children }: { title: string; children: React.ReactNode }) => (
  <div className="space-y-3">
    <h3 className="text-xs font-semibold uppercase tracking-wider text-primary">{title}</h3>
    <div className="space-y-3">{children}</div>
  </div>
);

const SettingRow = ({ label, description, children }: { label: string; description?: string; children: React.ReactNode }) => (
  <div className="flex items-center justify-between gap-4">
    <div>
      <Label className="text-xs font-medium">{label}</Label>
      {description && <p className="text-[10px] text-muted-foreground">{description}</p>}
    </div>
    {children}
  </div>
);

const PreferencesDialog = ({ open, onOpenChange, settings, onSettingsChange }: PreferencesDialogProps) => {
  const [local, setLocal] = useState<AppSettings>(settings);

  const update = (patch: Partial<AppSettings>) => {
    const next = { ...local, ...patch };
    setLocal(next);
    onSettingsChange(next);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="bg-card border-border max-w-lg max-h-[80vh] overflow-auto">
        <DialogHeader>
          <DialogTitle className="text-primary">Preferences</DialogTitle>
        </DialogHeader>

        <div className="space-y-5 mt-2">
          <Section title="View">
            <SettingRow label="Show Status Bar"><Switch checked={local.showStatusBar} onCheckedChange={(v) => update({ showStatusBar: v })} /></SettingRow>
            <SettingRow label="Show Detail Pane"><Switch checked={local.showDetailPane} onCheckedChange={(v) => update({ showDetailPane: v })} /></SettingRow>
            <SettingRow label="Show Device Pane"><Switch checked={local.showDevicePane} onCheckedChange={(v) => update({ showDevicePane: v })} /></SettingRow>
            <SettingRow label="Show Column Filters"><Switch checked={local.showColumnFilters} onCheckedChange={(v) => update({ showColumnFilters: v })} /></SettingRow>
            <SettingRow label="Inline Channel Usage" description="Show channel usage bars in AP table"><Switch checked={local.showApInlineChannelUsage} onCheckedChange={(v) => update({ showApInlineChannelUsage: v })} /></SettingRow>
            <SettingRow label="Dark Mode"><Switch checked={local.darkMode} onCheckedChange={(v) => update({ darkMode: v })} /></SettingRow>
            <SettingRow label="Use Zulu Time (UTC)" description="Display timestamps in UTC instead of local time"><Switch checked={local.useZuluTime} onCheckedChange={(v) => update({ useZuluTime: v })} /></SettingRow>
            <SettingRow label="Rows Per Page">
              <Select value={String(local.defaultRowsPerPage)} onValueChange={(v) => update({ defaultRowsPerPage: Number(v) })}>
                <SelectTrigger className="w-24 h-7 text-xs"><SelectValue /></SelectTrigger>
                <SelectContent>
                  {[25, 50, 100, 200].map(n => <SelectItem key={n} value={String(n)}>{n}</SelectItem>)}
                </SelectContent>
              </Select>
            </SettingRow>
          </Section>

          <Separator />

          <Section title="WiFi Capture">
            <SettingRow label="Packet Header Mode">
              <Select value={local.wifiPacketHeaderMode} onValueChange={(v) => update({ wifiPacketHeaderMode: v as "radiotap" | "ppi" })}>
                <SelectTrigger className="w-28 h-7 text-xs"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="radiotap">Radiotap</SelectItem>
                  <SelectItem value="ppi">PPI</SelectItem>
                </SelectContent>
              </Select>
            </SettingRow>
            <SettingRow label="Enable WiFi Frame Parsing"><Switch checked={local.enableWifiFrameParsing} onCheckedChange={(v) => update({ enableWifiFrameParsing: v })} /></SettingRow>
          </Section>

          <Separator />

          <Section title="Bluetooth">
            <SettingRow label="Bluetooth Scanning"><Switch checked={local.bluetoothEnabled} onCheckedChange={(v) => update({ bluetoothEnabled: v })} /></SettingRow>
            <SettingRow label="Scan Source">
              <Select value={local.bluetoothScanSource} onValueChange={(v) => update({ bluetoothScanSource: v as "bluez" | "ubertooth" | "both" })}>
                <SelectTrigger className="w-28 h-7 text-xs"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="bluez">BlueZ</SelectItem>
                  <SelectItem value="ubertooth">Ubertooth</SelectItem>
                  <SelectItem value="both">Both</SelectItem>
                </SelectContent>
              </Select>
            </SettingRow>
            <SettingRow label="Scan Timeout (sec)">
              <Input type="number" value={local.bluetoothScanTimeoutSecs} onChange={(e) => update({ bluetoothScanTimeoutSecs: Number(e.target.value) })} className="w-20 h-7 text-xs " />
            </SettingRow>
            <SettingRow label="Scan Pause (ms)">
              <Input type="number" value={local.bluetoothScanPauseMs} onChange={(e) => update({ bluetoothScanPauseMs: Number(e.target.value) })} className="w-20 h-7 text-xs " />
            </SettingRow>
          </Section>

          <Separator />

          <Section title="GPS">
            <SettingRow label="GPS Provider">
              <Select value={local.gps} onValueChange={(v) => update({ gps: v as AppSettings["gps"] })}>
                <SelectTrigger className="w-28 h-7 text-xs"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="disabled">Disabled</SelectItem>
                  <SelectItem value="gpsd">GPSD</SelectItem>
                  <SelectItem value="serial">Serial</SelectItem>
                  <SelectItem value="static">Static</SelectItem>
                </SelectContent>
              </Select>
            </SettingRow>
          </Section>

          <Separator />

          <Section title="Output & Storage">
            <SettingRow label="Output to Files"><Switch checked={local.outputToFiles} onCheckedChange={(v) => update({ outputToFiles: v })} /></SettingRow>
            <SettingRow label="Output Directory">
              <Input value={local.outputRoot} onChange={(e) => update({ outputRoot: e.target.value })} className="w-48 h-7 text-xs " />
            </SettingRow>
            <SettingRow label="Store SQLite"><Switch checked={local.storeSqlite} onCheckedChange={(v) => update({ storeSqlite: v })} /></SettingRow>
            <SettingRow label="Auto-create Exports on Startup"><Switch checked={local.autoCreateExportsOnStartup} onCheckedChange={(v) => update({ autoCreateExportsOnStartup: v })} /></SettingRow>
          </Section>

          <Separator />

          <Section title="Data Sources">
            <SettingRow label="OUI Source Path">
              <Input value={local.ouiSourcePath} onChange={(e) => update({ ouiSourcePath: e.target.value })} className="w-48 h-7 text-xs " />
            </SettingRow>
            <SettingRow label="Auto-check OUI Updates"><Switch checked={local.autoCheckOuiUpdates} onCheckedChange={(v) => update({ autoCheckOuiUpdates: v })} /></SettingRow>
            <SettingRow label="GeoIP City DB Path">
              <Input value={local.geoipCityDbPath} onChange={(e) => update({ geoipCityDbPath: e.target.value })} className="w-48 h-7 text-xs " />
            </SettingRow>
          </Section>

          <Separator />

          <Section title="Alerts">
            <SettingRow label="Handshake Alerts"><Switch checked={local.enableHandshakeAlerts} onCheckedChange={(v) => update({ enableHandshakeAlerts: v })} /></SettingRow>
            <SettingRow label="Watchlist Alerts"><Switch checked={local.enableWatchlistAlerts} onCheckedChange={(v) => update({ enableWatchlistAlerts: v })} /></SettingRow>
          </Section>
        </div>
      </DialogContent>
    </Dialog>
  );
};

export default PreferencesDialog;

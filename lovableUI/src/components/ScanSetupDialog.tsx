import { useEffect, useMemo, useState } from "react";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";

interface InterfaceOption {
  name: string;
  ifType: string;
}

interface BluetoothControllerOption {
  id: string;
  name: string;
}

export interface ScanSetupModel {
  wifi_enabled: boolean;
  bluetooth_enabled: boolean;
  selected_interface: string | null;
  mode: "locked" | "hop_specific";
  locked_channel: number | null;
  locked_ht_mode: string | null;
  hop_channels: number[];
  hop_dwell_ms: number;
  hop_ht_mode: string;
  bluetooth_controller: string | null;
}

interface ScanSetupDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  initialSection: "wifi" | "bluetooth";
  setup: ScanSetupModel | null;
  interfaces: InterfaceOption[];
  bluetoothControllers: BluetoothControllerOption[];
  onApply: (setup: ScanSetupModel) => Promise<void>;
}

interface InterfaceCapabilities {
  channels: number[];
  ht_modes: string[];
}

const defaultCapabilities: InterfaceCapabilities = {
  channels: [1, 6, 11],
  ht_modes: ["HT20"],
};

const ScanSetupDialog = ({
  open,
  onOpenChange,
  initialSection,
  setup,
  interfaces,
  bluetoothControllers,
  onApply,
}: ScanSetupDialogProps) => {
  const [section, setSection] = useState<"wifi" | "bluetooth">(initialSection);
  const [model, setModel] = useState<ScanSetupModel | null>(setup);
  const [caps, setCaps] = useState<InterfaceCapabilities>(defaultCapabilities);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (open) {
      setSection(initialSection);
      setModel(setup);
    }
  }, [open, initialSection, setup]);

  const selectedInterface = model?.selected_interface ?? interfaces[0]?.name ?? "";

  useEffect(() => {
    if (!open || !selectedInterface) return;
    fetch(`/api/interface/capabilities?name=${encodeURIComponent(selectedInterface)}`)
      .then(async (res) => {
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const body = (await res.json()) as InterfaceCapabilities;
        setCaps({
          channels: body.channels?.length ? body.channels : defaultCapabilities.channels,
          ht_modes: body.ht_modes?.length ? body.ht_modes : defaultCapabilities.ht_modes,
        });
      })
      .catch(() => setCaps(defaultCapabilities));
  }, [open, selectedInterface]);

  const sortedChannels = useMemo(
    () => [...(caps.channels || [])].sort((a, b) => a - b),
    [caps.channels],
  );

  if (!model) return null;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] max-w-xl overflow-auto bg-card border-border">
        <DialogHeader>
          <DialogTitle className="text-primary">Scan Setup</DialogTitle>
        </DialogHeader>

        <div className="mt-2 flex items-center gap-2">
          <button
            className={`rounded px-2 py-1 text-xs ${section === "wifi" ? "bg-primary text-primary-foreground" : "bg-secondary text-muted-foreground"}`}
            onClick={() => setSection("wifi")}
          >
            Wi-Fi
          </button>
          <button
            className={`rounded px-2 py-1 text-xs ${section === "bluetooth" ? "bg-primary text-primary-foreground" : "bg-secondary text-muted-foreground"}`}
            onClick={() => setSection("bluetooth")}
          >
            Bluetooth
          </button>
        </div>

        {section === "wifi" ? (
          <div className="space-y-3 mt-3">
            <div className="flex items-center justify-between">
              <span className="text-xs">Enable Wi-Fi Scan</span>
              <Switch
                checked={model.wifi_enabled}
                onCheckedChange={(v) => setModel((m) => (m ? { ...m, wifi_enabled: v } : m))}
              />
            </div>

            <div className="space-y-1">
              <span className="text-xs text-muted-foreground">Interface</span>
              <Select
                value={selectedInterface}
                onValueChange={(value) =>
                  setModel((m) => (m ? { ...m, selected_interface: value } : m))
                }
              >
                <SelectTrigger className="h-8 text-xs">
                  <SelectValue placeholder="Select interface" />
                </SelectTrigger>
                <SelectContent>
                  {interfaces.map((iface) => (
                    <SelectItem key={iface.name} value={iface.name}>
                      {iface.name} ({iface.ifType})
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1">
              <span className="text-xs text-muted-foreground">Channel Mode</span>
              <Select
                value={model.mode}
                onValueChange={(value) =>
                  setModel((m) => (m ? { ...m, mode: value as "locked" | "hop_specific" } : m))
                }
              >
                <SelectTrigger className="h-8 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="locked">Lock Channel</SelectItem>
                  <SelectItem value="hop_specific">Hop Specific Channels</SelectItem>
                </SelectContent>
              </Select>
            </div>

            {model.mode === "locked" ? (
              <div className="grid grid-cols-2 gap-2">
                <div className="space-y-1">
                  <span className="text-xs text-muted-foreground">Channel</span>
                  <Select
                    value={String(model.locked_channel ?? sortedChannels[0] ?? 1)}
                    onValueChange={(value) =>
                      setModel((m) =>
                        m ? { ...m, locked_channel: Number(value) || 1 } : m,
                      )
                    }
                  >
                    <SelectTrigger className="h-8 text-xs">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {sortedChannels.map((ch) => (
                        <SelectItem key={ch} value={String(ch)}>
                          {ch}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1">
                  <span className="text-xs text-muted-foreground">Bandwidth</span>
                  <Select
                    value={model.locked_ht_mode ?? caps.ht_modes[0] ?? "HT20"}
                    onValueChange={(value) =>
                      setModel((m) => (m ? { ...m, locked_ht_mode: value } : m))
                    }
                  >
                    <SelectTrigger className="h-8 text-xs">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {caps.ht_modes.map((mode) => (
                        <SelectItem key={mode} value={mode}>
                          {mode}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              </div>
            ) : (
              <div className="space-y-2">
                <span className="text-xs text-muted-foreground">Hop Channels</span>
                <div className="max-h-32 overflow-auto rounded border border-border p-2 grid grid-cols-5 gap-1">
                  {sortedChannels.map((ch) => {
                    const checked = model.hop_channels.includes(ch);
                    return (
                      <label key={ch} className="flex items-center gap-1 text-[11px]">
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={(e) => {
                            const enabled = e.target.checked;
                            setModel((m) => {
                              if (!m) return m;
                              const next = enabled
                                ? [...m.hop_channels, ch]
                                : m.hop_channels.filter((value) => value !== ch);
                              return { ...m, hop_channels: next.sort((a, b) => a - b) };
                            });
                          }}
                        />
                        {ch}
                      </label>
                    );
                  })}
                </div>
                <div className="grid grid-cols-2 gap-2">
                  <div className="space-y-1">
                    <span className="text-xs text-muted-foreground">Hop Dwell (ms)</span>
                    <Input
                      type="number"
                      className="h-8 text-xs"
                      value={model.hop_dwell_ms}
                      onChange={(e) =>
                        setModel((m) =>
                          m ? { ...m, hop_dwell_ms: Number(e.target.value) || 200 } : m,
                        )
                      }
                    />
                  </div>
                  <div className="space-y-1">
                    <span className="text-xs text-muted-foreground">Hop Bandwidth</span>
                    <Select
                      value={model.hop_ht_mode || caps.ht_modes[0] || "HT20"}
                      onValueChange={(value) =>
                        setModel((m) => (m ? { ...m, hop_ht_mode: value } : m))
                      }
                    >
                      <SelectTrigger className="h-8 text-xs">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {caps.ht_modes.map((mode) => (
                          <SelectItem key={mode} value={mode}>
                            {mode}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                </div>
              </div>
            )}
          </div>
        ) : (
          <div className="space-y-3 mt-3">
            <div className="flex items-center justify-between">
              <span className="text-xs">Enable Bluetooth Scan</span>
              <Switch
                checked={model.bluetooth_enabled}
                onCheckedChange={(v) => setModel((m) => (m ? { ...m, bluetooth_enabled: v } : m))}
              />
            </div>

            <div className="space-y-1">
              <span className="text-xs text-muted-foreground">Controller</span>
              <Select
                value={model.bluetooth_controller ?? "default"}
                onValueChange={(value) =>
                  setModel((m) => (m ? { ...m, bluetooth_controller: value === "default" ? null : value } : m))
                }
              >
                <SelectTrigger className="h-8 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="default">System Default</SelectItem>
                  {bluetoothControllers.map((ctrl) => (
                    <SelectItem key={ctrl.id} value={ctrl.id}>
                      {ctrl.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
        )}

        <div className="mt-4 flex justify-end gap-2">
          <button
            className="rounded border border-border px-2 py-1 text-xs"
            onClick={() => onOpenChange(false)}
          >
            Cancel
          </button>
          <button
            className="rounded bg-primary px-2 py-1 text-xs text-primary-foreground disabled:opacity-60"
            disabled={saving}
            onClick={async () => {
              setSaving(true);
              try {
                await onApply(model);
                onOpenChange(false);
              } finally {
                setSaving(false);
              }
            }}
          >
            {saving ? "Saving..." : "Save"}
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
};

export default ScanSetupDialog;

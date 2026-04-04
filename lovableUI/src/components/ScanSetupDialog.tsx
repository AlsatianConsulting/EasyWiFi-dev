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
  channel_ht_modes?: Record<string, string[]>;
  wifi_band: "all" | "2.4" | "5" | "6";
  wifi_bandwidths: string[];
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

const modeAllowedForChannel = (mode: string, channel: number): boolean => {
  const normalized = mode.trim().toUpperCase();
  if (!normalized) return false;
  const is24 = channel >= 1 && channel <= 14;
  const is5 = channel >= 32 && channel <= 177;
  const is6 = channel > 177;
  if (is24) {
    return ["NOHT", "HT20", "HT40+", "HT40-", "5MHZ", "10MHZ"].includes(normalized);
  }
  if (is5) {
    return [
      "NOHT",
      "HT20",
      "HT40+",
      "HT40-",
      "5MHZ",
      "10MHZ",
      "80MHZ",
      "160MHZ",
      "80+80MHZ",
    ].includes(normalized);
  }
  if (is6) {
    return ["HT20", "HT40+", "HT40-", "80MHZ", "160MHZ", "80+80MHZ"].includes(normalized);
  }
  return false;
};

const modesForChannel = (modes: string[], channel: number): string[] =>
  modes.filter((mode) => modeAllowedForChannel(mode, channel));

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
  const [hopPreset, setHopPreset] = useState<"all" | "all_24" | "all_5" | "all_6" | "selected">("selected");

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

  useEffect(() => {
    if (!model) return;
    if (model.wifi_bandwidths.length === 0 && caps.ht_modes.length > 0) {
      setModel({ ...model, wifi_bandwidths: [...caps.ht_modes] });
    }
  }, [caps.ht_modes, model]);

  const sortedChannels = useMemo(
    () => [...(caps.channels || [])].sort((a, b) => a - b),
    [caps.channels],
  );
  const bandChannels = useMemo(() => {
    const band = model?.wifi_band ?? "all";
    if (band === "all") return sortedChannels;
    if (band === "2.4") return sortedChannels.filter((ch) => ch >= 1 && ch <= 14);
    if (band === "5") return sortedChannels.filter((ch) => ch >= 32 && ch <= 177);
    return sortedChannels.filter((ch) => ch > 177);
  }, [model?.wifi_band, sortedChannels]);

  useEffect(() => {
    if (!open || !model) return;
    const selected = [...(model.hop_channels || [])].sort((a, b) => a - b);
    const all = [...sortedChannels].sort((a, b) => a - b);
    const all24 = sortedChannels.filter((ch) => ch >= 1 && ch <= 14);
    const all5 = sortedChannels.filter((ch) => ch >= 32 && ch <= 177);
    const all6 = sortedChannels.filter((ch) => ch > 177);
    const same = (left: number[], right: number[]) =>
      left.length === right.length && left.every((v, i) => v === right[i]);
    if (all.length > 0 && same(selected, all)) {
      setHopPreset("all");
    } else if (all24.length > 0 && same(selected, all24)) {
      setHopPreset("all_24");
    } else if (all5.length > 0 && same(selected, all5)) {
      setHopPreset("all_5");
    } else if (all6.length > 0 && same(selected, all6)) {
      setHopPreset("all_6");
    } else {
      setHopPreset("selected");
    }
  }, [open, model, sortedChannels]);

  const hopPresetChannels = useMemo(() => {
    if (hopPreset === "all") return sortedChannels;
    if (hopPreset === "all_24") return sortedChannels.filter((ch) => ch >= 1 && ch <= 14);
    if (hopPreset === "all_5") return sortedChannels.filter((ch) => ch >= 32 && ch <= 177);
    if (hopPreset === "all_6") return sortedChannels.filter((ch) => ch > 177);
    return bandChannels;
  }, [hopPreset, sortedChannels, bandChannels]);

  const effectiveHopChannels = useMemo(() => {
    if (!model || model.mode !== "hop_specific") return [] as number[];
    if (hopPreset === "selected") return [...model.hop_channels].sort((a, b) => a - b);
    return [...hopPresetChannels].sort((a, b) => a - b);
  }, [model, hopPreset, hopPresetChannels]);
  const lockChannel = model.locked_channel ?? bandChannels[0] ?? sortedChannels[0] ?? 1;
  const lockAllowedModes = useMemo(() => modesForChannel(caps.ht_modes, lockChannel), [caps.ht_modes, lockChannel]);

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

            <div className="space-y-1">
              <span className="text-xs text-muted-foreground">Band</span>
              <Select
                value={model.wifi_band}
                onValueChange={(value) =>
                  setModel((m) => (m ? { ...m, wifi_band: value as "all" | "2.4" | "5" | "6" } : m))
                }
              >
                <SelectTrigger className="h-8 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">All Bands</SelectItem>
                  <SelectItem value="2.4">2.4 GHz</SelectItem>
                  <SelectItem value="5">5 GHz</SelectItem>
                  <SelectItem value="6">6 GHz</SelectItem>
                </SelectContent>
              </Select>
            </div>

            {(model.mode === "locked" || hopPreset === "selected") && (
              <div className="space-y-2">
                <span className="text-xs text-muted-foreground">Bandwidths (default: all)</span>
                <div className="max-h-24 overflow-auto rounded border border-border p-2 grid grid-cols-3 gap-1">
                  {caps.ht_modes.map((mode) => {
                    const checked = model.wifi_bandwidths.includes(mode);
                    return (
                      <label key={mode} className="flex items-center gap-1 text-[11px]">
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={(e) => {
                            const enabled = e.target.checked;
                            setModel((m) => {
                              if (!m) return m;
                              const next = enabled
                                ? [...m.wifi_bandwidths, mode]
                                : m.wifi_bandwidths.filter((value) => value !== mode);
                              return { ...m, wifi_bandwidths: next };
                            });
                          }}
                        />
                        {mode}
                      </label>
                    );
                  })}
                </div>
              </div>
            )}

            {model.mode === "locked" ? (
              <div className="grid grid-cols-2 gap-2">
                <div className="space-y-1">
                  <span className="text-xs text-muted-foreground">Channel</span>
                  <Select
                    value={String(model.locked_channel ?? bandChannels[0] ?? sortedChannels[0] ?? 1)}
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
                      {bandChannels.map((ch) => (
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
                    value={
                      model.locked_ht_mode && lockAllowedModes.includes(model.locked_ht_mode)
                        ? model.locked_ht_mode
                        : lockAllowedModes[0] ?? "HT20"
                    }
                    onValueChange={(value) =>
                      setModel((m) => (m ? { ...m, locked_ht_mode: value } : m))
                    }
                  >
                    <SelectTrigger className="h-8 text-xs">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {lockAllowedModes.map((mode) => (
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
                <div className="space-y-1">
                  <span className="text-xs text-muted-foreground">Hop Preset</span>
                  <Select
                    value={hopPreset}
                    onValueChange={(value) => {
                      const preset = value as "all" | "all_24" | "all_5" | "all_6" | "selected";
                      setHopPreset(preset);
                      if (preset === "selected") return;
                      const next =
                        preset === "all"
                          ? sortedChannels
                          : preset === "all_24"
                            ? sortedChannels.filter((ch) => ch >= 1 && ch <= 14)
                            : preset === "all_5"
                              ? sortedChannels.filter((ch) => ch >= 32 && ch <= 177)
                              : sortedChannels.filter((ch) => ch > 177);
                      setModel((m) =>
                        m
                          ? {
                              ...m,
                              hop_channels: next,
                              wifi_band: "all",
                              channel_ht_modes: preset === "selected" ? m.channel_ht_modes : {},
                            }
                          : m,
                      );
                    }}
                  >
                    <SelectTrigger className="h-8 text-xs">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="all">Hop All</SelectItem>
                      <SelectItem value="all_24">Hop 2.4 GHz</SelectItem>
                      <SelectItem value="all_5">Hop 5 GHz</SelectItem>
                      <SelectItem value="all_6">Hop 6 GHz</SelectItem>
                      <SelectItem value="selected">Hop Selected</SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                {hopPreset === "selected" && (
                  <>
                    <span className="text-xs text-muted-foreground">Hop Channels</span>
                    <div className="max-h-32 overflow-auto rounded border border-border p-2 grid grid-cols-5 gap-1">
                      {bandChannels.map((ch) => {
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
                                  const channelModes = { ...(m.channel_ht_modes ?? {}) };
                                  if (enabled) {
                                    channelModes[String(ch)] = modesForChannel(caps.ht_modes, ch);
                                  } else {
                                    delete channelModes[String(ch)];
                                  }
                                  return {
                                    ...m,
                                    hop_channels: next.sort((a, b) => a - b),
                                    channel_ht_modes: channelModes,
                                  };
                            });
                          }}
                        />
                            {ch}
                          </label>
                        );
                      })}
                    </div>
                  </>
                )}

                {hopPreset === "selected" && (
                  <div className="space-y-1">
                    <span className="text-xs text-muted-foreground">Per-Channel Bandwidths</span>
                    <div className="max-h-44 overflow-auto rounded border border-border p-2 space-y-2">
                      {effectiveHopChannels.map((ch) => {
                        const allowedModes = modesForChannel(caps.ht_modes, ch);
                        const selectedForChannel = new Set(
                          (model.channel_ht_modes?.[String(ch)] ?? allowedModes).filter((mode) =>
                            modeAllowedForChannel(mode, ch),
                          ),
                        );
                        return (
                          <div key={ch} className="border-b border-border/40 pb-1 last:border-b-0">
                            <div className="text-[11px] font-medium mb-1">Channel {ch}</div>
                            <div className="grid grid-cols-3 gap-1">
                              {allowedModes.map((mode) => {
                                const checked = selectedForChannel.has(mode);
                                return (
                                  <label key={`${ch}-${mode}`} className="flex items-center gap-1 text-[11px]">
                                    <input
                                      type="checkbox"
                                      checked={checked}
                                      onChange={(e) => {
                                        const enabled = e.target.checked;
                                        setModel((m) => {
                                          if (!m) return m;
                                          const current = new Set(
                                            (m.channel_ht_modes?.[String(ch)] ?? allowedModes).filter((entry) =>
                                              modeAllowedForChannel(entry, ch),
                                            ),
                                          );
                                          if (enabled) current.add(mode);
                                          else current.delete(mode);
                                          const channelModes = { ...(m.channel_ht_modes ?? {}) };
                                          channelModes[String(ch)] = Array.from(current).sort();
                                          return { ...m, channel_ht_modes: channelModes };
                                        });
                                      }}
                                    />
                                    {mode}
                                  </label>
                                );
                              })}
                            </div>
                          </div>
                        );
                      })}
                      {effectiveHopChannels.length === 0 && (
                        <div className="text-[11px] text-muted-foreground">No hop channels selected.</div>
                      )}
                    </div>
                  </div>
                )}

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
              <span className="text-xs text-muted-foreground">Bluetooth Interface</span>
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
                const normalizedChannelModes =
                  model.mode === "hop_specific"
                    ? Object.fromEntries(
                        effectiveHopChannels.map((ch) => [
                          String(ch),
                          (
                            model.channel_ht_modes?.[String(ch)]?.length
                              ? model.channel_ht_modes?.[String(ch)]
                              : hopPreset === "selected"
                                ? model.wifi_bandwidths
                                : caps.ht_modes
                          ).filter((mode) => Boolean(mode) && modeAllowedForChannel(mode, ch)),
                        ]),
                      )
                    : {};
                await onApply({
                  ...model,
                  wifi_bandwidths:
                    model.mode === "hop_specific" && hopPreset !== "selected"
                      ? []
                      : model.wifi_bandwidths,
                  hop_channels:
                    model.mode === "hop_specific"
                      ? [...effectiveHopChannels]
                      : model.hop_channels,
                  channel_ht_modes: normalizedChannelModes,
                });
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

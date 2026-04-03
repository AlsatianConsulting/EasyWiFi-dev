import { BluetoothDeviceRecord } from "@/data/mockData";
import { useState, useMemo } from "react";
import { ArrowUpDown, ArrowUp, ArrowDown, Columns, Bluetooth } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Checkbox } from "@/components/ui/checkbox";

interface BluetoothTabProps {
  devices: BluetoothDeviceRecord[];
  selectedDevice: BluetoothDeviceRecord | null;
  onSelectDevice: (device: BluetoothDeviceRecord) => void;
  visibleColumns: string[];
  onVisibleColumnsChange: (cols: string[]) => void;
}

type SortDir = "asc" | "desc" | null;

const allColumns = [
  { key: "name", label: "Name", align: "text-left" },
  { key: "mac", label: "MAC", align: "text-left" },
  { key: "oui", label: "OUI", align: "text-left" },
  { key: "rssi", label: "RSSI", align: "text-center" },
  { key: "mfgrIds", label: "Mfgr IDs", align: "text-left" },
  { key: "firstSeen", label: "First Seen", align: "text-center" },
  { key: "lastSeen", label: "Last Seen", align: "text-center" },
  { key: "mfgrNames", label: "Mfgr Names", align: "text-left" },
  { key: "uuids", label: "UUIDs", align: "text-left" },
];

const getSortValue = (d: BluetoothDeviceRecord, key: string): string | number => {
  switch (key) {
    case "name": return d.advertisedName ?? "";
    case "mac": return d.mac;
    case "oui": return d.ouiManufacturer ?? "";
    case "rssi": return d.rssiDbm ?? -999;
    case "mfgrIds": return d.mfgrIds.join(", ");
    case "firstSeen": return d.firstSeen;
    case "lastSeen": return d.lastSeen;
    case "mfgrNames": return d.mfgrNames.join(", ");
    case "uuids": return d.uuidNames.join(", ");
    default: return "";
  }
};

const BluetoothTab = ({ devices, selectedDevice, onSelectDevice, visibleColumns, onVisibleColumnsChange }: BluetoothTabProps) => {
  const [sortKey, setSortKey] = useState<string | null>(null);
  const [sortDir, setSortDir] = useState<SortDir>(null);
  const [filterText, setFilterText] = useState("");

  const handleSort = (key: string) => {
    if (sortKey === key) {
      if (sortDir === "asc") setSortDir("desc");
      else if (sortDir === "desc") { setSortKey(null); setSortDir(null); }
      else setSortDir("asc");
    } else {
      setSortKey(key);
      setSortDir("asc");
    }
  };

  const toggleColumn = (key: string) => {
    if (visibleColumns.includes(key)) {
      if (visibleColumns.length > 1) onVisibleColumnsChange(visibleColumns.filter(c => c !== key));
    } else {
      onVisibleColumnsChange([...visibleColumns, key]);
    }
  };

  const sorted = useMemo(() => {
    let data = [...devices];
    if (filterText) {
      const q = filterText.toLowerCase();
      data = data.filter(d =>
        (d.advertisedName ?? "").toLowerCase().includes(q) ||
        d.mac.toLowerCase().includes(q) ||
        (d.ouiManufacturer ?? "").toLowerCase().includes(q)
      );
    }
    if (sortKey && sortDir) {
      data.sort((a, b) => {
        const va = getSortValue(a, sortKey);
        const vb = getSortValue(b, sortKey);
        const cmp = typeof va === "number" && typeof vb === "number" ? va - vb : String(va).localeCompare(String(vb));
        return sortDir === "asc" ? cmp : -cmp;
      });
    }
    return data;
  }, [devices, sortKey, sortDir, filterText]);

  const cols = allColumns.filter(c => visibleColumns.includes(c.key));

  const SortIcon = ({ col }: { col: string }) => {
    if (sortKey !== col) return <ArrowUpDown className="h-3 w-3 opacity-30" />;
    return sortDir === "asc" ? <ArrowUp className="h-3 w-3" /> : <ArrowDown className="h-3 w-3" />;
  };

  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center justify-between px-3 py-2 border-b border-border">
        <h2 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
          Bluetooth Devices
        </h2>
        <div className="flex items-center gap-2">
          <input
            type="text"
            placeholder="Filter..."
            value={filterText}
            onChange={(e) => setFilterText(e.target.value)}
            className="h-6 w-32 rounded border border-border bg-secondary px-2 text-[10px] text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
          />
          <Popover>
            <PopoverTrigger asChild>
              <button className="flex items-center gap-1 rounded px-1.5 py-1 text-[10px] text-muted-foreground hover:bg-secondary hover:text-foreground transition-colors" title="Choose columns">
                <Columns className="h-3.5 w-3.5" />
              </button>
            </PopoverTrigger>
            <PopoverContent className="w-48 p-2 bg-card border-border" align="end">
              <p className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium mb-2">Visible Columns</p>
              {allColumns.map(col => (
                <label key={col.key} className="flex items-center gap-2 py-1 text-xs cursor-pointer hover:bg-secondary/50 px-1 rounded">
                  <Checkbox checked={visibleColumns.includes(col.key)} onCheckedChange={() => toggleColumn(col.key)} className="h-3.5 w-3.5" />
                  {col.label}
                </label>
              ))}
            </PopoverContent>
          </Popover>
          <span className="text-xs text-muted-foreground ">{sorted.length} discovered</span>
        </div>
      </div>
      <div className="overflow-auto flex-1">
        <table className="w-full text-xs">
          <thead className="sticky top-0 bg-card border-b border-border">
            <tr className="text-muted-foreground uppercase tracking-wider">
              {cols.map(col => (
                <th key={col.key} className={`${col.align} px-3 py-2 font-medium cursor-pointer select-none hover:text-foreground transition-colors`} onClick={() => handleSort(col.key)}>
                  <span className="inline-flex items-center gap-1">{col.label} <SortIcon col={col.key} /></span>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {sorted.map((device) => (
              <tr key={device.mac} onClick={() => onSelectDevice(device)}
                className={`cursor-pointer border-b border-border/50 transition-colors ${
                  selectedDevice?.mac === device.mac ? "bg-primary/10 border-l-2 border-l-primary" : "hover:bg-secondary/50"
                }`}>
                {visibleColumns.includes("name") && (
                  <td className="px-3 py-2 font-medium flex items-center gap-1.5">
                    <Bluetooth className="h-3 w-3 text-primary" />
                    {device.advertisedName ?? <span className="text-muted-foreground italic">Unknown</span>}
                  </td>
                )}
                {visibleColumns.includes("mac") && <td className="px-3 py-2  text-muted-foreground">{device.mac}</td>}
                {visibleColumns.includes("oui") && <td className="px-3 py-2 text-muted-foreground truncate max-w-[100px]">{device.ouiManufacturer ?? "—"}</td>}
                {visibleColumns.includes("rssi") && <td className="text-center px-3 py-2 ">{device.rssiDbm ?? "—"}</td>}
                {visibleColumns.includes("mfgrIds") && <td className="px-3 py-2  text-[10px] text-muted-foreground truncate max-w-[100px]">{device.mfgrIds.join(", ") || "—"}</td>}
                {visibleColumns.includes("firstSeen") && <td className="text-center px-3 py-2  text-muted-foreground">{device.firstSeen}</td>}
                {visibleColumns.includes("lastSeen") && <td className="text-center px-3 py-2  text-muted-foreground">{device.lastSeen}</td>}
                {visibleColumns.includes("mfgrNames") && <td className="px-3 py-2 text-[10px] text-muted-foreground truncate max-w-[120px]">{device.mfgrNames.join(", ") || "—"}</td>}
                {visibleColumns.includes("uuids") && <td className="px-3 py-2 text-[10px] text-muted-foreground truncate max-w-[140px]">{device.uuidNames.join(", ") || "—"}</td>}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
};

export default BluetoothTab;

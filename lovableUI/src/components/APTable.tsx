import { useState, useMemo } from "react";
import { AccessPointRecord } from "@/data/mockData";
import { ArrowUpDown, ArrowUp, ArrowDown, Columns } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Checkbox } from "@/components/ui/checkbox";

interface APTableProps {
  accessPoints: AccessPointRecord[];
  selectedAP: AccessPointRecord | null;
  onSelectAP: (ap: AccessPointRecord) => void;
  visibleColumns: string[];
  onVisibleColumnsChange: (cols: string[]) => void;
}

type SortDir = "asc" | "desc" | null;

const allColumns = [
  { key: "ssid", label: "SSID", align: "text-left" },
  { key: "bssid", label: "BSSID", align: "text-left" },
  { key: "oui", label: "OUI", align: "text-left" },
  { key: "channel", label: "CH", align: "text-center" },
  { key: "encryption", label: "Encryption", align: "text-center" },
  { key: "rssi", label: "RSSI", align: "text-center" },
  { key: "wps", label: "WPS", align: "text-center" },
  { key: "clients", label: "Clients", align: "text-center" },
  { key: "firstSeen", label: "First Seen", align: "text-center" },
  { key: "lastSeen", label: "Last Seen", align: "text-center" },
  { key: "handshakes", label: "Handshakes", align: "text-center" },
];

const getSortValue = (ap: AccessPointRecord, key: string): string | number => {
  switch (key) {
    case "ssid": return ap.ssid ?? "";
    case "bssid": return ap.bssid;
    case "oui": return ap.ouiManufacturer ?? "";
    case "channel": return ap.channel ?? 0;
    case "encryption": return ap.encryptionShort;
    case "rssi": return ap.rssiDbm ?? -999;
    case "wps": return ap.wps ? 1 : 0;
    case "clients": return ap.numberOfClients;
    case "firstSeen": return ap.firstSeen;
    case "lastSeen": return ap.lastSeen;
    case "handshakes": return ap.handshakeCount;
    default: return "";
  }
};

const APTable = ({ accessPoints, selectedAP, onSelectAP, visibleColumns, onVisibleColumnsChange }: APTableProps) => {
  const [sortKey, setSortKey] = useState<string | null>(null);
  const [sortDir, setSortDir] = useState<SortDir>(null);
  const [columnFilters, setColumnFilters] = useState<Record<string, string>>({});

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

  const moveColumn = (key: string, direction: -1 | 1) => {
    const idx = visibleColumns.indexOf(key);
    if (idx < 0) return;
    const next = idx + direction;
    if (next < 0 || next >= visibleColumns.length) return;
    const updated = [...visibleColumns];
    const tmp = updated[idx];
    updated[idx] = updated[next];
    updated[next] = tmp;
    onVisibleColumnsChange(updated);
  };

  const columnByKey = new Map(allColumns.map((c) => [c.key, c]));
  const cols = visibleColumns
    .map((key) => columnByKey.get(key))
    .filter((c): c is (typeof allColumns)[number] => Boolean(c));
  const chooserOrder = [
    ...visibleColumns,
    ...allColumns.map((c) => c.key).filter((key) => !visibleColumns.includes(key)),
  ];

  const sorted = useMemo(() => {
    let data = [...accessPoints];
    const activeFilters = Object.entries(columnFilters)
      .map(([k, v]) => [k, v.trim().toLowerCase()] as const)
      .filter(([, v]) => v.length > 0);
    if (activeFilters.length > 0) {
      data = data.filter((ap) =>
        activeFilters.every(([key, query]) =>
          String(getSortValue(ap, key)).toLowerCase().includes(query),
        ),
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
  }, [accessPoints, sortKey, sortDir, columnFilters]);

  const SortIcon = ({ col }: { col: string }) => {
    if (sortKey !== col) return <ArrowUpDown className="h-3 w-3 opacity-30" />;
    return sortDir === "asc" ? <ArrowUp className="h-3 w-3" /> : <ArrowDown className="h-3 w-3" />;
  };

  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center justify-between px-3 py-2 border-b border-border">
        <h2 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
          Discovered Access Points
        </h2>
        <div className="flex items-center gap-2">
          <Popover>
            <PopoverTrigger asChild>
              <button className="flex items-center gap-1 rounded px-1.5 py-1 text-[10px] text-muted-foreground hover:bg-secondary hover:text-foreground transition-colors" title="Choose columns">
                <Columns className="h-3.5 w-3.5" />
              </button>
            </PopoverTrigger>
            <PopoverContent className="w-48 p-2 bg-card border-border" align="end">
              <p className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium mb-2">Visible Columns</p>
              {chooserOrder.map((key) => {
                const col = columnByKey.get(key);
                if (!col) return null;
                const active = visibleColumns.includes(col.key);
                const idx = visibleColumns.indexOf(col.key);
                return (
                  <div key={col.key} className="flex items-center justify-between gap-1 py-1 px-1 rounded hover:bg-secondary/50">
                    <label className="flex items-center gap-2 text-xs cursor-pointer">
                      <Checkbox checked={active} onCheckedChange={() => toggleColumn(col.key)} className="h-3.5 w-3.5" />
                      {col.label}
                    </label>
                    {active && (
                      <div className="flex items-center gap-1">
                        <button
                          type="button"
                          className="rounded border border-border p-0.5 disabled:opacity-40"
                          disabled={idx <= 0}
                          onClick={() => moveColumn(col.key, -1)}
                          title="Move left"
                        >
                          <ArrowUp className="h-3 w-3" />
                        </button>
                        <button
                          type="button"
                          className="rounded border border-border p-0.5 disabled:opacity-40"
                          disabled={idx >= visibleColumns.length - 1}
                          onClick={() => moveColumn(col.key, 1)}
                          title="Move right"
                        >
                          <ArrowDown className="h-3 w-3" />
                        </button>
                      </div>
                    )}
                  </div>
                );
              })}
            </PopoverContent>
          </Popover>
          <span className="text-xs text-muted-foreground ">{sorted.length} found</span>
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
            <tr className="border-t border-border/60">
              {cols.map((col) => (
                <th key={`filter-${col.key}`} className="px-2 py-1">
                  <input
                    type="text"
                    placeholder={`Filter ${col.label}`}
                    value={columnFilters[col.key] ?? ""}
                    onChange={(e) =>
                      setColumnFilters((prev) => ({ ...prev, [col.key]: e.target.value }))
                    }
                    className="h-6 w-full rounded border border-border bg-secondary px-2 text-[10px] text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
                  />
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {sorted.map((ap) => (
              <tr
                key={ap.bssid}
                onClick={() => onSelectAP(ap)}
                className={`cursor-pointer border-b border-border/50 transition-colors ${
                  selectedAP?.bssid === ap.bssid
                    ? "bg-primary/10 border-l-2 border-l-primary"
                    : "hover:bg-secondary/50"
                }`}
              >
                {visibleColumns.includes("ssid") && <td className="px-3 py-2 font-medium text-foreground">{ap.ssid ?? <span className="text-muted-foreground italic">Hidden</span>}</td>}
                {visibleColumns.includes("bssid") && <td className="px-3 py-2  text-muted-foreground">{ap.bssid}</td>}
                {visibleColumns.includes("oui") && <td className="px-3 py-2 text-muted-foreground truncate max-w-[120px]">{ap.ouiManufacturer ?? "—"}</td>}
                {visibleColumns.includes("channel") && <td className="text-center px-3 py-2 ">{ap.channel ?? "—"}</td>}
                {visibleColumns.includes("encryption") && <td className="text-center px-3 py-2 ">{ap.encryptionShort}</td>}
                {visibleColumns.includes("rssi") && <td className="text-center px-3 py-2 ">{ap.rssiDbm ?? "—"}</td>}
                {visibleColumns.includes("wps") && <td className="text-center px-3 py-2">{ap.wps ? "Yes" : "—"}</td>}
                {visibleColumns.includes("clients") && <td className="text-center px-3 py-2 ">{ap.numberOfClients}</td>}
                {visibleColumns.includes("firstSeen") && <td className="text-center px-3 py-2  text-muted-foreground">{ap.firstSeen}</td>}
                {visibleColumns.includes("lastSeen") && <td className="text-center px-3 py-2  text-muted-foreground">{ap.lastSeen}</td>}
                {visibleColumns.includes("handshakes") && <td className="text-center px-3 py-2 ">{ap.handshakeCount}</td>}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
};

export default APTable;

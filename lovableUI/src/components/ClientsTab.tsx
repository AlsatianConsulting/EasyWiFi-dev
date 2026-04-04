import { ClientRecord } from "@/data/mockData";
import { useState, useMemo } from "react";
import { ArrowUpDown, ArrowUp, ArrowDown, Columns } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Checkbox } from "@/components/ui/checkbox";

interface ClientsTabProps {
  clients: ClientRecord[];
  selectedClient: ClientRecord | null;
  onSelectClient: (client: ClientRecord) => void;
  visibleColumns: string[];
  onVisibleColumnsChange: (cols: string[]) => void;
  apFilter?: string | null;
  onClearApFilter?: () => void;
}

type SortDir = "asc" | "desc" | null;

const formatBytes = (bytes: number) => {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
};

const getStatus = (c: ClientRecord) => {
  if (c.associatedAp) return "associated";
  if (c.probes.length > 0) return "probing";
  return "unassociated";
};

const allColumns = [
  { key: "mac", label: "MAC", align: "text-left" },
  { key: "oui", label: "OUI", align: "text-left" },
  { key: "associatedAp", label: "Associated AP", align: "text-left" },
  { key: "rssi", label: "RSSI", align: "text-center" },
  { key: "wps", label: "WPS", align: "text-center" },
  { key: "probes", label: "Probes", align: "text-left" },
  { key: "firstSeen", label: "First Seen", align: "text-center" },
  { key: "lastSeen", label: "Last Seen", align: "text-center" },
  { key: "data", label: "Data", align: "text-center" },
];

const getSortValue = (c: ClientRecord, key: string): string | number => {
  switch (key) {
    case "mac": return c.mac;
    case "oui": return c.ouiManufacturer ?? "";
    case "associatedAp": return c.associatedAp ?? "";
    case "rssi": return c.rssiDbm ?? -999;
    case "wps": return c.wps ? 1 : 0;
    case "probes": return c.probes.join(", ");
    case "firstSeen": return c.firstSeen;
    case "lastSeen": return c.lastSeen;
    case "data": return c.dataTransferredBytes;
    default: return "";
  }
};

const ClientsTab = ({ clients, selectedClient, onSelectClient, visibleColumns, onVisibleColumnsChange, apFilter, onClearApFilter }: ClientsTabProps) => {
  const [filter, setFilter] = useState<"all" | "associated" | "unassociated" | "probing">("all");
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

  const counts = {
    all: clients.length,
    associated: clients.filter(c => getStatus(c) === "associated").length,
    unassociated: clients.filter(c => getStatus(c) === "unassociated").length,
    probing: clients.filter(c => getStatus(c) === "probing").length,
  };

  const sorted = useMemo(() => {
    let data = [...clients];
    if (apFilter) {
      data = data.filter(c => c.associatedAp === apFilter);
    }
    if (filter !== "all") {
      data = data.filter(c => getStatus(c) === filter);
    }
    const activeFilters = Object.entries(columnFilters)
      .map(([k, v]) => [k, v.trim().toLowerCase()] as const)
      .filter(([, v]) => v.length > 0);
    if (activeFilters.length > 0) {
      data = data.filter((c) =>
        activeFilters.every(([key, query]) =>
          String(getSortValue(c, key)).toLowerCase().includes(query),
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
  }, [clients, filter, sortKey, sortDir, columnFilters, apFilter]);

  const columnByKey = new Map(allColumns.map((c) => [c.key, c]));
  const colsRaw = visibleColumns
    .map((key) => columnByKey.get(key))
    .filter((c): c is (typeof allColumns)[number] => Boolean(c));
  const cols = colsRaw.length > 0 ? colsRaw : allColumns;
  const activeKeys = new Set(cols.map((c) => c.key));
  const chooserOrder = [
    ...visibleColumns,
    ...allColumns.map((c) => c.key).filter((key) => !visibleColumns.includes(key)),
  ];

  const SortIcon = ({ col }: { col: string }) => {
    if (sortKey !== col) return <ArrowUpDown className="h-3 w-3 opacity-30" />;
    return sortDir === "asc" ? <ArrowUp className="h-3 w-3" /> : <ArrowDown className="h-3 w-3" />;
  };

  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center justify-between px-3 py-2 border-b border-border">
        <div className="flex items-center gap-2">
          <h2 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
            All Clients & Probes
          </h2>
          {apFilter && (
            <span className="flex items-center gap-1 rounded bg-secondary px-2 py-0.5 text-[10px]  text-foreground">
              AP: {apFilter}
              <button onClick={onClearApFilter} className="ml-1 text-muted-foreground hover:text-foreground">×</button>
            </span>
          )}
        </div>
        <div className="flex items-center gap-2">
          <div className="flex items-center gap-1">
            {(["all", "associated", "probing", "unassociated"] as const).map((f) => (
              <button key={f} onClick={() => setFilter(f)}
                className={`rounded px-2 py-0.5 text-[10px] font-medium transition-colors ${filter === f ? "bg-primary text-primary-foreground" : "text-muted-foreground hover:bg-secondary"}`}>
                {f.charAt(0).toUpperCase() + f.slice(1)} ({counts[f]})
              </button>
            ))}
          </div>
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
            {sorted.map((client) => (
              <tr key={client.mac} onClick={() => onSelectClient(client)}
                className={`cursor-pointer border-b border-border/50 transition-colors ${selectedClient?.mac === client.mac ? "bg-primary/10 border-l-2 border-l-primary" : "hover:bg-secondary/50"}`}>
                {activeKeys.has("mac") && <td className="px-3 py-2  text-muted-foreground">{client.mac}</td>}
                {activeKeys.has("oui") && <td className="px-3 py-2 text-muted-foreground truncate max-w-[100px]">{client.ouiManufacturer ?? "—"}</td>}
                {activeKeys.has("associatedAp") && <td className="px-3 py-2  text-[10px]">{client.associatedAp ?? <span className="text-muted-foreground">—</span>}</td>}
                {activeKeys.has("rssi") && <td className="text-center px-3 py-2 ">{client.rssiDbm ?? "—"}</td>}
                {activeKeys.has("wps") && <td className="text-center px-3 py-2">{client.wps ? "Yes" : "—"}</td>}
                {activeKeys.has("probes") && <td className="px-3 py-2 text-[10px] text-muted-foreground truncate max-w-[150px]">{client.probes.filter(Boolean).join(", ") || "—"}</td>}
                {activeKeys.has("firstSeen") && <td className="text-center px-3 py-2  text-muted-foreground">{client.firstSeen}</td>}
                {activeKeys.has("lastSeen") && <td className="text-center px-3 py-2  text-muted-foreground">{client.lastSeen}</td>}
                {activeKeys.has("data") && <td className="text-center px-3 py-2 ">{formatBytes(client.dataTransferredBytes)}</td>}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
};

export default ClientsTab;

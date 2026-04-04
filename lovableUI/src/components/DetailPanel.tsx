import { AccessPointRecord } from "@/data/mockData";
import { ClientRecord } from "@/data/mockData";
import { PieChart, Pie, Cell, ResponsiveContainer, Tooltip } from "recharts";
import RSSIMeter from "./RSSIMeter";

interface DetailPanelProps {
  ap: AccessPointRecord | null;
  clients?: ClientRecord[];
  onNavigateToClients?: (apBssid: string) => void;
  onLockToAp?: (apBssid: string) => void;
  actionStatus?: { is_error: boolean; message: string } | null;
}

const PacketPieChart = ({ pm }: { pm: AccessPointRecord["packetMix"] }) => {
  const data = [
    { name: "Management", value: pm.management, color: "hsl(27, 76%, 53%)" },
    { name: "Control", value: pm.control, color: "hsl(38, 92%, 50%)" },
    { name: "Data", value: pm.data, color: "hsl(142, 71%, 45%)" },
    { name: "Other", value: pm.other, color: "hsl(215, 15%, 55%)" },
  ].filter(d => d.value > 0);

  return (
    <div className="rounded-lg border border-border bg-secondary/30 p-3">
      <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">Packet Mix</span>
      <div className="h-32 mt-1">
        <ResponsiveContainer width="100%" height="100%">
          <PieChart>
            <Pie data={data} cx="50%" cy="50%" innerRadius={30} outerRadius={50} paddingAngle={3} dataKey="value" stroke="none">
              {data.map((entry, index) => <Cell key={index} fill={entry.color} />)}
            </Pie>
            <Tooltip contentStyle={{ backgroundColor: "hsl(240, 8%, 10%)", border: "1px solid hsl(240, 6%, 18%)", borderRadius: "6px", fontSize: "11px" }} itemStyle={{ color: "hsl(210, 20%, 92%)" }} />
          </PieChart>
        </ResponsiveContainer>
      </div>
      <div className="flex flex-wrap justify-center gap-3 mt-1">
        {data.map((d) => (
          <div key={d.name} className="flex items-center gap-1">
            <div className="h-2 w-2 rounded-full" style={{ backgroundColor: d.color }} />
            <span className="text-[10px] text-muted-foreground">{d.name}</span>
          </div>
        ))}
      </div>
    </div>
  );
};

const formatUptime = (seconds: number | null): string => {
  if (seconds === null || seconds <= 0) return "—";
  const total = Math.floor(seconds);
  const d = Math.floor(total / 86400);
  const h = Math.floor((total % 86400) / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (d > 0) return `${d}d ${h}h ${m}m ${s}s`;
  if (h > 0) return `${h}h ${m}m ${s}s`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
};

const DetailPanel = ({ ap, clients = [], onNavigateToClients, onLockToAp, actionStatus }: DetailPanelProps) => {
  if (!ap) {
    return (
      <div className="flex h-full items-center justify-center text-muted-foreground text-sm">
        <p>Select an AP to view details</p>
      </div>
    );
  }

  const totalPackets = ap.packetMix.management + ap.packetMix.control + ap.packetMix.data + ap.packetMix.other;
  const observedClientBands = Array.from(
    new Set(
      clients
        .filter((c) => c.associatedAp?.toUpperCase() === ap.bssid.toUpperCase())
        .map((c) => c.networkIntel.band)
        .filter((band) => band && band !== "Unknown"),
    ),
  );
  const observedClientBandLabel = observedClientBands.length > 0 ? observedClientBands.join(", ") : "—";

  return (
    <div className="flex flex-col gap-3 p-3 overflow-auto h-full">
      <div className="rounded-lg border border-border bg-secondary/30 p-3">
        <h3 className="text-sm font-bold text-foreground">{ap.ssid ?? <span className="italic text-muted-foreground">Hidden Network</span>}</h3>
        <p className=" text-[10px] text-muted-foreground mt-0.5">{ap.bssid}</p>
        {ap.ouiManufacturer && <p className="text-[10px] text-muted-foreground mt-0.5">{ap.ouiManufacturer}</p>}
      </div>

      <div className="rounded-lg border border-border bg-secondary/30 p-3">
        <RSSIMeter rssi={ap.rssiDbm ?? -100} />
        {onLockToAp && (
          <button
            className="mt-2 rounded-md border border-border bg-primary px-2 py-1 text-[10px] font-medium text-primary-foreground"
            onClick={() => onLockToAp(ap.bssid)}
          >
            Lock To AP
          </button>
        )}
        {actionStatus?.message && (
          <p className={`mt-2 text-[10px] ${actionStatus.is_error ? "text-destructive" : "text-emerald-400"}`}>
            {actionStatus.message}
          </p>
        )}
      </div>

      <div className="grid grid-cols-2 gap-2">
        {[
          { label: "Channel", value: ap.channel ?? "—" },
          { label: "Frequency", value: ap.frequencyMhz ? `${ap.frequencyMhz} MHz` : "—" },
          { label: "Band", value: ap.band },
          { label: "Observed Client Bandwidth", value: observedClientBandLabel },
          { label: "Encryption", value: ap.encryptionShort },
          { label: "Full Encryption", value: ap.encryptionFull },
          { label: "First Seen", value: ap.firstSeen },
          { label: "Last Seen", value: ap.lastSeen },
          { label: "Handshakes", value: ap.handshakeCount },
          { label: "Uptime", value: formatUptime(ap.uptimeBeacons) },
          { label: "Uptime (s)", value: ap.uptimeBeacons?.toLocaleString() ?? "—" },
          { label: "Country", value: ap.countryCode80211d ?? "—" },
          { label: "Total Packets", value: totalPackets.toLocaleString() },
          { label: "Source Adapters", value: ap.sourceAdapters.join(", ") || "—" },
        ].map((item) => (
          <div key={item.label} className="rounded-md border border-border bg-secondary/30 p-2">
            <span className="text-[9px] uppercase tracking-wider text-muted-foreground block">{item.label}</span>
            <span className="text-xs font-medium ">{item.value}</span>
          </div>
        ))}
      </div>

      {/* Clients - clickable to navigate */}
      <div
        className={`rounded-md border border-border bg-secondary/30 p-2 ${onNavigateToClients ? "cursor-pointer hover:bg-secondary/60 transition-colors" : ""}`}
        onClick={() => onNavigateToClients?.(ap.bssid)}
      >
        <span className="text-[9px] uppercase tracking-wider text-muted-foreground block">Clients</span>
        <span className="text-xs font-medium ">{ap.numberOfClients}</span>
        {onNavigateToClients && <span className="text-[9px] text-muted-foreground ml-2">(click to view)</span>}
      </div>

      {ap.wps && (
        <div className="rounded-lg border border-border bg-secondary/30 p-3">
          <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">WPS Information</span>
          <div className="mt-2 grid grid-cols-2 gap-1.5">
            {[
              { label: "Version", value: ap.wps.version },
              { label: "State", value: ap.wps.state },
              { label: "Config Methods", value: ap.wps.configMethods },
              { label: "Manufacturer", value: ap.wps.manufacturer },
              { label: "Model", value: ap.wps.modelName },
              { label: "Model Number", value: ap.wps.modelNumber },
              { label: "Serial Number", value: ap.wps.serialNumber },
            ].filter(i => i.value).map((item) => (
              <div key={item.label}>
                <span className="text-[9px] text-muted-foreground">{item.label}</span>
                <span className="text-[11px]  block">{item.value}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      <PacketPieChart pm={ap.packetMix} />

      {ap.notes && (
        <div className="rounded-lg border border-border bg-secondary/30 p-3">
          <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">Notes</span>
          <p className="text-xs mt-1">{ap.notes}</p>
        </div>
      )}
    </div>
  );
};

export default DetailPanel;

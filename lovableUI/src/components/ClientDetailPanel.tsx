import { ClientRecord } from "@/data/mockData";
import RSSIMeter from "./RSSIMeter";
import { PieChart, Pie, Cell, ResponsiveContainer, Tooltip } from "recharts";

interface ClientDetailPanelProps {
  client: ClientRecord | null;
}

const formatBytes = (bytes: number) => {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
};

const ClientDetailPanel = ({ client }: ClientDetailPanelProps) => {
  if (!client) {
    return (
      <div className="flex h-full items-center justify-center text-muted-foreground text-sm">
        <p>Select a client to view details</p>
      </div>
    );
  }

  const ni = client.networkIntel;
  const pm = ni.packetMix;
  const pmData = [
    { name: "Management", value: pm.management, color: "hsl(27, 76%, 53%)" },
    { name: "Control", value: pm.control, color: "hsl(38, 92%, 50%)" },
    { name: "Data", value: pm.data, color: "hsl(142, 71%, 45%)" },
    { name: "Other", value: pm.other, color: "hsl(215, 15%, 55%)" },
  ].filter(d => d.value > 0);

  return (
    <div className="flex flex-col gap-3 p-3 overflow-auto h-full">
      <div className="rounded-lg border border-border bg-secondary/30 p-3">
        <h3 className="text-sm font-bold text-foreground">{client.ouiManufacturer ?? "Unknown"}</h3>
        <p className=" text-[10px] text-muted-foreground mt-0.5">{client.mac}</p>
      </div>

      <RSSIMeter rssi={client.rssiDbm ?? -100} />

      <div className="grid grid-cols-2 gap-2">
        {[
          { label: "Data Transferred", value: formatBytes(client.dataTransferredBytes) },
          { label: "First Seen", value: client.firstSeen },
          { label: "Last Seen", value: client.lastSeen },
          { label: "Source Adapters", value: client.sourceAdapters.join(", ") || "—" },
          { label: "Seen APs", value: client.seenAccessPoints.length },
          { label: "Handshake Networks", value: client.handshakeNetworks.length },
          { label: "Band", value: ni.band },
          { label: "Uplink", value: formatBytes(ni.uplinkBytes) },
          { label: "Downlink", value: formatBytes(ni.downlinkBytes) },
          { label: "Retry Frames", value: ni.retryFrameCount },
          { label: "Power Save", value: ni.powerSaveObserved ? "Yes" : "No" },
          { label: "EAPOL Frames", value: ni.eapolFrameCount },
          { label: "PMKID Count", value: ni.pmkidCount },
        ].map((item) => (
          <div key={item.label} className="rounded-md border border-border bg-secondary/30 p-2">
            <span className="text-[9px] uppercase tracking-wider text-muted-foreground block">{item.label}</span>
            <span className="text-xs font-medium ">{item.value}</span>
          </div>
        ))}
      </div>

      {client.associatedAp && (
        <div className="rounded-lg border border-border bg-secondary/30 p-3">
          <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">Associated AP</span>
          <p className="text-xs  mt-1">{client.associatedAp}</p>
        </div>
      )}

      {client.probes.length > 0 && (
        <div className="rounded-lg border border-border bg-secondary/30 p-3">
          <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">Probed SSIDs ({client.probes.length})</span>
          <div className="flex flex-wrap gap-1 mt-2">
            {client.probes.map((ssid, i) => (
              <span key={i} className="rounded bg-secondary px-2 py-0.5 text-[10px] ">{ssid || "Broadcast"}</span>
            ))}
          </div>
        </div>
      )}

      {client.seenAccessPoints.length > 0 && (
        <div className="rounded-lg border border-border bg-secondary/30 p-3">
          <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">Seen Access Points</span>
          <div className="flex flex-wrap gap-1 mt-2">
            {client.seenAccessPoints.map((ap, i) => (
              <span key={i} className="rounded bg-secondary px-2 py-0.5 text-[10px] ">{ap}</span>
            ))}
          </div>
        </div>
      )}

      {client.handshakeNetworks.length > 0 && (
        <div className="rounded-lg border border-border bg-secondary/30 p-3">
          <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">Handshake Networks</span>
          <div className="flex flex-wrap gap-1 mt-2">
            {client.handshakeNetworks.map((net, i) => (
              <span key={i} className="rounded bg-secondary px-2 py-0.5 text-[10px] ">{net}</span>
            ))}
          </div>
        </div>
      )}

      {(ni.localIpv4Addresses.length > 0 || ni.localIpv6Addresses.length > 0 || ni.dhcpHostnames.length > 0 || ni.dnsNames.length > 0) && (
        <div className="rounded-lg border border-border bg-secondary/30 p-3">
          <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">Network Intel</span>
          <div className="mt-2 space-y-1">
            {ni.localIpv4Addresses.map((ip, i) => (
              <div key={`v4-${i}`} className="flex items-center justify-between">
                <span className="text-[10px] text-muted-foreground">IPv4</span>
                <span className=" text-[10px]">{ip}</span>
              </div>
            ))}
            {ni.localIpv6Addresses.map((ip, i) => (
              <div key={`v6-${i}`} className="flex items-center justify-between">
                <span className="text-[10px] text-muted-foreground">IPv6</span>
                <span className=" text-[10px]">{ip}</span>
              </div>
            ))}
            {ni.dhcpHostnames.map((h, i) => (
              <div key={`dh-${i}`} className="flex items-center justify-between">
                <span className="text-[10px] text-muted-foreground">Hostname</span>
                <span className=" text-[10px]">{h}</span>
              </div>
            ))}
            {ni.dnsNames.map((d, i) => (
              <div key={`dns-${i}`} className="flex items-center justify-between">
                <span className="text-[10px] text-muted-foreground">DNS</span>
                <span className=" text-[10px]">{d}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {client.wps && (
        <div className="rounded-lg border border-border bg-secondary/30 p-3">
          <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">WPS Information</span>
          <div className="mt-2 grid grid-cols-2 gap-1.5">
            {[
              { label: "Version", value: client.wps.version },
              { label: "State", value: client.wps.state },
              { label: "Config Methods", value: client.wps.configMethods },
              { label: "Manufacturer", value: client.wps.manufacturer },
              { label: "Model", value: client.wps.modelName },
            ].filter(i => i.value).map((item) => (
              <div key={item.label}>
                <span className="text-[9px] text-muted-foreground">{item.label}</span>
                <span className="text-[11px]  block">{item.value}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {pmData.length > 0 && (
        <div className="rounded-lg border border-border bg-secondary/30 p-3">
          <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">Packet Mix</span>
          <div className="h-28 mt-1">
            <ResponsiveContainer width="100%" height="100%">
              <PieChart>
                <Pie data={pmData} cx="50%" cy="50%" innerRadius={25} outerRadius={42} paddingAngle={3} dataKey="value" stroke="none">
                  {pmData.map((entry, index) => <Cell key={index} fill={entry.color} />)}
                </Pie>
                <Tooltip contentStyle={{ backgroundColor: "hsl(240, 8%, 10%)", border: "1px solid hsl(240, 6%, 18%)", borderRadius: "6px", fontSize: "11px" }} itemStyle={{ color: "hsl(210, 20%, 92%)" }} />
              </PieChart>
            </ResponsiveContainer>
          </div>
          <div className="flex flex-wrap justify-center gap-2 mt-1">
            {pmData.map((d) => (
              <div key={d.name} className="flex items-center gap-1">
                <div className="h-2 w-2 rounded-full" style={{ backgroundColor: d.color }} />
                <span className="text-[9px] text-muted-foreground">{d.name}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
};

export default ClientDetailPanel;

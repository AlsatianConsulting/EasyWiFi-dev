import { BluetoothDeviceRecord } from "@/data/mockData";
import RSSIMeter from "./RSSIMeter";
import { Bluetooth } from "lucide-react";

interface BluetoothDetailPanelProps {
  device: BluetoothDeviceRecord | null;
  onEnumerateServices?: (mac: string) => void;
  enumerationStatus?: { is_error: boolean; message: string } | null;
}

const BluetoothDetailPanel = ({ device, onEnumerateServices, enumerationStatus }: BluetoothDetailPanelProps) => {
  if (!device) {
    return (
      <div className="flex h-full items-center justify-center text-muted-foreground text-sm">
        <p>Select a device to view details</p>
      </div>
    );
  }

  const ae = device.activeEnumeration;

  return (
    <div className="flex flex-col gap-3 p-3 overflow-auto h-full">
      <div className="rounded-lg border border-border bg-secondary/30 p-3">
        <div className="flex items-center gap-2">
          <Bluetooth className="h-4 w-4 text-primary" />
          <h3 className="text-sm font-bold text-foreground">{device.advertisedName ?? "Unknown Device"}</h3>
        </div>
        <p className=" text-[10px] text-muted-foreground mt-0.5">{device.mac}</p>
        {device.ouiManufacturer && <p className="text-[10px] text-muted-foreground mt-0.5">{device.ouiManufacturer}</p>}
        {onEnumerateServices && (
          <>
            <button
              className="mt-2 rounded-md border border-border bg-primary px-2 py-1 text-[10px] font-medium text-primary-foreground"
              onClick={() => onEnumerateServices(device.mac)}
            >
              Scan Services
            </button>
            <p className="mt-1 text-[10px] text-amber-400">
              Active scan warning: this can trigger pairing prompts or visible activity on the target device.
            </p>
          </>
        )}
        {enumerationStatus?.message && (
          <p className={`mt-2 text-[10px] ${enumerationStatus.is_error ? "text-destructive" : "text-emerald-400"}`}>
            {enumerationStatus.message}
          </p>
        )}
      </div>

      <div className="self-stretch 2xl:self-start">
        <RSSIMeter rssi={device.rssiDbm ?? -100} compactOnWide />
      </div>

      <div className="grid grid-cols-2 gap-2">
        {[
          { label: "Transport", value: device.transport },
          { label: "Address Type", value: device.addressType ?? "—" },
          { label: "Device Type", value: device.deviceType ?? "—" },
          { label: "Class of Device", value: device.classOfDevice ?? "—" },
          { label: "Alias", value: device.alias ?? "—" },
          { label: "First Seen", value: device.firstSeen },
          { label: "Last Seen", value: device.lastSeen },
          { label: "Source Adapters", value: device.sourceAdapters.join(", ") || "—" },
        ].map((item) => (
          <div key={item.label} className="rounded-md border border-border bg-secondary/30 p-2">
            <span className="text-[9px] uppercase tracking-wider text-muted-foreground block">{item.label}</span>
            <span className="text-xs font-medium ">{item.value}</span>
          </div>
        ))}
      </div>

      {device.mfgrIds.length > 0 && (
        <div className="rounded-lg border border-border bg-secondary/30 p-3">
          <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">Manufacturer IDs</span>
          <div className="flex flex-wrap gap-1 mt-2">
            {device.mfgrIds.map((id, i) => (
              <span key={i} className="rounded bg-secondary px-2 py-0.5 text-[10px] ">{id}</span>
            ))}
          </div>
          {device.mfgrNames.length > 0 && (
            <p className="text-[10px] text-muted-foreground mt-1">{device.mfgrNames.join(", ")}</p>
          )}
        </div>
      )}

      {device.uuidNames.length > 0 && (
        <div className="rounded-lg border border-border bg-secondary/30 p-3">
          <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">Services / UUIDs</span>
          <div className="flex flex-wrap gap-1 mt-2">
            {device.uuidNames.map((name, i) => (
              <span key={i} className="rounded bg-secondary px-2 py-0.5 text-[10px] ">{name}</span>
            ))}
          </div>
          {device.uuids.length > 0 && (
            <div className="mt-2 space-y-0.5">
              {device.uuids.map((uuid, i) => (
                <p key={i} className=" text-[9px] text-muted-foreground">{uuid}</p>
              ))}
            </div>
          )}
        </div>
      )}

      {ae && (
        <div className="rounded-lg border border-border bg-secondary/30 p-3">
          <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">Active Enumeration</span>
          <div className="grid grid-cols-2 gap-2 mt-2">
            {[
              { label: "Connected", value: ae.connected ? "Yes" : "No" },
              { label: "Paired", value: ae.paired ? "Yes" : "No" },
              { label: "Trusted", value: ae.trusted ? "Yes" : "No" },
              { label: "Blocked", value: ae.blocked ? "Yes" : "No" },
              { label: "Services Resolved", value: ae.servicesResolved ? "Yes" : "No" },
              { label: "TX Power", value: ae.txPowerDbm != null ? `${ae.txPowerDbm} dBm` : "—" },
              { label: "Battery", value: ae.batteryPercent != null ? `${ae.batteryPercent}%` : "—" },
              { label: "Appearance", value: ae.appearanceName ?? "—" },
              { label: "Icon", value: ae.icon ?? "—" },
              { label: "Modalias", value: ae.modalias ?? "—" },
            ].map((item) => (
              <div key={item.label}>
                <span className="text-[9px] text-muted-foreground">{item.label}</span>
                <span className="text-[11px]  block">{item.value}</span>
              </div>
            ))}
          </div>

          {ae.services.length > 0 && (
            <div className="mt-2">
              <span className="text-[9px] text-muted-foreground">GATT Services</span>
              <div className="flex flex-wrap gap-1 mt-1">
                {ae.services.map((svc, i) => (
                  <span key={i} className="rounded bg-secondary px-2 py-0.5 text-[10px] ">
                    {svc.name ?? svc.uuid} {svc.primary ? "(Primary)" : ""}
                  </span>
                ))}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
};

export default BluetoothDetailPanel;

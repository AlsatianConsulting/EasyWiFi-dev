import React, { useEffect, useRef } from "react";

interface RSSIMeterProps {
  rssi: number;
  compactOnWide?: boolean;
}

const RSSIMeter: React.FC<RSSIMeterProps> = ({ rssi, compactOnWide = true }) => {
  const needleRef = useRef<SVGLineElement>(null);

  // Map RSSI (-100 to -30) across the visible top semicircle (left=weak right=strong).
  const clampedRssi = Math.max(-100, Math.min(-30, rssi));
  const normalized = (clampedRssi + 100) / 70; // 0 (weak) to 1 (strong)
  const angle = -180 + normalized * 180; // -180° (left/red) to 0° (right/green)

  useEffect(() => {
    if (needleRef.current) {
      needleRef.current.style.transition = "transform 0.6s cubic-bezier(0.34, 1.56, 0.64, 1)";
      needleRef.current.style.transformOrigin = "150px 130px";
      needleRef.current.style.transform = `rotate(${angle}deg)`;
    }
  }, [angle]);

  const getLabel = () => {
    if (rssi >= -40) return "Excellent";
    if (rssi >= -55) return "Good";
    if (rssi >= -70) return "Fair";
    if (rssi >= -85) return "Weak";
    return "Very Weak";
  };

  return (
    <div className={`rounded-lg border border-border bg-card p-3 ${compactOnWide ? "2xl:max-w-[260px] 2xl:p-2" : ""}`}>
      <div className="mb-1 flex items-center justify-between">
        <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-medium">
          Signal Strength Meter
        </span>
        <span className={`${compactOnWide ? "2xl:text-xs" : ""} text-sm font-bold text-foreground`}>{rssi} dBm</span>
      </div>

      <svg viewBox="0 0 300 170" className={`${compactOnWide ? "2xl:h-[96px]" : ""} w-full`}>
        {/* Arc background segments: Red → Yellow → Green */}
        <defs>
          <linearGradient id="meterGradient" x1="0%" y1="0%" x2="100%" y2="0%">
            <stop offset="0%" stopColor="hsl(0, 72%, 51%)" />
            <stop offset="35%" stopColor="hsl(38, 92%, 50%)" />
            <stop offset="100%" stopColor="hsl(142, 71%, 45%)" />
          </linearGradient>
        </defs>

        {/* Outer arc track */}
        <path
          d="M 30 130 A 120 120 0 0 1 270 130"
          fill="none"
          stroke="hsl(240, 6%, 18%)"
          strokeWidth="16"
          strokeLinecap="round"
        />

        {/* Colored arc */}
        <path
          d="M 30 130 A 120 120 0 0 1 270 130"
          fill="none"
          stroke="url(#meterGradient)"
          strokeWidth="14"
          strokeLinecap="round"
          opacity="0.8"
        />

        {/* Tick marks */}
        {Array.from({ length: 8 }).map((_, i) => {
          const tickAngle = -180 + i * (180 / 7);
          const rad = (tickAngle * Math.PI) / 180;
          const innerR = 105;
          const outerR = 118;
          const x1 = 150 + innerR * Math.cos(rad);
          const y1 = 130 + innerR * Math.sin(rad);
          const x2 = 150 + outerR * Math.cos(rad);
          const y2 = 130 + outerR * Math.sin(rad);
          const labelR = 95;
          const lx = 150 + labelR * Math.cos(rad);
          const ly = 130 + labelR * Math.sin(rad);
          const dbm = -100 + i * 10;
          return (
            <g key={i}>
              <line x1={x1} y1={y1} x2={x2} y2={y2} stroke="hsl(210, 20%, 92%)" strokeWidth="1.5" opacity="0.4" />
              <text x={lx} y={ly} textAnchor="middle" dominantBaseline="middle" fill="hsl(215, 15%, 55%)" fontSize="8" fontFamily="monospace">
                {dbm}
              </text>
            </g>
          );
        })}

        {/* Needle pivot */}
        <circle cx="150" cy="130" r="6" fill="hsl(27, 76%, 53%)" />
        <circle cx="150" cy="130" r="3" fill="hsl(240, 10%, 7%)" />

        {/* Needle */}
        <line
          ref={needleRef}
          x1="150"
          y1="130"
          x2="55"
          y2="130"
          stroke="hsl(27, 76%, 53%)"
          strokeWidth="2.5"
          strokeLinecap="round"
          style={{ transformOrigin: "150px 130px" }}
        />

        {/* Label */}
        <text x="150" y="160" textAnchor="middle" fill="hsl(210, 20%, 92%)" fontSize={compactOnWide ? "10" : "11"} fontWeight="600">
          {getLabel()}
        </text>
      </svg>
    </div>
  );
};

export default RSSIMeter;

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/** Map color keys to concrete Tailwind color hex values — avoids dynamic class names
 *  that Tailwind v4 would purge at build time. */
export const COLOR_HEX: Record<string, { bg: string; text: string; border: string; bgFaint: string }> = {
  green:  { bg: "#22c55e", text: "#4ade80", border: "rgba(34,197,94,0.2)",  bgFaint: "rgba(34,197,94,0.15)" },
  red:    { bg: "#ef4444", text: "#f87171", border: "rgba(239,68,68,0.2)",  bgFaint: "rgba(239,68,68,0.15)" },
  yellow: { bg: "#eab308", text: "#facc15", border: "rgba(234,179,8,0.2)",  bgFaint: "rgba(234,179,8,0.15)" },
  gray:   { bg: "#6b7280", text: "#9ca3af", border: "rgba(107,114,128,0.2)", bgFaint: "rgba(107,114,128,0.15)" },
  blue:   { bg: "#3b82f6", text: "#60a5fa", border: "rgba(59,130,246,0.2)", bgFaint: "rgba(59,130,246,0.15)" },
};

export function ProgressBar({ pct, total, filled, color }: { pct: number; total: number; filled: number; color: string }) {
  const c = COLOR_HEX[color] || COLOR_HEX.blue;
  if (total <= 0) {
    return (
      <div className="w-full bg-gray-700 rounded-full h-1.5 overflow-hidden">
        <div className="h-1.5 rounded-full transition-all" style={{ width: `${pct}%`, backgroundColor: c.bg }} />
      </div>
    );
  }
  return (
    <div className="flex gap-0.5">
      {Array.from({ length: total }, (_, i) => (
        <div
          key={i}
          className="h-2 flex-1 rounded-full transition-all"
          style={{ backgroundColor: i < filled ? c.bg : "#4b5563" }}
        />
      ))}
    </div>
  );
}

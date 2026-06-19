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

/** The step currently in progress is light blue; completed steps take the
 *  status colour (blue while the flow runs, green once it completes); pending
 *  steps are grey. */
const ACTIVE_SEGMENT_BG = "#93c5fd"; // light blue (in progress)
const PENDING_SEGMENT_BG = "#4b5563"; // grey (pending)

export function ProgressBar({
  pct,
  total,
  filled,
  color,
  activeIndex,
}: {
  pct: number;
  total: number;
  filled: number;
  color: string;
  /** Index of the step currently in progress — rendered orange. `null` when no
   *  step is actively running (idle / all complete). */
  activeIndex?: number | null;
}) {
  const c = COLOR_HEX[color] || COLOR_HEX.blue;
  if (total <= 0) {
    return (
      <div className="w-full bg-gray-700 rounded-full h-1.5 overflow-hidden">
        <div className="h-1.5 rounded-full transition-all" style={{ width: `${pct}%`, backgroundColor: c.bg }} />
      </div>
    );
  }
  // Completed segments take the status colour: blue while the flow is running
  // (status "Running" → blue), green once the flow completes (status
  // "Completed" → green). The in-progress step is light blue; pending grey.
  return (
    <div className="flex gap-0.5">
      {Array.from({ length: total }, (_, i) => {
        const isActive = i === activeIndex && i >= filled;
        const bg = isActive ? ACTIVE_SEGMENT_BG : i < filled ? c.bg : PENDING_SEGMENT_BG;
        return (
          <div
            key={i}
            className={`h-2 flex-1 rounded-full transition-all ${isActive ? "animate-pulse" : ""}`}
            style={{ backgroundColor: bg }}
          />
        );
      })}
    </div>
  );
}

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

export interface StatusInfo {
  label: string;
  color: "green" | "red" | "yellow" | "gray" | "blue";
}

const COLOR_TEXT: Record<StatusInfo["color"], string> = {
  green:  "#4ade80",
  red:    "#f87171",
  yellow: "#facc15",
  gray:   "#9ca3af",
  blue:   "#60a5fa",
};

export function getStatusInfo(state: string, canStart?: boolean): StatusInfo {
  const s = state.toLowerCase();
  if (s === "done" || s.startsWith("completed")) return { label: "Completed", color: "green" };
  if (s.startsWith("error"))                     return { label: "Error",     color: "red"   };
  if (s === "paused")                            return { label: "Paused",    color: "yellow" };
  if (s === "stopped")                           return { label: "Stopped",   color: "gray"  };
  if (s === "pending" && canStart)               return { label: "Pending",   color: "gray"  };
  return                                                { label: "Running",   color: "blue"  };
}

export function StatusBadge({ status }: { status: StatusInfo }) {
  const color = COLOR_TEXT[status.color];
  return (
    <span className="inline-flex items-center gap-1 text-xs font-medium" style={{ color }}>
      {status.label === "Completed" && <CheckIcon />}
      {status.label === "Error"     && <XIcon />}
      {status.label === "Running"   && <span className="w-1.5 h-1.5 rounded-full animate-pulse bg-current" />}
      {status.label}
    </span>
  );
}

function CheckIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
    </svg>
  );
}

function XIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
    </svg>
  );
}

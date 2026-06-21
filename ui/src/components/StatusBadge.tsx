// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useTranslation } from "react-i18next";
import { CheckIcon, XIcon } from "./icons";

/**
 * Stable, language-independent status keys. These drive control flow across
 * the dashboard (terminal/active checks, pause/resume gating, counters), so
 * they must NOT be the translated display text — the human label is resolved
 * only at render via the `status` i18n namespace.
 */
export type StatusKey = "completed" | "error" | "paused" | "stopped" | "pending" | "running";

export interface StatusInfo {
  status: StatusKey;
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
  if (s === "done" || s.startsWith("completed")) return { status: "completed", color: "green" };
  if (s.startsWith("error"))                     return { status: "error",     color: "red"   };
  if (s === "paused")                            return { status: "paused",    color: "yellow" };
  if (s === "stopped")                           return { status: "stopped",   color: "gray"  };
  if (s === "pending" && canStart)               return { status: "pending",   color: "gray"  };
  return                                                { status: "running",   color: "blue"  };
}

export function StatusBadge({ status }: { status: StatusInfo }) {
  const { t } = useTranslation("status");
  const color = COLOR_TEXT[status.color];
  return (
    <span className="inline-flex items-center leading-none gap-1 text-[10px] font-medium" style={{ color }}>
      {status.status === "completed" && <CheckIcon />}
      {status.status === "error"     && <XIcon />}
      {status.status === "running"   && <span className="w-1.5 h-1.5 rounded-full animate-pulse bg-current" />}
      {t(status.status)}
    </span>
  );
}

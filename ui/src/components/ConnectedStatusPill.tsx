// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Status pill — small visual indicator above per-user credential cards.
 * Three states: connected (green), missing (amber), warning (red). Driven
 * by `state`, not the raw booleans, so callers stay readable
 * (`state="missing"` is easier to scan than `connected=false && ...`).
 */

interface Props {
  state: "connected" | "missing" | "warning";
  /** Optional sub-text (e.g. "validated 12 minutes ago"). */
  label?: string;
}

const VARIANT: Record<
  Props["state"],
  { bg: string; border: string; text: string; icon: string; title: string }
> = {
  connected: {
    bg: "bg-green-950/60",
    border: "border-green-700/50",
    text: "text-green-300",
    icon: "✅",
    title: "Connected",
  },
  missing: {
    bg: "bg-amber-950/60",
    border: "border-amber-700/50",
    text: "text-amber-300",
    icon: "⚠",
    title: "Not connected",
  },
  warning: {
    bg: "bg-red-950/60",
    border: "border-red-700/50",
    text: "text-red-300",
    icon: "❌",
    title: "Rejected",
  },
};

export function ConnectedStatusPill({ state, label }: Props) {
  const v = VARIANT[state];
  return (
    <span
      className={`inline-flex items-center gap-1.5 text-xs px-2 py-0.5 rounded-full border ${v.bg} ${v.border} ${v.text}`}
      role="status"
    >
      <span aria-hidden="true">{v.icon}</span>
      <span className="font-medium">{v.title}</span>
      {label && <span className="text-gray-400">— {label}</span>}
    </span>
  );
}

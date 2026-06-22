// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Status pill — small visual indicator above per-user credential cards.
 * States: token (green, a per-user token is saved), connected (green, a
 * deployment-level resource like the GitHub App is configured), missing
 * (amber), warning (red). Driven by `state`, not the raw booleans, so callers
 * stay readable (`state="missing"` is easier to scan than `connected=false`).
 */

import { useTranslation } from "react-i18next";

interface Props {
  state: "token" | "connected" | "missing" | "warning";
  /** Optional sub-text (e.g. "validated 12 minutes ago"). */
  label?: string;
}

const VARIANT: Record<
  Props["state"],
  { bg: string; border: string; text: string; icon: string; titleKey: string }
> = {
  token: {
    bg: "bg-green-950/60",
    border: "border-green-700/50",
    text: "text-green-300",
    icon: "✅",
    titleKey: "pill.tokenProvided",
  },
  connected: {
    bg: "bg-green-950/60",
    border: "border-green-700/50",
    text: "text-green-300",
    icon: "✅",
    titleKey: "pill.connected",
  },
  missing: {
    bg: "bg-amber-950/60",
    border: "border-amber-700/50",
    text: "text-amber-300",
    icon: "⚠",
    titleKey: "pill.missing",
  },
  warning: {
    bg: "bg-red-950/60",
    border: "border-red-700/50",
    text: "text-red-300",
    icon: "❌",
    titleKey: "pill.rejected",
  },
};

export function ConnectedStatusPill({ state, label }: Props) {
  const { t } = useTranslation("credentials");
  const v = VARIANT[state];
  return (
    <span
      className={`inline-flex items-center gap-1.5 text-xs px-2 py-0.5 rounded-full border ${v.bg} ${v.border} ${v.text}`}
      role="status"
    >
      <span aria-hidden="true">{v.icon}</span>
      <span className="font-medium">{t(v.titleKey)}</span>
      {label && <span className="text-gray-400">— {label}</span>}
    </span>
  );
}

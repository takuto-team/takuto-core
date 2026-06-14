// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { Link } from "react-router-dom";
import type { StructuredWarning, SystemStatus } from "../api/types";

interface Props {
  /**
   * Result of `GET /api/onboarding/status`. `null` means the endpoint 404'd
   * (older server) — the banner then falls back to `legacyPreflightError`.
   * `undefined` means the fetch is still in flight; render nothing.
   */
  status: SystemStatus | null | undefined;
  /**
   * Legacy single-string preflight error from `GET /api/config`. Used as the
   * back-compat fallback while we still ship one release with both surfaces
   * (04_architecture.md §1, §8).
   */
  legacyPreflightError?: string | null;
  /**
   * Whether the current user has the `admin` role. Drives the admin-only
   * deep-links (provider-switch, docs links for config/key warnings). When
   * absent, the banner falls back to the non-admin variant. Defaults to
   * `false` so Storybook stories don't need to thread the prop through.
   */
  isAdmin?: boolean;
}

/** Canonical out-of-tree docs anchor for admin-side fixes. */
const DOCS_URL = "https://github.com/morphet81/maestro/blob/main/AGENTS.md";

/**
 * Per-warning-code deep-link spec. Keyed by the `code` field emitted by
 * `crates/maestro-core/src/docker_hooks.rs::collect_system_status`. Each
 * entry yields a `CtaSpec` describing how the right-hand "Set up" link
 * should render for that code.
 *
 * `kind: "internal"` → React-Router navigation (no full-page reload).
 * `kind: "external"` → opens the docs URL in a new tab.
 * `adminOnly: true` → for non-admins, the link is replaced with greyed
 *   text directing them to ask their admin.
 *
 * Codes not in this map render with no CTA (per the table's last row).
 */
type CtaSpec =
  | { kind: "internal"; to: string; label: string; adminOnly: boolean }
  | { kind: "external"; href: string; label: string; adminOnly: boolean };

const CTA_BY_CODE: Record<string, CtaSpec> = {
  claude_not_authenticated: {
    kind: "internal",
    to: "/config.html?tab=ai",
    label: "Set Claude credential",
    adminOnly: false,
  },
  cursor_not_authenticated: {
    kind: "internal",
    to: "/config.html?tab=ai",
    label: "Set Cursor credential",
    adminOnly: false,
  },
  codex_not_authenticated: {
    kind: "internal",
    to: "/config.html?tab=ai",
    label: "Set Codex credential",
    adminOnly: false,
  },
  opencode_not_authenticated: {
    kind: "internal",
    to: "/config.html?tab=ai",
    label: "Set OpenCode credential",
    adminOnly: false,
  },
  gh_auth_missing: {
    kind: "internal",
    to: "/config.html?tab=ai",
    label: "Set GitHub PAT",
    adminOnly: false,
  },
  provider_not_implemented: {
    kind: "internal",
    to: "/config.html?tab=ai",
    label: "Change provider",
    adminOnly: true,
  },
  master_key_unavailable: {
    kind: "external",
    href: DOCS_URL,
    label: "Read docs",
    adminOnly: true,
  },
  secret_key_world_readable: {
    kind: "external",
    href: DOCS_URL,
    label: "Read docs",
    adminOnly: true,
  },
  config_missing: {
    kind: "external",
    href: DOCS_URL,
    label: "Read docs",
    adminOnly: true,
  },
  acli_missing: {
    kind: "external",
    href: DOCS_URL,
    label: "Read docs",
    adminOnly: true,
  },
};

const NON_ADMIN_HINT = "Ask your admin to change the provider";

/**
 * Dashboard banner derived from `GET /api/onboarding/status`. Renders one
 * row per critical warning, with a deep-link "Set up" button on the right
 * driven by `CTA_BY_CODE`. When the new endpoint is unavailable (older
 * server) it falls back to the legacy single-string preflight error (which
 * never carries structured codes, so no deep-links there).
 */
export function OnboardingBanner({
  status,
  legacyPreflightError,
  isAdmin = false,
}: Props) {
  // While the fetch is in flight we render nothing — the dashboard already
  // handles its own loading state and we don't want a "loading…" flicker.
  if (status === undefined) {
    return null;
  }

  // Fallback path: server is older than Phase 0 (endpoint 404'd) and we have
  // a string from /api/config. Mirror the visual shape of the new banner so
  // dashboards on both server versions look identical. No deep-links here —
  // the legacy string has no structured `code` to map.
  if (status === null) {
    if (!legacyPreflightError) return null;
    return (
      <div
        role="alert"
        className="bg-red-950/80 border-b border-red-700 px-4 py-3 text-red-200"
      >
        <div className="w-full flex items-start gap-3">
          <span aria-hidden="true" className="text-red-400 text-lg leading-none mt-0.5">
            ⚠
          </span>
          <div className="flex-1 min-w-0">
            <p className="font-semibold text-red-300 text-sm">
              Maestro is not ready — setup required
            </p>
            {legacyPreflightError.split("\n").map((line, i) => (
              <p
                key={i}
                className="text-xs text-red-300/80 mt-1 font-mono break-all"
              >
                {line}
              </p>
            ))}
            <p className="text-xs text-red-300/70 mt-1">
              Run{" "}
              <code className="bg-red-900/50 px-1 rounded">
                docker compose run --rm -it maestro setup
              </code>{" "}
              to complete setup, then restart.
            </p>
          </div>
        </div>
      </div>
    );
  }

  // Healthy or non-critical-only state: render nothing. We render one row
  // per critical warning (no grouping by code) so each warning can carry
  // its own deep-link — per task #27, "Multiple warnings with the same
  // destination should each get their own link — don't collapse."
  const criticals = status.warnings.filter((w) => w.severity === "critical");
  if (criticals.length === 0) {
    return null;
  }

  return (
    <div
      role="alert"
      className="bg-red-950/80 border-b border-red-700 px-4 py-3 text-red-200"
    >
      <div className="w-full flex items-start gap-3">
        <span aria-hidden="true" className="text-red-400 text-lg leading-none mt-0.5">
          ⚠
        </span>
        <div className="flex-1 min-w-0">
          <p className="font-semibold text-red-300 text-sm">
            Setup is not finished
          </p>
          <ul className="mt-1 space-y-1.5">
            {criticals.map((w, i) => (
              <li
                key={`${w.code}-${i}`}
                className="flex items-start justify-between gap-3 text-xs text-red-300/80"
              >
                <p className="break-words flex-1 min-w-0">{w.message}</p>
                <WarningCta warning={w} isAdmin={isAdmin} />
              </li>
            ))}
          </ul>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// CTA renderer — small enough to inline, separated for testability.
// ---------------------------------------------------------------------------

function WarningCta({
  warning,
  isAdmin,
}: {
  warning: StructuredWarning;
  isAdmin: boolean;
}) {
  const spec = CTA_BY_CODE[warning.code];
  // Unknown code → render nothing on the right. The message still shows.
  if (!spec) return null;

  // Admin-only CTAs collapse to a hint for non-admin users.
  if (spec.adminOnly && !isAdmin) {
    // Provider-switch warning gets the specific hint copy; other
    // admin-only codes (docs links, secret-key issues) use the generic
    // "Ask your admin to fix this". We discriminate on the warning *code*
    // rather than the `to` URL because the URL collapsed to a shared
    // /config.html?tab=ai destination when AI Settings was consolidated.
    const hint =
      warning.code === "provider_not_implemented"
        ? NON_ADMIN_HINT
        : "Ask your admin to fix this";
    return (
      <span
        className="text-xs text-red-300/60 italic flex-shrink-0"
        aria-label={hint}
      >
        {hint}
      </span>
    );
  }

  const className =
    "flex-shrink-0 text-xs px-2.5 py-1 rounded-md bg-red-900/60 text-red-100 border border-red-700/60 hover:bg-red-800/80 hover:text-white transition-colors whitespace-nowrap";

  if (spec.kind === "internal") {
    return (
      <Link
        to={spec.to}
        className={className}
        aria-label={`${spec.label} — fix: ${warning.message}`}
      >
        {spec.label} →
      </Link>
    );
  }

  return (
    <a
      href={spec.href}
      target="_blank"
      rel="noopener noreferrer"
      className={className}
      aria-label={`${spec.label} (opens documentation in a new tab) — fix: ${warning.message}`}
    >
      {spec.label} →
    </a>
  );
}

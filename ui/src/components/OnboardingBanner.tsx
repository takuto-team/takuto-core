// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

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
}

/**
 * Static map from a structured-warning `code` to the canonical Phase 1
 * "Set up" deep-link. Phase 1 (#13) wires the routes; Phase 0 just renders
 * the anchors so the empty-state banner shows the right copy. Codes not in
 * this map render without a CTA.
 *
 * Keep keys lowercase + snake_case to match the Rust `StructuredWarning.code`
 * convention. The href targets here are placeholders matching the routes the
 * UX doc (05_ux_design.md §1, §7) plans to create.
 */
const SETUP_HREF_BY_CODE: Record<string, string> = {
  config_missing: "/onboarding",
  github_missing: "/onboarding",
  provider_missing: "/onboarding",
  acli_missing: "/onboarding",
  setup_required: "/onboarding",
  // Anything else: no CTA — Phase 1 adds them as needed.
};

interface CriticalGroup {
  code: string;
  warnings: StructuredWarning[];
}

/** Group critical warnings by `code`, preserving first-seen order. */
function groupCritical(warnings: StructuredWarning[]): CriticalGroup[] {
  const order: string[] = [];
  const byCode = new Map<string, StructuredWarning[]>();
  for (const w of warnings) {
    if (w.severity !== "critical") continue;
    if (!byCode.has(w.code)) {
      order.push(w.code);
      byCode.set(w.code, []);
    }
    byCode.get(w.code)!.push(w);
  }
  return order.map((code) => ({ code, warnings: byCode.get(code)! }));
}

/**
 * Dashboard banner derived from `GET /api/onboarding/status`. Renders one
 * grouped row per critical warning. When the new endpoint is unavailable
 * (older server) it falls back to the legacy single-string preflight error.
 *
 * No CTA logic happens here yet — Phase 1 (#13) wires the "Set up" links.
 */
export function OnboardingBanner({ status, legacyPreflightError }: Props) {
  // While the fetch is in flight we render nothing — the dashboard already
  // handles its own loading state and we don't want a "loading…" flicker.
  if (status === undefined) {
    return null;
  }

  // Fallback path: server is older than Phase 0 (endpoint 404'd) and we have
  // a string from /api/config. Mirror the visual shape of the new banner so
  // dashboards on both server versions look identical.
  if (status === null) {
    if (!legacyPreflightError) return null;
    return (
      <div
        role="alert"
        className="bg-red-950/80 border-b border-red-700 px-4 py-3 text-red-200"
      >
        <div className="max-w-7xl mx-auto flex items-start gap-3">
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

  // Healthy or non-critical-only state: render nothing.
  const groups = groupCritical(status.warnings);
  if (groups.length === 0) {
    return null;
  }

  return (
    <div
      role="alert"
      className="bg-red-950/80 border-b border-red-700 px-4 py-3 text-red-200"
    >
      <div className="max-w-7xl mx-auto flex items-start gap-3">
        <span aria-hidden="true" className="text-red-400 text-lg leading-none mt-0.5">
          ⚠
        </span>
        <div className="flex-1 min-w-0">
          <p className="font-semibold text-red-300 text-sm">
            Setup is not finished
          </p>
          <ul className="mt-1 space-y-1">
            {groups.map((g) => {
              const href = SETUP_HREF_BY_CODE[g.code];
              return (
                <li key={g.code} className="text-xs text-red-300/80">
                  {g.warnings.map((w, i) => (
                    <p
                      key={`${g.code}-${i}`}
                      className="break-words"
                    >
                      {w.message}
                    </p>
                  ))}
                  {href && (
                    <a
                      href={href}
                      className="inline-block mt-0.5 text-red-200 hover:text-white underline underline-offset-2"
                    >
                      Set up →
                    </a>
                  )}
                </li>
              );
            })}
          </ul>
        </div>
      </div>
    </div>
  );
}

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Per-user credentials section — visible to every authenticated user.
 *
 * Lives inside the consolidated "AI Settings" tab on /config.html. Manages
 * the caller's own AI provider credential (api_key + optional Claude
 * cli_state) and their GitHub PAT.
 *
 * Source of truth: tmp/multi-agents/05_ux_design.md §2.2 (AI auth panel) +
 * §2.3 (GitHub auth panel) + 04_architecture.md §3 + §4.4.
 *
 * Hard constraints (enforced here so reviewers see them in one place):
 *   - A1: Cursor is **API-key only**. No ttyd capture, no CLI-state path.
 *     The Cursor card MUST NOT mention ttyd, "device login", "interactive
 *     terminal", or any browser-flow vocabulary. Regression-guarded in
 *     `MyCredentialsSection.test.tsx`.
 *   - A3: per-user toggle is **"Attribute commits to me"** — NOT
 *     "Sign commits". v1 does NOT do GPG/SSH signing. Regression-guarded.
 *   - All four v1 adapters (Claude, Cursor, Codex, OpenCode) are wired as
 *     of Phase 4. Each renders a paste-an-API-key card.
 */

import { useCallback, useEffect, useState } from "react";
import {
  apiJson,
  // deleteGithubPat / deleteProviderCredential intentionally NOT imported —
  // task #31 removed the Disconnect / Remove-PAT buttons because the
  // single Replace/Save flow covers rotation and revocation happens on
  // the provider side (anthropic.com / cursor.com / github.com).
  fetchUserCredentials,
  patchGithubSettings,
  setGithubPat,
  setProviderCredential,
  UserCredentialsError,
} from "../../api/client";
import { useToast } from "../../hooks/useToast";
import type {
  AuthStatus,
  GithubAuthMode,
  UserCredentialsStatus,
} from "../../api/types";
import { AiCredentialPanel } from "./AiCredentialPanel";
import { GitHubCredentialPanel } from "./GitHubCredentialPanel";
import { PROVIDER_LABEL } from "./helpers";

export function MyCredentialsSection() {
  const { showToast } = useToast();
  const [creds, setCreds] = useState<UserCredentialsStatus | null>(null);
  const [auth, setAuth] = useState<AuthStatus | null>(null);
  // Split state: `initialLoading` gates the first paint only. Subsequent
  // refetches (triggered by save handlers) do NOT flip it back to true, so
  // the credential panels stay mounted across save → refresh → toast.
  //
  // **Root cause of #31 issue C:** the previous `refresh()` set
  // `loading=true` for the *post-save* refetch as well, which unmounted the
  // entire credential panel. When the panel remounted with the fresh
  // `credentials` prop, its local `apiKey`/`saving` state was reset, the
  // toast had ALREADY fired during the loading window, and React's
  // commit/batch ordering left the user looking at a freshly-mounted panel
  // whose pill *was* "Connected" but whose perceived experience was: "I
  // pressed Save, the page blanked, the pill is back to Not connected
  // (until I refresh)."
  //
  // The fix is structural — don't ever unmount the panel during a save.
  // The save handler stays linear: POST → await background refetch → toast.
  const [initialLoading, setInitialLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  /**
   * Returns the in-flight Promise so callers can `await` it. Refreshes
   * both `creds` AND `auth` (provider_selected + github_mode live on
   * /api/auth/status). Crucially does NOT flip `initialLoading`, so the
   * panel stays mounted while the refetch is in flight.
   */
  const refresh = useCallback(async () => {
    const [c, a] = await Promise.all([
      fetchUserCredentials().catch(() => null),
      apiJson<AuthStatus>("/api/auth/status").catch(() => null),
    ]);
    setCreds(c);
    setAuth(a);
    setLoadError(c ? null : "Could not load your credentials.");
  }, []);

  // Initial mount: refetch, then flip `initialLoading` once. After this
  // the panels are mounted for the lifetime of the section; save handlers
  // call `refresh()` directly without touching `initialLoading`.
  useEffect(() => {
    let mounted = true;
    refresh().finally(() => {
      if (mounted) setInitialLoading(false);
    });
    return () => {
      mounted = false;
    };
  }, [refresh]);

  /**
   * Provider name the admin has selected, used to pick which AI card to
   * render. Falls back to whatever the user already has stored, then "claude"
   * as the absolute default.
   */
  const adminProvider = auth?.provider_selected ?? null;
  // Wire-format note: the backend returns `provider.provider` (matches the
  // `provider` column in `user_provider_credentials`). See
  // `crates/maestro-web/src/routes/credentials.rs::ProviderCredentialStatus`.
  const userProvider = creds?.provider?.provider ?? null;
  const activeProvider = adminProvider ?? userProvider ?? "claude";

  // Mismatch banner: admin switched the deployment provider but the user
  // still has a stored credential for the old one. UX §2.2 last row.
  const showProviderMismatch =
    !!adminProvider && !!userProvider && adminProvider !== userProvider;

  const handleSurfaceError = useCallback(
    (e: unknown, fallback: string) => {
      if (e instanceof UserCredentialsError) {
        if (e.code === "sso_authorization_required" && e.orgSsoUrl) {
          showToast(
            `GitHub SSO required. Authorize at ${e.orgSsoUrl} and try again.`,
            "error",
          );
          return;
        }
        showToast(`${e.message} (code: ${e.code})`, "error");
        return;
      }
      showToast(e instanceof Error ? e.message : fallback, "error");
    },
    [showToast],
  );

  return (
    <section
      aria-labelledby="my-credentials-section-title"
      className="flex flex-col gap-3"
    >
      <h2
        id="my-credentials-section-title"
        className="text-lg font-semibold text-white"
      >
        My credentials
      </h2>
      <p className="text-xs text-gray-500">
        Your personal AI provider and GitHub tokens. Stored encrypted per-user;
        workflows you start use these instead of the deployment default.
      </p>

      {initialLoading && <p className="text-sm text-gray-500">Loading…</p>}
      {!initialLoading && loadError && (
        <p className="text-sm text-red-400">{loadError}</p>
      )}

      {!initialLoading && (
        <div className="flex flex-col gap-6">
          {showProviderMismatch && adminProvider && userProvider && (
            <div
              role="alert"
              className="bg-amber-950/40 border border-amber-700/50 rounded-lg p-3 text-xs text-amber-200"
            >
              Your admin switched the AI provider to{" "}
              <strong>{PROVIDER_LABEL[adminProvider] ?? adminProvider}</strong>
              . Your <strong>{PROVIDER_LABEL[userProvider] ?? userProvider}</strong>{" "}
              credential is paused — connect the new provider below to keep
              running workflows.
            </div>
          )}

          <AiCredentialPanel
            activeProvider={activeProvider}
            credentials={creds}
            onSave={async (body) => {
              try {
                // Body is the discriminated request shape (`{ api_key }`
                // or `{ kind: "cli_state", claude_session_json }`). The
                // panel constructs the right body based on the active tab.
                await setProviderCredential(activeProvider, body);
                // Refresh the server state BEFORE toasting "connected" so
                // the pill flips at the same instant the user sees the
                // success message. `refresh()` no longer toggles the
                // page-level loading flag, so the panel stays mounted.
                await refresh();
                const providerLabel =
                  PROVIDER_LABEL[activeProvider] ?? activeProvider;
                const what =
                  body.kind === "cli_state"
                    ? "session uploaded"
                    : "connected";
                showToast(`${providerLabel} ${what}.`, "success");
              } catch (e: unknown) {
                handleSurfaceError(e, "Could not save your credential.");
              }
            }}
          />

          <GitHubCredentialPanel
            github={creds?.github ?? null}
            authMode={auth?.github_mode as GithubAuthMode | undefined}
            onSavePat={async (pat, attribute) => {
              try {
                // Capture login from the response *before* re-fetching so we
                // can use it in the success toast. The refresh() call
                // refreshes both `creds.github` and `auth.github_mode` (which
                // flips app → app_plus_pat once the PAT lands).
                const next = await setGithubPat({
                  pat,
                  attribute_commits: attribute,
                });
                await refresh();
                showToast(
                  `GitHub token saved — you're @${next.github?.login ?? "?"}.`,
                  "success",
                );
              } catch (e: unknown) {
                handleSurfaceError(e, "Could not save your GitHub token.");
              }
            }}
            onToggleAttributeCommits={async (attribute) => {
              try {
                await patchGithubSettings({ attribute_commits: attribute });
                await refresh();
              } catch (e: unknown) {
                handleSurfaceError(e, "Could not update GitHub settings.");
              }
            }}
          />
        </div>
      )}
    </section>
  );
}

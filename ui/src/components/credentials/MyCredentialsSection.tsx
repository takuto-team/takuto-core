// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Per-user AI credentials section — visible to every authenticated user.
 *
 * Lives inside the consolidated "AI Settings" tab on /config.html. Manages
 * the caller's own AI provider credential (api_key + optional Claude
 * cli_state). The per-user GitHub PAT lives on its own "GitHub" tab
 * (`GitHubCredentialsSection`).
 *
 * Source of truth: tmp/multi-agents/05_ux_design.md §2.2 (AI auth panel) +
 * 04_architecture.md §3 + §4.4.
 *
 * Hard constraints (enforced here so reviewers see them in one place):
 *   - A1: Cursor is **API-key only**. No ttyd capture, no CLI-state path.
 *     The Cursor card MUST NOT mention ttyd, "device login", "interactive
 *     terminal", or any browser-flow vocabulary. Regression-guarded in
 *     `MyCredentialsSection.test.tsx`.
 *   - All four v1 adapters (Claude, Cursor, Codex, OpenCode) are wired as
 *     of Phase 4. Each renders a paste-an-API-key card.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  apiJson,
  // An explicit per-provider Delete button is supported again: it scopes the
  // hard-delete to the CURRENT provider + slot, so a stored key can be cleared
  // without having to overwrite it (and without ever touching another
  // provider's row). Rotation still flows through the Replace/Save button.
  deleteProviderCredential,
  fetchUserCredentials,
  setProviderCredential,
  UserCredentialsError,
} from "../../api/client";
import { useToast } from "../../hooks/useToast";
import type {
  AuthStatus,
  ProviderCredentialKind,
  UserCredentialsStatus,
} from "../../api/types";
import { AiCredentialPanel } from "./AiCredentialPanel";
import { PROVIDER_LABEL } from "./helpers";

interface Props {
  /** Bump to force a refetch — e.g. when the admin changes the active provider
   *  in a sibling section, so the credential card reflects the new provider
   *  without a manual page reload. */
  refreshKey?: number;
  /** Reports `true` when a credential field holds a typed-but-unsaved value, so
   *  the AI Settings tab can warn before navigation. */
  onDirtyChange?: (dirty: boolean) => void;
}

export function MyCredentialsSection({ refreshKey = 0, onDirtyChange }: Props = {}) {
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

  // Refetch when the parent bumps `refreshKey` (e.g. the admin switched the
  // active provider) so the card reflects the new provider in place — no
  // unmount (initialLoading untouched), no double-fetch on first render.
  const firstRender = useRef(true);
  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    void refresh();
  }, [refreshKey, refresh]);

  /**
   * Provider name the admin has selected, used to pick which AI card to
   * render. Falls back to whatever the user already has stored, then "claude"
   * as the absolute default.
   */
  const adminProvider = auth?.provider_selected ?? null;
  // Wire-format note: the backend returns `provider.provider` (matches the
  // `provider` column in `user_provider_credentials`). See
  // `crates/takuto-web/src/routes/credentials.rs::ProviderCredentialStatus`.
  const userProvider = creds?.provider?.provider ?? null;
  const activeProvider = adminProvider ?? userProvider ?? "claude";

  // Mismatch banner: admin switched the deployment provider but the user
  // still has a stored credential for the old one. UX §2.2 last row.
  const showProviderMismatch =
    !!adminProvider && !!userProvider && adminProvider !== userProvider;

  const handleSurfaceError = useCallback(
    (e: unknown, fallback: string) => {
      if (e instanceof UserCredentialsError) {
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
        Your personal AI provider token. Stored encrypted per-user; workflows
        you start use this instead of the deployment default.
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
            onDirtyChange={onDirtyChange}
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
                return true;
              } catch (e: unknown) {
                handleSurfaceError(e, "Could not save your credential.");
                return false;
              }
            }}
            onDelete={async (kind: ProviderCredentialKind) => {
              try {
                // Scoped to the CURRENT provider + slot — never another
                // provider's row.
                await deleteProviderCredential(activeProvider, kind);
                await refresh();
                const providerLabel =
                  PROVIDER_LABEL[activeProvider] ?? activeProvider;
                showToast(`${providerLabel} key removed.`, "success");
                return true;
              } catch (e: unknown) {
                handleSurfaceError(e, "Could not delete your credential.");
                return false;
              }
            }}
          />
        </div>
      )}
    </section>
  );
}

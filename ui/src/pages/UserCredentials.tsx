// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Phase 2 per-user credential surface — `/me/credentials`.
 *
 * Source of truth: tmp/multi-agents/05_ux_design.md §2.2 (AI auth panel) +
 * §2.3 (GitHub auth panel) + 04_architecture.md §3 + §4.4.
 *
 * Hard constraints (enforced here so reviewers see them in one place):
 *   - A1: Cursor is **API-key only**. No ttyd capture, no CLI-state path.
 *     The Cursor card MUST NOT mention ttyd, "device login", "interactive
 *     terminal", or any browser-flow vocabulary. Regression-guarded in
 *     `UserCredentials.test.tsx`.
 *   - A3: per-user toggle is **"Attribute commits to me"** — NOT
 *     "Sign commits". v1 does NOT do GPG/SSH signing. Regression-guarded.
 *   - Codex / OpenCode adapters ship in Phase 4 — their cards are grey,
 *     read-only "Coming in Phase 4" boxes.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
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
} from "../api/client";
import { ConnectedStatusPill } from "../components/ConnectedStatusPill";
import { CredentialPasteField } from "../components/CredentialPasteField";
import { useToast } from "../hooks/useToast";
import type {
  AuthStatus,
  GithubAuthMode,
  UserCredentialsStatus,
} from "../api/types";

interface Props {
  onLogout: () => void;
  authEnabled: boolean;
}

/** Providers Phase 4 will ship; their card is read-only for Phase 2. */
const PHASE_4_PROVIDERS: ReadonlySet<string> = new Set(["codex", "opencode"]);

const PROVIDER_LABEL: Record<string, string> = {
  claude: "Claude",
  cursor: "Cursor",
  codex: "Codex",
  opencode: "OpenCode",
  gemini: "Gemini",
};

export function UserCredentials({ onLogout, authEnabled }: Props) {
  const { showToast } = useToast();
  const [creds, setCreds] = useState<UserCredentialsStatus | null>(null);
  const [auth, setAuth] = useState<AuthStatus | null>(null);
  // Split state: `initialLoading` gates the first paint only. Subsequent
  // refetches (triggered by save handlers) do NOT flip it back to true, so
  // the credential panels stay mounted across save → refresh → toast.
  //
  // **Root cause of #31 issue C:** the previous `refresh()` set
  // `loading=true` for the *post-save* refetch as well, which unmounted the
  // entire credential panel (it's behind `{!loading && <Panel/>}`). When
  // the panel remounted with the fresh `credentials` prop, its local
  // `apiKey`/`saving` state was reset, the toast had ALREADY fired during
  // the loading window, and React's commit/batch ordering left the user
  // looking at a freshly-mounted panel whose pill *was* "Connected" but
  // whose perceived experience was: "I pressed Save, the page blanked, the
  // pill is back to Not connected (until I refresh)." On slow networks the
  // unmount window also collapsed back to the empty-state branch before
  // remounting, which produced exactly the symptom the user reported.
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
  // the panels are mounted for the lifetime of the page; save handlers
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
    <div className="min-h-screen">
      <header className="border-b border-gray-800 bg-gray-950/80 backdrop-blur-sm sticky top-0 z-40">
        <div className="max-w-3xl mx-auto px-4 sm:px-6 lg:px-8">
          <div className="flex items-center justify-between h-14">
            <Link
              to="/"
              className="flex items-center gap-2 text-gray-400 hover:text-gray-200 transition-colors text-sm"
            >
              &larr; Dashboard
            </Link>
            <span className="text-lg font-bold text-white">My credentials</span>
            {authEnabled && (
              <button
                onClick={onLogout}
                className="text-xs text-gray-500 hover:text-gray-300 cursor-pointer"
              >
                Log out
              </button>
            )}
          </div>
        </div>
      </header>

      <main className="max-w-3xl mx-auto px-4 sm:px-6 lg:px-8 py-8 flex flex-col gap-6">
        {initialLoading && (
          <p className="text-sm text-gray-500">Loading…</p>
        )}
        {!initialLoading && loadError && (
          <p className="text-sm text-red-400">{loadError}</p>
        )}

        {!initialLoading && (
          <>
            {showProviderMismatch && (
              <div
                role="alert"
                className="bg-amber-950/40 border border-amber-700/50 rounded-lg p-3 text-xs text-amber-200"
              >
                Your admin switched the AI provider to{" "}
                <strong>{PROVIDER_LABEL[adminProvider!] ?? adminProvider}</strong>
                . Your <strong>{PROVIDER_LABEL[userProvider!] ?? userProvider}</strong>{" "}
                credential is paused — connect the new provider below to keep
                running workflows.
              </div>
            )}

            <AiCredentialPanel
              activeProvider={activeProvider}
              credentials={creds}
              onSave={async (apiKey) => {
                try {
                  await setProviderCredential(activeProvider, { api_key: apiKey });
                  // Refresh the server state BEFORE toasting "connected" so
                  // the pill flips at the same instant the user sees the
                  // success message. `refresh()` no longer toggles the
                  // page-level loading flag, so the panel stays mounted —
                  // see the comment on `refresh` above for the root-cause
                  // analysis (#31 issue C).
                  await refresh();
                  showToast(
                    `${PROVIDER_LABEL[activeProvider] ?? activeProvider} connected.`,
                    "success",
                  );
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
                  // Capture login from the response *before* re-fetching so
                  // we can use it in the success toast. The refresh() call
                  // refreshes both `creds.github` and `auth.github_mode`
                  // (which flips app → app_plus_pat once the PAT lands).
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
          </>
        )}
      </main>
    </div>
  );
}

// ---------------------------------------------------------------------------
// AI provider card.
// ---------------------------------------------------------------------------

interface AiCredentialPanelProps {
  activeProvider: string;
  credentials: UserCredentialsStatus | null;
  onSave: (apiKey: string) => Promise<void>;
}

function AiCredentialPanel({
  activeProvider,
  credentials,
  onSave,
}: AiCredentialPanelProps) {
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);

  const isPhase4 = PHASE_4_PROVIDERS.has(activeProvider);
  // Wire-format note: the backend's row is named `provider.provider` (the
  // provider this credential was sealed for) and `provider.active` (false
  // only after a provider switch deactivated it). See
  // `routes/credentials.rs::ProviderCredentialStatus`.
  const hasMatchingCredential =
    !!credentials?.provider &&
    credentials.provider.provider === activeProvider &&
    credentials.provider.active;

  const label = PROVIDER_LABEL[activeProvider] ?? activeProvider;
  const helper = useMemo(() => providerHelper(activeProvider), [activeProvider]);
  const lastValidated = credentials?.provider?.last_validated_at ?? null;

  const submit = async () => {
    setSaving(true);
    try {
      await onSave(apiKey);
      setApiKey("");
    } finally {
      setSaving(false);
    }
  };

  return (
    <section
      aria-labelledby="ai-card-title"
      className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-4"
    >
      <div className="flex items-center justify-between gap-3 flex-wrap">
        <h2 id="ai-card-title" className="text-lg font-semibold text-white">
          AI provider — {label}
        </h2>
        <ConnectedStatusPill
          state={hasMatchingCredential ? "connected" : "missing"}
          label={
            hasMatchingCredential && lastValidated
              ? `validated ${relativeTime(lastValidated)}`
              : undefined
          }
        />
      </div>

      {isPhase4 ? (
        <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 text-sm text-gray-400">
          <p>
            <strong className="text-gray-200">Coming in Phase 4</strong> —{" "}
            {label}'s adapter ships alongside the multi-provider stream parser.
            You'll be able to paste a key here once that lands.
          </p>
        </div>
      ) : (
        // Issues A + B from #31: no Rotate / Disconnect buttons.
        // The single Replace/Save button covers rotation; revocation lives
        // on the provider side (anthropic.com / cursor.com / github.com).
        // To wipe the local row, the user pastes a different key.
        <CredentialPasteField
          label={`${label} API key`}
          value={apiKey}
          onChange={setApiKey}
          onSubmit={submit}
          saving={saving}
          placeholder={`Paste your ${label} API key`}
          helper={helper}
          saveLabel={hasMatchingCredential ? "Replace" : "Save"}
        />
      )}
    </section>
  );
}

function providerHelper(provider: string): string {
  switch (provider) {
    case "cursor":
      // A1 regression guard: no ttyd / browser-flow vocabulary here.
      return "Cursor accepts only an API key. Generate one at cursor.com/dashboard and paste it above.";
    case "claude":
      return "Get a Claude API key at anthropic.com/settings, or paste a CLAUDE_CODE_OAUTH_TOKEN. The server stores it encrypted; it never leaves your browser unencrypted.";
    case "codex":
    case "opencode":
      return "Phase 4 ships the adapter.";
    default:
      return "Paste the API key issued by your provider.";
  }
}

// ---------------------------------------------------------------------------
// GitHub auth card.
// ---------------------------------------------------------------------------

interface GitHubCredentialPanelProps {
  github: UserCredentialsStatus["github"] | null;
  authMode: GithubAuthMode | undefined;
  onSavePat: (pat: string, attributeCommits: boolean) => Promise<void>;
  onToggleAttributeCommits: (next: boolean) => Promise<void>;
}

function GitHubCredentialPanel({
  github,
  authMode,
  onSavePat,
  onToggleAttributeCommits,
}: GitHubCredentialPanelProps) {
  const [pat, setPat] = useState("");
  const [attribute, setAttribute] = useState(github?.attribute_commits ?? true);
  const [saving, setSaving] = useState(false);
  // Keep local toggle in sync with server state on refresh.
  useEffect(() => {
    setAttribute(github?.attribute_commits ?? true);
  }, [github?.attribute_commits]);

  // Wire-format note: presence of a PAT is derived from the parent's
  // `github` field being non-null. The backend never returns `has_pat` —
  // see `routes/credentials.rs::GithubCredentialStatus`. The effective mode
  // lives on `/api/auth/status::github_mode`.
  const hasPat = github != null;
  const effectiveMode = authMode ?? "missing";

  const submit = async () => {
    setSaving(true);
    try {
      await onSavePat(pat, attribute);
      setPat("");
    } finally {
      setSaving(false);
    }
  };

  // Issue B from #31: no "Remove PAT" button. PAT revocation happens on
  // github.com; to wipe the local row the user saves a different token.

  const toggle = async (next: boolean) => {
    setAttribute(next);
    try {
      await onToggleAttributeCommits(next);
    } catch {
      // Revert on failure — parent surfaces the toast.
      setAttribute((v) => !v);
    }
  };

  return (
    <section
      aria-labelledby="gh-card-title"
      className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-4"
    >
      <div className="flex items-center justify-between gap-3 flex-wrap">
        <h2 id="gh-card-title" className="text-lg font-semibold text-white">
          GitHub
        </h2>
        <ConnectedStatusPill
          state={hasPat ? "connected" : effectiveMode === "app" ? "connected" : "missing"}
          label={describeMode(effectiveMode)}
        />
      </div>

      {effectiveMode === "app" && !hasPat && (
        <p className="text-sm text-gray-400">
          Maestro is using its GitHub App. Workflows run as the bot. Add a
          personal access token below if you want commits and PRs attributed
          to you.
        </p>
      )}
      {effectiveMode === "pat_only" && !hasPat && (
        <p className="text-sm text-amber-300">
          No shared GitHub App is configured. Maestro can only talk to GitHub
          via a personal access token — without one, GitHub-touching
          workflows won't start.
        </p>
      )}
      {hasPat && (
        <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 text-sm text-gray-300">
          <p>
            Logged in as{" "}
            <strong className="text-gray-200">@{github?.login ?? "?"}</strong>
            {github?.scopes && github.scopes.length > 0 && (
              <>
                {" · "}Scopes: {github.scopes.join(", ")}
              </>
            )}
          </p>
          <p className="text-xs text-gray-500 mt-1">
            Your commits, PRs, and PR comments are attributed to you. The
            GitHub App handles read-only API calls.
          </p>
        </div>
      )}

      {/* A3 regression guard: this toggle is **"Attribute commits to me"** —
          NOT "Sign commits". v1 does NOT GPG/SSH-sign. */}
      <div className="flex items-start gap-2 bg-gray-950/40 border border-gray-800 rounded-lg p-3">
        <input
          id="attribute-commits-toggle"
          type="checkbox"
          checked={attribute}
          disabled={!hasPat || saving}
          onChange={(e) => void toggle(e.target.checked)}
          className="mt-1 accent-blue-500"
        />
        <label
          htmlFor="attribute-commits-toggle"
          className="text-sm text-gray-300"
        >
          Attribute commits to me
          <p className="text-xs text-gray-500 mt-0.5">
            Your GitHub username and email will appear as the author on
            commits, PRs, and review comments. Cryptographic signing is a v2
            feature.
          </p>
        </label>
      </div>

      <CredentialPasteField
        label={hasPat ? "Replace personal access token" : "Personal access token"}
        value={pat}
        onChange={setPat}
        onSubmit={submit}
        saving={saving}
        placeholder="ghp_…"
        saveLabel={hasPat ? "Replace" : "Validate & save"}
        helper={
          <>
            Required scopes: <code className="text-gray-400">repo</code>{" "}
            (classic) or{" "}
            <code className="text-gray-400">
              contents:write + pull_requests:write + issues:read
            </code>{" "}
            (fine-grained).{" "}
            <a
              href="https://github.com/settings/tokens"
              target="_blank"
              rel="noopener noreferrer"
              className="text-blue-400 hover:text-blue-300"
              aria-label="Open GitHub PAT creation page (opens in a new tab)"
            >
              Help me create one →
            </a>
          </>
        }
      />

    </section>
  );
}

function describeMode(mode: GithubAuthMode): string {
  switch (mode) {
    case "app":
      return "App only";
    case "app_plus_pat":
      return "App + your PAT";
    case "pat_only":
      return "PAT only";
    case "pat_required":
      return "PAT required";
    case "missing":
      return "Not connected";
  }
}

/** Tiny relative-time helper — used only for the "validated X ago" label. */
function relativeTime(iso: string): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return iso;
  const delta = Math.max(0, Date.now() - t);
  const mins = Math.round(delta / 60_000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins} minute${mins === 1 ? "" : "s"} ago`;
  const hours = Math.round(mins / 60);
  if (hours < 48) return `${hours} hour${hours === 1 ? "" : "s"} ago`;
  const days = Math.round(hours / 24);
  return `${days} day${days === 1 ? "" : "s"} ago`;
}

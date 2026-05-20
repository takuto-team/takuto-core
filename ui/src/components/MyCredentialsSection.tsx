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

import { useCallback, useEffect, useMemo, useState } from "react";
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
import { ConnectedStatusPill } from "./ConnectedStatusPill";
import { CredentialPasteField } from "./CredentialPasteField";
import { useToast } from "../hooks/useToast";
import type {
  AuthStatus,
  GithubAuthMode,
  SetProviderCredentialRequest,
  UserCredentialsStatus,
} from "../api/types";
import { parseClaudeSessionBlob } from "../utils/claudeSession";

const PROVIDER_LABEL: Record<string, string> = {
  claude: "Claude",
  cursor: "Cursor",
  codex: "Codex",
  opencode: "OpenCode",
  gemini: "Gemini",
};

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
            onSave={async (body) => {
              try {
                // Task #40: body is the discriminated request shape
                // (`{ api_key }` or `{ kind: "cli_state",
                // claude_session_json }`). The panel constructs the right
                // body based on the active tab.
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

// ---------------------------------------------------------------------------
// AI provider card.
// ---------------------------------------------------------------------------

interface AiCredentialPanelProps {
  activeProvider: string;
  credentials: UserCredentialsStatus | null;
  onSave: (body: SetProviderCredentialRequest) => Promise<void>;
}

type ClaudeAuthMethod = "api_key" | "cli_state";

export function AiCredentialPanel({
  activeProvider,
  credentials,
  onSave,
}: AiCredentialPanelProps) {
  const [apiKey, setApiKey] = useState("");
  const [sessionJson, setSessionJson] = useState("");
  const [saving, setSaving] = useState(false);
  // Inline pre-flight validation error for the session blob. Set by the
  // Save path so the user sees structured feedback BEFORE the server
  // round-trip (#40 T-CLAUDE-UI-006). Cleared on each edit.
  const [sessionError, setSessionError] = useState<string | null>(null);
  const [claudeTab, setClaudeTab] = useState<ClaudeAuthMethod>("api_key");

  const isClaude = activeProvider === "claude";

  // Wire-format note: the GET response now carries a bundle (api_key +
  // cli_state slots) per task #39. See
  // `routes/credentials.rs::ProviderCredentialBundle`.
  const bundle = credentials?.provider ?? null;
  const bundleMatches = !!bundle && bundle.provider === activeProvider;
  const apiKeyActive = bundleMatches && !!bundle?.api_key?.active;
  const cliStateActive = bundleMatches && !!bundle?.cli_state?.active;
  // The card is "connected" if EITHER slot has an active row.
  const hasMatchingCredential = apiKeyActive || cliStateActive;

  const label = PROVIDER_LABEL[activeProvider] ?? activeProvider;
  const apiKeyHelper = useMemo(
    () => providerHelper(activeProvider, "api_key"),
    [activeProvider],
  );
  const sessionHelper = useMemo(
    () => providerHelper(activeProvider, "cli_state"),
    [activeProvider],
  );

  /**
   * Pick the most informative status pill label.
   *   - Both kinds connected   → "API key + Session"
   *   - Only api_key connected → "API key"
   *   - Only cli_state         → "Session"
   *   - Neither                → undefined (pill shows base copy)
   */
  const pillLabel = useMemo(() => {
    if (apiKeyActive && cliStateActive) return "API key + Session";
    if (apiKeyActive) return "API key";
    if (cliStateActive) return "Session";
    return undefined;
  }, [apiKeyActive, cliStateActive]);

  const submitApiKey = async () => {
    setSaving(true);
    try {
      await onSave({ api_key: apiKey });
      setApiKey("");
    } finally {
      setSaving(false);
    }
  };

  const submitSession = async () => {
    // #40 T-CLAUDE-UI-006: client-side validation BEFORE the POST. Surface
    // structured errors inline so the user can correct without a round-trip.
    const result = parseClaudeSessionBlob(sessionJson);
    if (!result.ok) {
      setSessionError(result.message);
      return;
    }
    setSessionError(null);
    setSaving(true);
    try {
      await onSave({
        kind: "cli_state",
        claude_session_json: sessionJson,
      });
      setSessionJson("");
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
        <h3 id="ai-card-title" className="text-lg font-semibold text-white">
          AI provider — {label}
        </h3>
        <ConnectedStatusPill
          state={hasMatchingCredential ? "connected" : "missing"}
          label={pillLabel}
        />
      </div>

      {isClaude ? (
        <>
          {/* #40: Claude is the only provider that accepts cli_state today.
              Render an "Auth method" segmented control so users on Claude
              Code Pro/Team can upload their ~/.claude.json blob in addition
              to (or instead of) a bearer API key. */}
          <div
            role="tablist"
            aria-label="Claude auth method"
            className="flex gap-1 p-1 bg-gray-950/60 border border-gray-800 rounded-lg w-fit"
          >
            <ClaudeAuthTabButton
              isActive={claudeTab === "api_key"}
              connected={apiKeyActive}
              onClick={() => setClaudeTab("api_key")}
              label="API key"
            />
            <ClaudeAuthTabButton
              isActive={claudeTab === "cli_state"}
              connected={cliStateActive}
              onClick={() => setClaudeTab("cli_state")}
              label="Claude Code session"
            />
          </div>

          {claudeTab === "api_key" ? (
            <CredentialPasteField
              label="Claude API key"
              value={apiKey}
              onChange={setApiKey}
              onSubmit={submitApiKey}
              saving={saving}
              placeholder="sk-ant-… or CLAUDE_CODE_OAUTH_TOKEN"
              helper={apiKeyHelper}
              saveLabel={apiKeyActive ? "Replace" : "Save"}
            />
          ) : (
            <ClaudeSessionField
              value={sessionJson}
              onChange={(v) => {
                setSessionJson(v);
                if (sessionError) setSessionError(null);
              }}
              onSubmit={submitSession}
              saving={saving}
              error={sessionError}
              connected={cliStateActive}
              helper={sessionHelper}
            />
          )}
        </>
      ) : (
        // Issues A + B from #31: no Rotate / Disconnect buttons.
        // The single Replace/Save button covers rotation; revocation lives
        // on the provider side (anthropic.com / cursor.com / github.com).
        // To wipe the local row, the user pastes a different key.
        <CredentialPasteField
          label={`${label} API key`}
          value={apiKey}
          onChange={setApiKey}
          onSubmit={submitApiKey}
          saving={saving}
          placeholder={`Paste your ${label} API key`}
          helper={apiKeyHelper}
          saveLabel={apiKeyActive ? "Replace" : "Save"}
        />
      )}
    </section>
  );
}

/**
 * Tab button for the Claude auth-method selector. Renders a small dot
 * indicator when that kind is already connected so the user can see at a
 * glance which mode(s) they've already saved.
 */
function ClaudeAuthTabButton({
  isActive,
  connected,
  onClick,
  label,
}: {
  isActive: boolean;
  connected: boolean;
  onClick: () => void;
  label: string;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={isActive}
      onClick={onClick}
      className={`px-3 py-1.5 text-xs rounded-md cursor-pointer transition-colors flex items-center gap-1.5 ${
        isActive ? "bg-gray-800 text-white" : "text-gray-400 hover:text-gray-200"
      }`}
    >
      {connected && (
        <span
          aria-label="connected"
          className="inline-block w-1.5 h-1.5 rounded-full bg-green-400"
        />
      )}
      {label}
    </button>
  );
}

/**
 * `~/.claude.json` paste field — large textarea with inline help and a
 * client-side validation message slot. The Save handler runs the structural
 * check (`parseClaudeSessionBlob`) before the POST so users see obvious
 * shape problems without a round-trip.
 */
function ClaudeSessionField({
  value,
  onChange,
  onSubmit,
  saving,
  error,
  connected,
  helper,
}: {
  value: string;
  onChange: (v: string) => void;
  onSubmit: () => void;
  saving: boolean;
  error: string | null;
  connected: boolean;
  helper: string;
}) {
  const [showHelp, setShowHelp] = useState(false);
  const canSubmit = !saving && value.trim().length > 0;
  return (
    <div className="flex flex-col gap-2">
      <label
        htmlFor="claude-session-textarea"
        className="text-xs text-gray-400"
      >
        Paste contents of your local{" "}
        <code className="text-gray-300">~/.claude.json</code>
      </label>
      <textarea
        id="claude-session-textarea"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder='{ "oauthAccount": { "accountUuid": "…", "emailAddress": "you@example.com", "organizationUuid": "…" }, … }'
        rows={12}
        spellCheck={false}
        autoComplete="off"
        disabled={saving}
        className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-xs text-gray-200 font-mono whitespace-pre"
        aria-invalid={error !== null}
        aria-describedby={error ? "claude-session-error" : undefined}
      />
      {error && (
        <p
          id="claude-session-error"
          role="alert"
          className="text-xs text-red-300 bg-red-950/40 border border-red-700/50 rounded px-2 py-1.5"
        >
          {error}
        </p>
      )}
      <p className="text-xs text-gray-500">{helper}</p>
      <p className="text-xs text-gray-500">
        <button
          type="button"
          onClick={() => setShowHelp((v) => !v)}
          className="text-blue-400 hover:text-blue-300 cursor-pointer"
          aria-expanded={showHelp}
        >
          {showHelp ? "Hide" : "Where do I find it?"}
        </button>
      </p>
      {showHelp && (
        <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-3 text-xs text-gray-400 space-y-2">
          <p>
            On macOS / Linux, run{" "}
            <code className="text-gray-300">cat ~/.claude.json</code> in your
            shell and copy the output.
          </p>
          <p>
            Maestro only needs the{" "}
            <code className="text-gray-300">oauthAccount</code> block (with{" "}
            <code className="text-gray-300">accountUuid</code>,{" "}
            <code className="text-gray-300">emailAddress</code>, and{" "}
            <code className="text-gray-300">organizationUuid</code>) but
            pasting the full file is fine — the server ignores extra fields.
          </p>
          <p>
            The bearer token is still set separately on the{" "}
            <strong>API key</strong> tab.
          </p>
        </div>
      )}
      <div className="flex justify-end">
        <button
          type="button"
          disabled={!canSubmit}
          onClick={onSubmit}
          className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
        >
          {saving ? "Saving…" : connected ? "Replace session" : "Save session"}
        </button>
      </div>
    </div>
  );
}

function providerHelper(
  provider: string,
  kind: "api_key" | "cli_state",
): string {
  if (kind === "cli_state") {
    // Only Claude renders this branch (task #39 amendment).
    return "Required for Pro/Team accounts whose local `claude` uses `/login`. Maestro reads `oauthAccount` from this blob and writes it to the worker's `.claude.json` at workflow start. The bearer token is still set separately on the API key tab.";
  }
  switch (provider) {
    case "cursor":
      // A1 regression guard: no ttyd / browser-flow vocabulary here.
      return "Cursor accepts only an API key. Generate one at cursor.com/dashboard and paste it above.";
    case "claude":
      return "For direct Anthropic API or proxies that accept the same API key format. If you're on Pro/Team and your local `claude` uses `/login`, use 'Claude Code session' instead.";
    case "codex":
      return "OpenAI API key (sk-…). The Codex CLI reads OPENAI_API_KEY from the worker environment — Maestro bridges this from the value you paste here.";
    case "opencode":
      return "OpenCode credential (anthropic-style key or any provider key — depends on which provider you've configured in [agent.providers.opencode]). Note: opencode does NOT auto-read env vars; admin must configure a provider in opencode.json.";
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

export function GitHubCredentialPanel({
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
        <h3 id="gh-card-title" className="text-lg font-semibold text-white">
          GitHub
        </h3>
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
          via a personal access token — without one, GitHub-touching workflows
          won't start.
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

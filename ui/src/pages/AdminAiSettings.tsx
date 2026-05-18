// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Phase 1 (auth-overhaul) — admin AI Settings page.
 *
 * Source of truth: tmp/multi-agents/04_architecture.md §2 and
 * tmp/multi-agents/05_ux_design.md §2.5 / §2.6.
 *
 * What this page is NOT:
 *   - Not a per-user credential surface (Phase 2 ships `/users/me/credentials`).
 *   - Not a provider "Test" button (deferred to Phase 2 alongside validation).
 *
 * What it IS: a single PUT /api/config/agent panel that lets an admin pick
 * the active provider, edit its sub-table (base_url, model, extras,
 * allow_shared_default), and toggle the user-facing `available_providers`
 * whitelist. Switching the active provider triggers a confirm modal
 * (05_ux_design.md §2.6) because Phase 2 will mark every per-user credential
 * for the previous provider as `inactive=1`.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { apiJson, putAgentConfig, AgentConfigError } from "../api/client";
import { useToast } from "../hooks/useToast";
import type {
  AgentClaudeConfig,
  AgentCodexConfig,
  AgentConfig,
  AgentConfigPatch,
  AgentCursorConfig,
  AgentOpenCodeConfig,
  AgentProviderId,
  ConfigResponse,
} from "../api/types";

interface Props {
  onLogout: () => void;
  authEnabled: boolean;
  isAdmin: boolean;
}

/** Providers wired in the v1 dashboard. `gemini` is a v2 placeholder. */
const V1_PROVIDERS: AgentProviderId[] = ["claude", "cursor", "codex", "opencode"];

const PROVIDER_LABEL: Record<AgentProviderId, string> = {
  claude: "Claude",
  cursor: "Cursor",
  codex: "Codex",
  opencode: "OpenCode",
  gemini: "Gemini (v2)",
  none: "None",
};

/** Phase 4 lands the actual adapter for these — warn the admin until then. */
const PHASE_4_PROVIDERS: ReadonlySet<AgentProviderId> = new Set<AgentProviderId>([
  "codex",
  "opencode",
]);

/**
 * Per-provider draft state. We carry every field so the form can render the
 * union (cursor.cli, codex.provider_name) without juggling discriminants in
 * each <input>.
 */
export interface ProviderDraft {
  model: string;
  base_url: string;
  /** Cursor-only (the CLI binary name; default "agent"). */
  cli: string;
  /** Codex-only (named entry in ~/.codex/config.toml). */
  provider_name: string;
  /** One arg per line — converted to `string[]` on save. */
  extra_args_text: string;
  allow_shared_default: boolean;
}

const EMPTY_DRAFT: ProviderDraft = {
  model: "",
  base_url: "",
  cli: "agent",
  provider_name: "",
  extra_args_text: "",
  allow_shared_default: false,
};

function draftFromConfig(
  provider: AgentProviderId,
  cfg: AgentConfig | undefined,
): ProviderDraft {
  const sub = cfg?.providers?.[provider as keyof typeof cfg.providers];
  if (!sub) return { ...EMPTY_DRAFT };
  const base: ProviderDraft = {
    ...EMPTY_DRAFT,
    model: sub.model ?? "",
    extra_args_text: (sub.extra_args ?? []).join("\n"),
    allow_shared_default: sub.allow_shared_default ?? false,
  };
  if (provider === "claude") base.base_url = (sub as AgentClaudeConfig).base_url ?? "";
  if (provider === "cursor") base.cli = (sub as AgentCursorConfig).cli ?? "agent";
  if (provider === "codex") {
    base.base_url = (sub as AgentCodexConfig).base_url ?? "";
    base.provider_name = (sub as AgentCodexConfig).provider_name ?? "";
  }
  if (provider === "opencode") {
    base.base_url = (sub as AgentOpenCodeConfig).base_url ?? "";
  }
  return base;
}

/** Build the API patch from the user's draft for a single provider. */
function patchFromDraft(
  provider: AgentProviderId,
  draft: ProviderDraft,
): AgentConfigPatch["providers"] {
  const extraArgs = draft.extra_args_text
    .split("\n")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  const common = {
    model: draft.model,
    extra_args: extraArgs,
    allow_shared_default: draft.allow_shared_default,
  };
  switch (provider) {
    case "claude":
      return { claude: { ...common, base_url: draft.base_url } };
    case "cursor":
      return { cursor: { ...common, cli: draft.cli } };
    case "codex":
      return {
        codex: { ...common, base_url: draft.base_url, provider_name: draft.provider_name },
      };
    case "opencode":
      return { opencode: { ...common, base_url: draft.base_url } };
    default:
      return undefined;
  }
}

export function AdminAiSettings({ onLogout, authEnabled, isAdmin }: Props) {
  const { showToast } = useToast();
  const [config, setConfig] = useState<ConfigResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  // Form state — controlled inputs only; reset from `config` on every load.
  const [selectedProvider, setSelectedProvider] = useState<AgentProviderId>("claude");
  const [draft, setDraft] = useState<ProviderDraft>(EMPTY_DRAFT);
  const [availableProviders, setAvailableProviders] = useState<AgentProviderId[]>([]);
  // Provider-switch confirm modal (05_ux_design.md §2.6).
  const [pendingProviderSwitch, setPendingProviderSwitch] = useState<{
    from: AgentProviderId;
    to: AgentProviderId;
  } | null>(null);

  const refresh = useCallback(() => {
    setLoading(true);
    setError("");
    apiJson<ConfigResponse>("/api/config")
      .then((c) => {
        setConfig(c);
        const agent = (c.agent ?? {}) as AgentConfig;
        const provider: AgentProviderId = (agent.provider ?? "claude") as AgentProviderId;
        setSelectedProvider(provider);
        setDraft(draftFromConfig(provider, agent));
        setAvailableProviders(
          Array.isArray(agent.available_providers) && agent.available_providers.length > 0
            ? (agent.available_providers as AgentProviderId[])
            : V1_PROVIDERS,
        );
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  /**
   * The server may return a config with a different (legacy) sub-table layout
   * than the one we're editing. We derive the "currently saved" provider so
   * we can detect "provider switch" on save and trigger the confirm modal.
   */
  const savedProvider = useMemo<AgentProviderId>(
    () => ((config?.agent?.provider as AgentProviderId) ?? "claude"),
    [config],
  );

  const handleSelectProvider = useCallback(
    (next: AgentProviderId) => {
      setSelectedProvider(next);
      setDraft(draftFromConfig(next, (config?.agent ?? {}) as AgentConfig));
    },
    [config],
  );

  const toggleAvailable = useCallback((p: AgentProviderId) => {
    setAvailableProviders((prev) =>
      prev.includes(p) ? prev.filter((x) => x !== p) : [...prev, p],
    );
  }, []);

  const buildPatch = useCallback((): AgentConfigPatch => {
    const patch: AgentConfigPatch = {
      provider: selectedProvider,
      available_providers: availableProviders,
      providers: patchFromDraft(selectedProvider, draft),
    };
    return patch;
  }, [selectedProvider, availableProviders, draft]);

  const persist = useCallback(
    async (patch: AgentConfigPatch) => {
      setSaving(true);
      try {
        const updated = await putAgentConfig(patch);
        setConfig(updated);
        showToast("AI provider settings saved.", "success");
      } catch (e: unknown) {
        if (e instanceof AgentConfigError) {
          // Surface the structured code so QA / admins can correlate with the
          // server's denied-flag list etc. (05_ux_design.md §4.5).
          showToast(`${e.message} (code: ${e.code})`, "error");
        } else {
          showToast(e instanceof Error ? e.message : String(e), "error");
        }
      } finally {
        setSaving(false);
      }
    },
    [showToast],
  );

  const handleSave = useCallback(() => {
    if (selectedProvider !== savedProvider) {
      setPendingProviderSwitch({ from: savedProvider, to: selectedProvider });
      return;
    }
    void persist(buildPatch());
  }, [selectedProvider, savedProvider, persist, buildPatch]);

  const handleConfirmSwitch = useCallback(() => {
    setPendingProviderSwitch(null);
    void persist(buildPatch());
  }, [persist, buildPatch]);

  if (!isAdmin) {
    return (
      <div className="min-h-screen flex items-center justify-center">
        <p className="text-sm text-gray-400">
          Admin only — this page is hidden for non-admin users.
        </p>
      </div>
    );
  }

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
            <span className="text-lg font-bold text-white">AI Provider Settings</span>
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

      <main className="max-w-3xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
        {loading && <p className="text-sm text-gray-500">Loading…</p>}
        {!loading && error && (
          <p className="text-sm text-red-400">Could not load config: {error}</p>
        )}
        {!loading && !error && (
          <ProviderForm
            selectedProvider={selectedProvider}
            onSelectProvider={handleSelectProvider}
            draft={draft}
            onDraftChange={setDraft}
            availableProviders={availableProviders}
            onToggleAvailable={toggleAvailable}
            onSave={handleSave}
            saving={saving}
          />
        )}
      </main>

      {pendingProviderSwitch && (
        <ProviderSwitchConfirm
          from={pendingProviderSwitch.from}
          to={pendingProviderSwitch.to}
          onCancel={() => setPendingProviderSwitch(null)}
          onConfirm={handleConfirmSwitch}
        />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Provider form (extracted to keep AdminAiSettings small).
// ---------------------------------------------------------------------------

interface ProviderFormProps {
  selectedProvider: AgentProviderId;
  onSelectProvider: (p: AgentProviderId) => void;
  draft: ProviderDraft;
  onDraftChange: (d: ProviderDraft) => void;
  availableProviders: AgentProviderId[];
  onToggleAvailable: (p: AgentProviderId) => void;
  onSave: () => void;
  saving: boolean;
}

export function ProviderForm({
  selectedProvider,
  onSelectProvider,
  draft,
  onDraftChange,
  availableProviders,
  onToggleAvailable,
  onSave,
  saving,
}: ProviderFormProps) {
  const cursorBaseUrlDisabled = selectedProvider === "cursor";
  const phase4Warning = PHASE_4_PROVIDERS.has(selectedProvider);

  // Tiny helper to avoid spreading the same `onDraftChange({ ...draft, k })`
  // boilerplate at every <input>.
  const update = (patch: Partial<ProviderDraft>) =>
    onDraftChange({ ...draft, ...patch });

  return (
    <div className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-6">
      {/* Provider dropdown */}
      <section className="flex flex-col gap-2">
        <label htmlFor="provider-select" className="text-xs text-gray-400">
          Provider
        </label>
        <select
          id="provider-select"
          value={selectedProvider}
          onChange={(e) => onSelectProvider(e.target.value as AgentProviderId)}
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
        >
          {V1_PROVIDERS.map((p) => (
            <option key={p} value={p}>
              {PROVIDER_LABEL[p]}
            </option>
          ))}
        </select>
        {phase4Warning && (
          <p className="text-xs text-amber-300/90">
            Provider implementation lands in Phase 4 — saving the config is
            allowed, but workflows won't start until the adapter ships.
          </p>
        )}
      </section>

      {/* Model */}
      <section className="flex flex-col gap-2">
        <label htmlFor="model-input" className="text-xs text-gray-400">
          Model
        </label>
        <input
          id="model-input"
          type="text"
          value={draft.model}
          onChange={(e) => update({ model: e.target.value })}
          placeholder="Leave empty for the vendor default"
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
      </section>

      {/* Cursor CLI binary */}
      {selectedProvider === "cursor" && (
        <section className="flex flex-col gap-2">
          <label htmlFor="cli-input" className="text-xs text-gray-400">
            CLI binary
          </label>
          <input
            id="cli-input"
            type="text"
            value={draft.cli}
            onChange={(e) => update({ cli: e.target.value })}
            placeholder="agent"
            className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
          />
        </section>
      )}

      {/* Codex provider_name */}
      {selectedProvider === "codex" && (
        <section className="flex flex-col gap-2">
          <label htmlFor="provider-name-input" className="text-xs text-gray-400">
            Provider name (entry in ~/.codex/config.toml)
          </label>
          <input
            id="provider-name-input"
            type="text"
            value={draft.provider_name}
            onChange={(e) => update({ provider_name: e.target.value })}
            placeholder="e.g. openai"
            className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
          />
        </section>
      )}

      {/* Base URL */}
      <section className="flex flex-col gap-2">
        <label htmlFor="base-url-input" className="text-xs text-gray-400">
          Base URL
        </label>
        <input
          id="base-url-input"
          type="text"
          value={cursorBaseUrlDisabled ? "" : draft.base_url}
          onChange={(e) => update({ base_url: e.target.value })}
          placeholder="Leave empty to use the vendor public API"
          disabled={cursorBaseUrlDisabled}
          title={
            cursorBaseUrlDisabled
              ? "Cursor CLI does not support custom upstream endpoints"
              : undefined
          }
          className={`bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm font-mono ${
            cursorBaseUrlDisabled
              ? "text-gray-600 cursor-not-allowed"
              : "text-gray-200"
          }`}
        />
        {cursorBaseUrlDisabled && (
          <p className="text-xs text-gray-500">
            Cursor CLI does not support custom upstream endpoints.
          </p>
        )}
        {selectedProvider === "opencode" && (
          <p className="text-xs text-gray-500">
            LM Studio recipe: set base URL to{" "}
            <code className="text-gray-400">http://lm-studio:1234/v1</code>{" "}
            and model to <code className="text-gray-400">lmstudio/&lt;model-id&gt;</code>.
          </p>
        )}
      </section>

      {/* Extra args */}
      <section className="flex flex-col gap-2">
        <label htmlFor="extra-args-input" className="text-xs text-gray-400">
          Extra args (one per line)
        </label>
        <textarea
          id="extra-args-input"
          value={draft.extra_args_text}
          onChange={(e) => update({ extra_args_text: e.target.value })}
          placeholder="--max-turns&#10;50"
          rows={4}
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        <p className="text-xs text-gray-500">
          Maestro-owned flags (e.g.{" "}
          <code className="text-gray-400">--dangerously-skip-permissions</code>,{" "}
          <code className="text-gray-400">--resume</code>) are rejected
          server-side.
        </p>
      </section>

      {/* Shared default toggle */}
      <section className="flex items-start gap-2">
        <input
          id="shared-default-input"
          type="checkbox"
          checked={draft.allow_shared_default}
          onChange={(e) => update({ allow_shared_default: e.target.checked })}
          className="mt-0.5 accent-blue-500"
        />
        <label htmlFor="shared-default-input" className="text-xs text-gray-300">
          Allow shared default token
          <p className="text-gray-500 mt-0.5">
            When on, users without their own credential fall back to the
            deployment-default token configured in{" "}
            <code className="text-gray-400">maestro.env</code>. Default off.
          </p>
        </label>
      </section>

      {/* Available providers whitelist */}
      <section className="flex flex-col gap-2">
        <p className="text-xs text-gray-400">Available providers for users</p>
        <div className="flex flex-wrap gap-3">
          {V1_PROVIDERS.map((p) => {
            const inputId = `available-${p}`;
            return (
              <label
                key={p}
                htmlFor={inputId}
                className="flex items-center gap-2 text-xs text-gray-300"
              >
                <input
                  id={inputId}
                  type="checkbox"
                  checked={availableProviders.includes(p)}
                  onChange={() => onToggleAvailable(p)}
                  className="accent-blue-500"
                />
                {PROVIDER_LABEL[p]}
              </label>
            );
          })}
        </div>
      </section>

      {/* Save */}
      <div className="flex justify-end">
        <button
          type="button"
          disabled={saving}
          onClick={onSave}
          className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
        >
          {saving ? "Saving…" : "Save changes"}
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Provider-switch confirm modal (05_ux_design.md §2.6).
// ---------------------------------------------------------------------------

interface SwitchProps {
  from: AgentProviderId;
  to: AgentProviderId;
  onCancel: () => void;
  onConfirm: () => void;
}

export function ProviderSwitchConfirm({ from, to, onCancel, onConfirm }: SwitchProps) {
  const [typed, setTyped] = useState("");
  const canConfirm = typed.trim().toUpperCase() === "SWITCH";
  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        className="bg-gray-900 border border-amber-700/50 rounded-xl p-6 max-w-md w-full mx-4"
        onClick={(e) => e.stopPropagation()}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="provider-switch-title"
        aria-describedby="provider-switch-body"
      >
        <h3
          id="provider-switch-title"
          className="text-lg font-medium text-amber-300 mb-2"
        >
          Switch AI provider?
        </h3>
        <div id="provider-switch-body" className="text-sm text-gray-300 mb-4">
          <p>
            You&rsquo;re switching from{" "}
            <strong>{PROVIDER_LABEL[from] ?? from}</strong> to{" "}
            <strong>{PROVIDER_LABEL[to] ?? to}</strong>.
          </p>
          <p className="mt-2 text-gray-400">
            Per-user credentials for {PROVIDER_LABEL[from] ?? from} will be
            deactivated. Each user must connect their{" "}
            {PROVIDER_LABEL[to] ?? to} account before they can run new
            workflows. Workflows already running will finish on{" "}
            {PROVIDER_LABEL[from] ?? from}.
          </p>
          <p className="mt-2 text-xs text-gray-500">
            Per-user credentials will be migrated in a later phase.
          </p>
        </div>
        <label
          htmlFor="provider-switch-confirm"
          className="block text-xs text-gray-400 mb-1"
        >
          Type <code className="text-amber-300">SWITCH</code> to confirm
        </label>
        <input
          id="provider-switch-confirm"
          type="text"
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          autoFocus
          className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono mb-4"
        />
        <div className="flex justify-end gap-3">
          <button
            onClick={onCancel}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
          >
            Cancel
          </button>
          <button
            onClick={onConfirm}
            disabled={!canConfirm}
            className="text-sm px-4 py-2 rounded-lg bg-amber-600 text-white hover:bg-amber-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
          >
            Switch provider
          </button>
        </div>
      </div>
    </div>
  );
}

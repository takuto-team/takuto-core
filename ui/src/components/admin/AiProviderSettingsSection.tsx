// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Admin-only AI provider settings section.
 *
 * Lives inside the consolidated "AI Settings" tab on /config.html. The parent
 * tab (`AiSettingsTab`) decides whether to render this section based on the
 * caller's role; this file does NOT re-check `isAdmin` because the parent
 * gate is the single source of truth. Server-side enforcement at
 * `PUT /api/config/agent` is the real security boundary — this UI gate is
 * cosmetic.
 *
 * Source of truth: tmp/multi-agents/04_architecture.md §2 and
 * tmp/multi-agents/05_ux_design.md §2.5 / §2.6.
 *
 * What this section is NOT:
 *   - Not a per-user credential surface (that's `MyCredentialsSection`).
 *   - Not a provider "Test" button (deferred to a later phase).
 *
 * What it IS: a single PUT /api/config/agent panel that lets an admin pick
 * the active provider, edit its sub-table (base_url, model, extras,
 * allow_shared_default), and toggle the user-facing `available_providers`
 * whitelist. Switching the active provider triggers a confirm modal
 * (05_ux_design.md §2.6) because the switch marks every per-user credential
 * for the previous provider as `inactive=1`.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { apiJson, putAgentConfig, AgentConfigError } from "../../api/client";
import { useToast } from "../../hooks/useToast";
import type {
  AgentConfig,
  AgentConfigPatch,
  AgentProviderId,
  ConfigResponse,
} from "../../api/types";
import {
  EMPTY_DRAFT,
  PROVIDER_LABEL,
  ProviderForm,
  V1_PROVIDERS,
  type ProviderDraft,
} from "./ProviderForm";
import { ProviderSwitchConfirm } from "./ProviderSwitchConfirm";

export { ProviderSwitchConfirm };
export { PROVIDER_LABEL };

function draftFromConfig(
  provider: AgentProviderId,
  cfg: AgentConfig | undefined,
): ProviderDraft {
  // Discriminated-union narrowing via the provider key on `cfg.providers`. The
  // table indexes each provider with its own typed sub-table (AgentClaudeConfig
  // etc.), so accessing `providers.claude` gives the exact shape — no casts.
  const providers = cfg?.providers;
  if (!providers) return { ...EMPTY_DRAFT };
  switch (provider) {
    case "claude": {
      const sub = providers.claude;
      if (!sub) return { ...EMPTY_DRAFT };
      return {
        ...EMPTY_DRAFT,
        model: sub.model ?? "",
        extra_args_text: (sub.extra_args ?? []).join("\n"),
        allow_shared_default: sub.allow_shared_default ?? false,
        base_url: sub.base_url ?? "",
      };
    }
    case "cursor": {
      const sub = providers.cursor;
      if (!sub) return { ...EMPTY_DRAFT };
      return {
        ...EMPTY_DRAFT,
        model: sub.model ?? "",
        extra_args_text: (sub.extra_args ?? []).join("\n"),
        allow_shared_default: sub.allow_shared_default ?? false,
        cli: sub.cli ?? "agent",
      };
    }
    case "codex": {
      const sub = providers.codex;
      if (!sub) return { ...EMPTY_DRAFT };
      return {
        ...EMPTY_DRAFT,
        model: sub.model ?? "",
        extra_args_text: (sub.extra_args ?? []).join("\n"),
        allow_shared_default: sub.allow_shared_default ?? false,
        base_url: sub.base_url ?? "",
        provider_name: sub.provider_name ?? "",
      };
    }
    case "opencode": {
      const sub = providers.opencode;
      if (!sub) return { ...EMPTY_DRAFT };
      return {
        ...EMPTY_DRAFT,
        model: sub.model ?? "",
        extra_args_text: (sub.extra_args ?? []).join("\n"),
        allow_shared_default: sub.allow_shared_default ?? false,
        base_url: sub.base_url ?? "",
      };
    }
    case "gemini": {
      const sub = providers.gemini;
      if (!sub) return { ...EMPTY_DRAFT };
      return {
        ...EMPTY_DRAFT,
        model: sub.model ?? "",
        extra_args_text: (sub.extra_args ?? []).join("\n"),
        allow_shared_default: sub.allow_shared_default ?? false,
        base_url: sub.base_url ?? "",
      };
    }
    case "none":
      return { ...EMPTY_DRAFT };
  }
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

export function AiProviderSettingsSection() {
  const { showToast } = useToast();
  const [config, setConfig] = useState<ConfigResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  // Form state — controlled inputs only; reset from `config` on every load.
  const [selectedProvider, setSelectedProvider] = useState<AgentProviderId>("claude");
  const [draft, setDraft] = useState<ProviderDraft>(EMPTY_DRAFT);
  const [availableProviders, setAvailableProviders] = useState<AgentProviderId[]>([]);
  const [shareConversation, setShareConversation] = useState(false);
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
        setShareConversation(agent.share_conversation_across_steps === true);
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
      share_conversation_across_steps: shareConversation,
      providers: patchFromDraft(selectedProvider, draft),
    };
    return patch;
  }, [selectedProvider, availableProviders, shareConversation, draft]);

  const persist = useCallback(
    async (patch: AgentConfigPatch) => {
      setSaving(true);
      try {
        const updated = await putAgentConfig(patch);
        setConfig(updated);
        // Backend returns `persisted: false` + `persist_warning: "<error>"`
        // when the in-memory patch succeeded but the on-disk write failed
        // (e.g. read-only mount, EACCES). Strict `=== false` so a legacy
        // server that doesn't return the field (undefined) is treated as
        // "assume OK". See `routes/config_agent.rs::PutAgentConfigResponse`.
        if (updated.persisted === false) {
          const reason = updated.persist_warning ?? "unknown error";
          showToast(
            `AI provider settings applied in memory but NOT persisted to disk: ${reason}. The change will be lost on next restart — fix the config volume and save again.`,
            "error",
          );
        } else {
          showToast("AI provider settings saved.", "success");
        }
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

  return (
    <section
      aria-labelledby="ai-provider-section-title"
      className="flex flex-col gap-3"
    >
      <h2
        id="ai-provider-section-title"
        className="text-lg font-semibold text-white"
      >
        Provider settings
      </h2>
      <p className="text-xs text-gray-500">
        Admin-only. Pick the active AI provider, configure its sub-table, and
        choose which providers users can pick from.
      </p>

      {loading && <p className="text-sm text-gray-500">Loading…</p>}
      {!loading && error && (
        <p className="text-sm text-red-400">Could not load config: {error}</p>
      )}
      {!loading && !error && (
        <>
          <section className="flex items-start gap-2 mb-4 pb-4 border-b border-gray-800">
            <input
              id="share-conversation-input"
              type="checkbox"
              checked={shareConversation}
              onChange={(e) => setShareConversation(e.target.checked)}
              className="mt-0.5 accent-blue-500"
            />
            <label htmlFor="share-conversation-input" className="text-xs text-gray-300">
              Share one conversation across a flow's steps
              <p className="text-gray-500 mt-0.5">
                When on, each step resumes the previous step's session, so the agent
                carries full context forward (it remembers what it implemented when
                it reviews). When off (default), every step runs in a fresh session
                with no memory of earlier steps — safer for smaller local models.
                Applies to all providers. Save with the button below.
              </p>
            </label>
          </section>
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
        </>
      )}

      {pendingProviderSwitch && (
        <ProviderSwitchConfirm
          from={pendingProviderSwitch.from}
          to={pendingProviderSwitch.to}
          onCancel={() => setPendingProviderSwitch(null)}
          onConfirm={handleConfirmSwitch}
        />
      )}
    </section>
  );
}

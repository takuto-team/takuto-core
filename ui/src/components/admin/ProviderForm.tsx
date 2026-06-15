// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Provider configuration form used by `AiProviderSettingsSection`. Extracted
 * into its own file so the section shell can stay focused on the admin
 * save / confirm-switch flow (CODING_STANDARDS §3 one component per file).
 */

import type { AgentProviderId } from "../../api/types";

/** Providers wired in the v1 dashboard. `gemini` is a v2 placeholder. */
const V1_PROVIDERS: AgentProviderId[] = ["claude", "cursor", "codex", "opencode"];

export const PROVIDER_LABEL: Record<AgentProviderId, string> = {
  claude: "Claude",
  cursor: "Cursor",
  codex: "Codex",
  opencode: "OpenCode",
  gemini: "Gemini (v2)",
  none: "None",
};

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
  /** OpenCode-only: max context window (tokens). Empty = let OpenCode choose. */
  context_limit: string;
  /** OpenCode-only: max output (tokens) per response. Empty = let OpenCode choose. */
  output_limit: string;
}

export const EMPTY_DRAFT: ProviderDraft = {
  model: "",
  base_url: "",
  cli: "agent",
  provider_name: "",
  extra_args_text: "",
  allow_shared_default: false,
  context_limit: "",
  output_limit: "",
};

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

  // OpenCode self-hosted spec (lore/audits/2026-05-27-opencode-self-hosted-spec.md
  // §2.4): OpenCode requires non-empty base_url + model. The validator
  // returns 400 with code `opencode_base_url_required` / `opencode_model_required`
  // on submit; this client-side guard short-circuits before the bounce so
  // the operator sees the requirement up front.
  const opencodeMissingBaseUrl =
    selectedProvider === "opencode" && draft.base_url.trim() === "";
  const opencodeMissingModel =
    selectedProvider === "opencode" && draft.model.trim() === "";
  const saveDisabled = saving || opencodeMissingBaseUrl || opencodeMissingModel;

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
      </section>

      {/* Model */}
      <section className="flex flex-col gap-2">
        <label htmlFor="model-input" className="text-xs text-gray-400">
          Model
          {selectedProvider === "opencode" && (
            <span className="text-red-400 ml-1">*</span>
          )}
        </label>
        <input
          id="model-input"
          type="text"
          value={draft.model}
          onChange={(e) => update({ model: e.target.value })}
          placeholder={
            selectedProvider === "opencode"
              ? "lmstudio/qwen3-coder"
              : "Leave empty for the vendor default"
          }
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        {selectedProvider === "opencode" && (
          <p className="text-xs text-gray-500">
            Required. The model id served by your self-hosted endpoint (e.g.{" "}
            <code className="text-gray-400">lmstudio/qwen3-coder</code> or{" "}
            <code className="text-gray-400">ollama/llama3</code>).
          </p>
        )}
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
          {selectedProvider === "opencode" && (
            <span className="text-red-400 ml-1">*</span>
          )}
        </label>
        <input
          id="base-url-input"
          type="text"
          value={cursorBaseUrlDisabled ? "" : draft.base_url}
          onChange={(e) => update({ base_url: e.target.value })}
          placeholder={
            selectedProvider === "opencode"
              ? "http://lm-studio:1234/v1"
              : "Leave empty to use the vendor public API"
          }
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
            Required. OpenCode is the self-hosted adapter — point this at
            your OpenAI-compatible model server. Examples:{" "}
            <code className="text-gray-400">http://lm-studio:1234/v1</code>{" "}
            (LM Studio),{" "}
            <code className="text-gray-400">http://ollama:11434/v1</code>{" "}
            (Ollama). To use Anthropic / OpenAI directly, pick the Claude
            or Codex provider instead.
          </p>
        )}
      </section>

      {/* OpenCode-only token limits (self-hosted models carry no models.dev
          metadata, so OpenCode can't auto-discover the window). */}
      {selectedProvider === "opencode" && (
        <section className="flex flex-col gap-2">
          <p className="text-xs text-gray-400">Token limits (optional)</p>
          <div className="flex gap-4">
            <div className="flex flex-col gap-1 flex-1">
              <label htmlFor="context-limit-input" className="text-xs text-gray-500">
                Context window
              </label>
              <input
                id="context-limit-input"
                type="number"
                min={1}
                value={draft.context_limit}
                onChange={(e) => update({ context_limit: e.target.value })}
                placeholder="32768"
                className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
              />
            </div>
            <div className="flex flex-col gap-1 flex-1">
              <label htmlFor="output-limit-input" className="text-xs text-gray-500">
                Max output
              </label>
              <input
                id="output-limit-input"
                type="number"
                min={1}
                value={draft.output_limit}
                onChange={(e) => update({ output_limit: e.target.value })}
                placeholder="8192"
                className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
              />
            </div>
          </div>
          <p className="text-xs text-gray-500">
            Tokens. Tells OpenCode the window of your self-hosted model so it
            tracks remaining context (it can't look this up for a local
            endpoint). Leave blank to let OpenCode choose. Match your server's
            loaded context length (e.g. LM Studio's per-model setting).
          </p>
        </section>
      )}

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
          Takuto-owned flags (e.g.{" "}
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
            <code className="text-gray-400">takuto.env</code>. Default off.
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
      <div className="flex flex-col items-end gap-2">
        {(opencodeMissingBaseUrl || opencodeMissingModel) && (
          <p className="text-xs text-red-400">
            OpenCode requires both a Base URL and a Model to save.
          </p>
        )}
        <button
          type="button"
          disabled={saveDisabled}
          onClick={onSave}
          className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
        >
          {saving ? "Saving…" : "Save changes"}
        </button>
      </div>
    </div>
  );
}

export { V1_PROVIDERS };

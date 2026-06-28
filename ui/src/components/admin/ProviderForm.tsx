// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Provider configuration form used by `AiProviderSettingsSection`. Extracted
 * into its own file so the section shell can stay focused on the admin
 * save / confirm-switch flow (CODING_STANDARDS §3 one component per file).
 */

import { Trans, useTranslation } from "react-i18next";
import type { AgentProviderId } from "../../api/types";

/** Providers wired in the v1 dashboard, in picker display order (most-tested
 *  first). `gemini` is a v2 placeholder. */
const V1_PROVIDERS: AgentProviderId[] = ["cursor", "opencode", "claude", "codex"];

/** Providers flagged as not fully tested in the picker — annotated in the
 *  dropdown so operators pick them knowingly. */
const NOT_FULLY_TESTED = new Set<AgentProviderId>(["claude", "codex"]);

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
  /** Cursor-only: Privacy Mode (ghost mode). Default on. */
  privacy_mode: boolean;
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
  privacy_mode: true,
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
}

export function ProviderForm({
  selectedProvider,
  onSelectProvider,
  draft,
  onDraftChange,
  availableProviders,
  onToggleAvailable,
}: ProviderFormProps) {
  const { t } = useTranslation("config");
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

  // Tiny helper to avoid spreading the same `onDraftChange({ ...draft, k })`
  // boilerplate at every <input>.
  const update = (patch: Partial<ProviderDraft>) =>
    onDraftChange({ ...draft, ...patch });

  return (
    <div className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-6">
      {/* Provider dropdown */}
      <section className="flex flex-col gap-2">
        <label htmlFor="provider-select" className="text-xs text-gray-400">
          {t("ai.form.provider")}
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
              {NOT_FULLY_TESTED.has(p) ? ` ${t("ai.form.notFullyTested")}` : ""}
            </option>
          ))}
        </select>
      </section>

      {/* Model */}
      <section className="flex flex-col gap-2">
        <label htmlFor="model-input" className="text-xs text-gray-400">
          {t("ai.form.model")}
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
              : t("ai.form.modelDefaultPlaceholder")
          }
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        {selectedProvider === "opencode" && (
          <p className="text-xs text-gray-500">
            <Trans
              i18nKey="ai.form.opencodeModelHelp"
              ns="config"
              components={{ code: <code className="text-gray-400" /> }}
            />
          </p>
        )}
      </section>

      {/* Cursor CLI binary */}
      {selectedProvider === "cursor" && (
        <section className="flex flex-col gap-2">
          <label htmlFor="cli-input" className="text-xs text-gray-400">
            {t("ai.form.cliBinary")}
          </label>
          <input
            id="cli-input"
            type="text"
            value={draft.cli}
            onChange={(e) => update({ cli: e.target.value })}
            placeholder="agent"
            className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
          />

          <div className="flex items-start justify-between gap-4 mt-2">
            <div className="min-w-0">
              <span className="text-xs text-gray-400">{t("ai.form.privacyMode")}</span>
              <p className="text-xs text-gray-500 mt-1">
                {t("ai.form.privacyModeHelp")}
              </p>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={draft.privacy_mode}
              aria-label={t("ai.form.privacyModeAria")}
              onClick={() => update({ privacy_mode: !draft.privacy_mode })}
              className={`relative inline-flex h-7 w-12 flex-shrink-0 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500/50 cursor-pointer ${
                draft.privacy_mode ? "bg-blue-600" : "bg-gray-700"
              }`}
            >
              <span
                className={`inline-block h-5 w-5 transform rounded-full bg-white transition-transform ${
                  draft.privacy_mode ? "translate-x-6" : "translate-x-1"
                }`}
              />
            </button>
          </div>
        </section>
      )}

      {/* Codex provider_name */}
      {selectedProvider === "codex" && (
        <section className="flex flex-col gap-2">
          <label htmlFor="provider-name-input" className="text-xs text-gray-400">
            {t("ai.form.providerName")}
          </label>
          <input
            id="provider-name-input"
            type="text"
            value={draft.provider_name}
            onChange={(e) => update({ provider_name: e.target.value })}
            placeholder={t("ai.form.providerNamePlaceholder")}
            className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
          />
        </section>
      )}

      {/* Base URL */}
      <section className="flex flex-col gap-2">
        <label htmlFor="base-url-input" className="text-xs text-gray-400">
          {t("ai.form.baseUrl")}
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
              : t("ai.form.baseUrlDefaultPlaceholder")
          }
          disabled={cursorBaseUrlDisabled}
          title={
            cursorBaseUrlDisabled
              ? t("ai.form.cursorNoBaseUrl")
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
            {t("ai.form.cursorNoBaseUrl")}
          </p>
        )}
        {selectedProvider === "opencode" && (
          <p className="text-xs text-gray-500">
            <Trans
              i18nKey="ai.form.opencodeBaseUrlHelp"
              ns="config"
              components={{ code: <code className="text-gray-400" /> }}
            />
          </p>
        )}
      </section>

      {/* OpenCode-only token limits (self-hosted models carry no models.dev
          metadata, so OpenCode can't auto-discover the window). */}
      {selectedProvider === "opencode" && (
        <section className="flex flex-col gap-2">
          <p className="text-xs text-gray-400">{t("ai.form.tokenLimits")}</p>
          <div className="flex gap-4">
            <div className="flex flex-col gap-1 flex-1">
              <label htmlFor="context-limit-input" className="text-xs text-gray-500">
                {t("ai.form.contextWindow")}
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
                {t("ai.form.maxOutput")}
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
            {t("ai.form.tokenLimitsHelp")}
          </p>
        </section>
      )}

      {/* Extra args */}
      <section className="flex flex-col gap-2">
        <label htmlFor="extra-args-input" className="text-xs text-gray-400">
          {t("ai.form.extraArgs")}
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
          <Trans
            i18nKey="ai.form.extraArgsHelp"
            ns="config"
            components={{ code: <code className="text-gray-400" /> }}
          />
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
          {t("ai.form.allowSharedDefault")}
          <p className="text-gray-500 mt-0.5">
            <Trans
              i18nKey="ai.form.allowSharedDefaultHelp"
              ns="config"
              components={{ code: <code className="text-gray-400" /> }}
            />
          </p>
        </label>
      </section>

      {/* Available providers whitelist */}
      <section className="flex flex-col gap-2">
        <p className="text-xs text-gray-400">{t("ai.form.availableProviders")}</p>
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

      {(opencodeMissingBaseUrl || opencodeMissingModel) && (
        <p className="text-xs text-red-400 text-right">
          {t("ai.form.opencodeRequires")}
        </p>
      )}
    </div>
  );
}

export { V1_PROVIDERS };

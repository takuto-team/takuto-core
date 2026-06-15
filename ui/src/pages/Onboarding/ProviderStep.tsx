// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { AgentProviderId } from "../../api/types";

const V1_PROVIDERS: AgentProviderId[] = ["claude", "cursor", "codex", "opencode"];

const PROVIDER_LABEL: Record<AgentProviderId, string> = {
  claude: "Claude",
  cursor: "Cursor",
  codex: "Codex",
  opencode: "OpenCode (self-hosted)",
  gemini: "Gemini (v2)",
  none: "None",
};

interface Props {
  provider: AgentProviderId;
  onChangeProvider: (p: AgentProviderId) => void;
  baseUrl: string;
  onChangeBaseUrl: (v: string) => void;
  model: string;
  onChangeModel: (v: string) => void;
  extraArgsText: string;
  onChangeExtraArgs: (v: string) => void;
}

export function ProviderStep({
  provider,
  onChangeProvider,
  baseUrl,
  onChangeBaseUrl,
  model,
  onChangeModel,
  extraArgsText,
  onChangeExtraArgs,
}: Props) {
  const cursorBaseUrlDisabled = provider === "cursor";
  return (
    <div className="flex flex-col gap-4">
      <div>
        <label htmlFor="onb-provider" className="block text-xs text-gray-400 mb-1">
          Provider
        </label>
        <select
          id="onb-provider"
          value={provider}
          onChange={(e) => onChangeProvider(e.target.value as AgentProviderId)}
          className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
        >
          {V1_PROVIDERS.map((p) => (
            <option key={p} value={p}>
              {PROVIDER_LABEL[p]}
            </option>
          ))}
        </select>
        {provider === "opencode" && (
          <p className="text-xs text-gray-500 mt-1">
            OpenCode wires Takuto to your self-hosted OpenAI-compatible
            endpoint (LM Studio, Ollama, vLLM, private gateways). To use
            Anthropic / OpenAI directly, pick the Claude or Codex
            provider instead.
          </p>
        )}
      </div>

      <div>
        <label htmlFor="onb-base-url" className="block text-xs text-gray-400 mb-1">
          Base URL
          {provider === "opencode" && (
            <span className="text-red-400 ml-1">*</span>
          )}
        </label>
        <input
          id="onb-base-url"
          type="text"
          value={cursorBaseUrlDisabled ? "" : baseUrl}
          onChange={(e) => onChangeBaseUrl(e.target.value)}
          placeholder={
            provider === "opencode"
              ? "http://lm-studio:1234/v1"
              : "Leave empty to use the vendor public API"
          }
          disabled={cursorBaseUrlDisabled}
          title={
            cursorBaseUrlDisabled
              ? "Cursor CLI does not support custom upstream endpoints"
              : undefined
          }
          className={`w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm font-mono ${
            cursorBaseUrlDisabled ? "text-gray-600 cursor-not-allowed" : "text-gray-200"
          }`}
        />
        {cursorBaseUrlDisabled && (
          <p className="text-xs text-gray-500 mt-1">
            Cursor CLI does not support custom upstream endpoints.
          </p>
        )}
        {provider === "opencode" && (
          <p className="text-xs text-gray-500 mt-1">
            Required for OpenCode. Point this at your self-hosted
            OpenAI-compatible server.
          </p>
        )}
      </div>

      <div>
        <label htmlFor="onb-model" className="block text-xs text-gray-400 mb-1">
          Model
          {provider === "opencode" && (
            <span className="text-red-400 ml-1">*</span>
          )}
        </label>
        <input
          id="onb-model"
          type="text"
          value={model}
          onChange={(e) => onChangeModel(e.target.value)}
          placeholder={
            provider === "opencode"
              ? "lmstudio/qwen3-coder"
              : "Leave empty for the vendor default"
          }
          className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        {provider === "opencode" && (
          <p className="text-xs text-gray-500 mt-1">
            Required for OpenCode. The model id served by your endpoint
            (e.g. <code className="text-gray-400">lmstudio/qwen3-coder</code>).
          </p>
        )}
      </div>

      <div>
        <label htmlFor="onb-extra-args" className="block text-xs text-gray-400 mb-1">
          Extra args (one per line)
        </label>
        <textarea
          id="onb-extra-args"
          value={extraArgsText}
          onChange={(e) => onChangeExtraArgs(e.target.value)}
          rows={3}
          className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
      </div>
    </div>
  );
}

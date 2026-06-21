// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { Trans, useTranslation } from "react-i18next";
import type { AgentProviderId } from "../../api/types";

const V1_PROVIDERS: AgentProviderId[] = ["claude", "cursor", "codex", "opencode"];

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
  const { t } = useTranslation("onboarding");
  const cursorBaseUrlDisabled = provider === "cursor";
  return (
    <div className="flex flex-col gap-4">
      <div>
        <label htmlFor="onb-provider" className="block text-xs text-gray-400 mb-1">
          {t("provider.label")}
        </label>
        <select
          id="onb-provider"
          value={provider}
          onChange={(e) => onChangeProvider(e.target.value as AgentProviderId)}
          className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
        >
          {V1_PROVIDERS.map((p) => (
            <option key={p} value={p}>
              {t(`provider.options.${p}`)}
            </option>
          ))}
        </select>
        {provider === "opencode" && (
          <p className="text-xs text-gray-500 mt-1">{t("provider.opencodeHint")}</p>
        )}
      </div>

      <div>
        <label htmlFor="onb-base-url" className="block text-xs text-gray-400 mb-1">
          {t("provider.baseUrl")}
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
              ? t("provider.baseUrlPlaceholderOpencode")
              : t("provider.baseUrlPlaceholderDefault")
          }
          disabled={cursorBaseUrlDisabled}
          title={cursorBaseUrlDisabled ? t("provider.cursorNoUpstream") : undefined}
          className={`w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm font-mono ${
            cursorBaseUrlDisabled ? "text-gray-600 cursor-not-allowed" : "text-gray-200"
          }`}
        />
        {cursorBaseUrlDisabled && (
          <p className="text-xs text-gray-500 mt-1">{t("provider.cursorNoUpstreamHint")}</p>
        )}
        {provider === "opencode" && (
          <p className="text-xs text-gray-500 mt-1">{t("provider.baseUrlOpencodeHint")}</p>
        )}
      </div>

      <div>
        <label htmlFor="onb-model" className="block text-xs text-gray-400 mb-1">
          {t("provider.model")}
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
              ? t("provider.modelPlaceholderOpencode")
              : t("provider.modelPlaceholderDefault")
          }
          className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        {provider === "opencode" && (
          <p className="text-xs text-gray-500 mt-1">
            <Trans
              i18nKey="provider.modelOpencodeHint"
              ns="onboarding"
              components={{ code: <code className="text-gray-400" /> }}
            />
          </p>
        )}
      </div>

      <div>
        <label htmlFor="onb-extra-args" className="block text-xs text-gray-400 mb-1">
          {t("provider.extraArgs")}
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

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Admin-only AI provider settings section — a pure renderer of the form state
 * and handlers from `useAiProviderSettings`. The parent tab (`AiSettingsTab`)
 * gates rendering on the caller's role; this file does NOT re-check `isAdmin`.
 * Server-side enforcement at `PUT /api/config/agent` is the real security
 * boundary — this UI gate is cosmetic.
 *
 * Source of truth: tmp/multi-agents/04_architecture.md §2 and
 * tmp/multi-agents/05_ux_design.md §2.5 / §2.6. Switching the active provider
 * triggers a confirm modal (§2.6) because it marks every per-user credential
 * for the previous provider as `inactive=1`.
 */

import { forwardRef, useEffect, useImperativeHandle } from "react";
import { useTranslation } from "react-i18next";
import { useAiProviderSettings } from "../../hooks/useAiProviderSettings";
import { PROVIDER_LABEL, ProviderForm } from "./ProviderForm";
import { ProviderSwitchConfirm } from "./ProviderSwitchConfirm";
import type { ConfigSectionHandle, ConfigSectionProps } from "./configSection";

export { ProviderSwitchConfirm };
export { PROVIDER_LABEL };

interface Props extends ConfigSectionProps {
  /** Called after a successful save so sibling sections (e.g. the per-user
   *  credentials card) can refetch and reflect the newly-selected provider. */
  onProviderSaved?: () => void;
}

export const AiProviderSettingsSection = forwardRef<ConfigSectionHandle, Props>(
  function AiProviderSettingsSection({ onProviderSaved, onDirtyChange }: Props, ref) {
  const { t } = useTranslation("config");
  const {
    loading,
    error,
    isDirty,
    selectedProvider,
    draft,
    availableProviders,
    pendingProviderSwitch,
    selectProvider,
    setDraft,
    toggleAvailable,
    saveAsync,
    confirmSwitch,
    cancelSwitch,
  } = useAiProviderSettings({ onProviderSaved });

  useImperativeHandle(ref, () => ({ isDirty: () => isDirty, save: saveAsync }), [
    isDirty,
    saveAsync,
  ]);

  useEffect(() => {
    onDirtyChange?.(isDirty);
  }, [isDirty, onDirtyChange]);

  return (
    <section aria-labelledby="ai-provider-section-title" className="flex flex-col gap-3">
      <h2 id="ai-provider-section-title" className="text-lg font-semibold text-white">
        {t("ai.providerSettings")}
      </h2>
      <p className="text-xs text-gray-500">
        {t("ai.providerSettingsHelp")}
      </p>

      {loading && <p className="text-sm text-gray-500">{t("actions.loading")}</p>}
      {!loading && error && <p className="text-sm text-red-400">{t("errors.loadConfig", { error })}</p>}
      {!loading && !error && (
        <ProviderForm
          selectedProvider={selectedProvider}
          onSelectProvider={selectProvider}
          draft={draft}
          onDraftChange={setDraft}
          availableProviders={availableProviders}
          onToggleAvailable={toggleAvailable}
        />
      )}

      {pendingProviderSwitch && (
        <ProviderSwitchConfirm
          from={pendingProviderSwitch.from}
          to={pendingProviderSwitch.to}
          onCancel={cancelSwitch}
          onConfirm={confirmSwitch}
        />
      )}
    </section>
  );
});

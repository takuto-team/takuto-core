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

import { useAiProviderSettings } from "../../hooks/useAiProviderSettings";
import { PROVIDER_LABEL, ProviderForm } from "./ProviderForm";
import { ProviderSwitchConfirm } from "./ProviderSwitchConfirm";

export { ProviderSwitchConfirm };
export { PROVIDER_LABEL };

interface Props {
  /** Called after a successful save so sibling sections (e.g. the per-user
   *  credentials card) can refetch and reflect the newly-selected provider. */
  onProviderSaved?: () => void;
}

export function AiProviderSettingsSection({ onProviderSaved }: Props = {}) {
  const {
    loading,
    error,
    saving,
    selectedProvider,
    draft,
    availableProviders,
    pendingProviderSwitch,
    selectProvider,
    setDraft,
    toggleAvailable,
    requestSave,
    confirmSwitch,
    cancelSwitch,
  } = useAiProviderSettings({ onProviderSaved });

  return (
    <section aria-labelledby="ai-provider-section-title" className="flex flex-col gap-3">
      <h2 id="ai-provider-section-title" className="text-lg font-semibold text-white">
        Provider settings
      </h2>
      <p className="text-xs text-gray-500">
        Admin-only. Pick the active AI provider, configure its sub-table, and choose which
        providers users can pick from.
      </p>

      {loading && <p className="text-sm text-gray-500">Loading…</p>}
      {!loading && error && <p className="text-sm text-red-400">Could not load config: {error}</p>}
      {!loading && !error && (
        <ProviderForm
          selectedProvider={selectedProvider}
          onSelectProvider={selectProvider}
          draft={draft}
          onDraftChange={setDraft}
          availableProviders={availableProviders}
          onToggleAvailable={toggleAvailable}
          onSave={requestSave}
          saving={saving}
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
}

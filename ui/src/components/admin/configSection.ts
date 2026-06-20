// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Shared imperative-handle contract for the admin AI config sections
 * (provider settings, share-conversation, step guardrails). The
 * `AiSettingsTab` holds a ref per section, aggregates their dirty state, and
 * drives a SINGLE Save button that calls `save()` on each dirty section.
 *
 * Mirrors the existing `AiCredentialPanelHandle` pattern in
 * `components/credentials/AiCredentialPanel.tsx`.
 */
export interface ConfigSectionHandle {
  /** Whether this section has unsaved edits. */
  isDirty: () => boolean;
  /** Persist this section. Resolves `true` on success, `false` on
   *  validation failure / error / user cancel. A clean section resolves
   *  `true` without a network call. */
  save: () => Promise<boolean>;
}

/** Props every admin config section accepts so the tab can track combined
 *  dirty state without polling. */
export interface ConfigSectionProps {
  /** Called whenever this section's dirty state flips. */
  onDirtyChange?: (dirty: boolean) => void;
}

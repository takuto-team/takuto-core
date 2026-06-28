// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Consolidated "AI Settings" tab on `/config.html`.
 *
 * The three admin config sections (provider settings, share-conversation,
 * step guardrails) all persist to `PUT /api/config/agent`. To avoid the
 * "I edited but forgot to click that section's Save" footgun, this tab owns a
 * SINGLE "Save changes" button: each admin section exposes an imperative
 * `{ isDirty, save }` handle and reports dirty via `onDirtyChange`; the tab
 * saves every dirty section at once.
 *
 * The single Save lives in the page-level `SettingsFooter` (rendered by
 * `Config`): this tab reports combined dirty via `onDirtyChange` and registers
 * `saveAll` via `registerSave`. `saveAll` persists every dirty admin section
 * AND folds in a typed-but-unsaved per-user key (the credential panel runs in
 * `deferSave` mode, so it has no own Save button). The Delete button stays as a
 * discrete action.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { AiProviderSettingsSection } from "./admin/AiProviderSettingsSection";
import { ShareConversationSwitch } from "./admin/ShareConversationSwitch";
import { StepGuardrailsSection } from "./admin/StepGuardrailsSection";
import { MyCredentialsSection } from "./MyCredentialsSection";
import type { ConfigSectionHandle } from "./admin/configSection";
import type { AiCredentialPanelHandle } from "./credentials/AiCredentialPanel";

interface Props {
  isAdmin: boolean;
  /** Reports combined unsaved state (config edits OR a typed-but-unsaved key)
   *  so `Config` can warn before navigation. */
  onDirtyChange?: (dirty: boolean) => void;
  /** Registers the tab's "save all config sections" fn so `Config`'s
   *  unsaved-changes modal can offer "Save & leave". */
  registerSave?: (save: () => Promise<boolean>) => void;
}

export function AiSettingsTab({ isAdmin, onDirtyChange, registerSave }: Props) {
  // Bumped when the admin saves a new active provider so the per-user
  // credential card refetches and shows the right provider without a reload.
  const [credRefreshKey, setCredRefreshKey] = useState(0);
  // Live provider chosen in the admin dropdown (before any save) so the
  // credential card follows the selection in realtime. Null until the admin
  // section reports (or for non-admins, who never render it).
  const [selectedProvider, setSelectedProvider] = useState<string | null>(null);

  const providerRef = useRef<ConfigSectionHandle>(null);
  const shareRef = useRef<ConfigSectionHandle>(null);
  const guardrailsRef = useRef<ConfigSectionHandle>(null);
  const credentialRef = useRef<AiCredentialPanelHandle>(null);

  const [providerDirty, setProviderDirty] = useState(false);
  const [shareDirty, setShareDirty] = useState(false);
  const [guardrailsDirty, setGuardrailsDirty] = useState(false);
  const [credentialDirty, setCredentialDirty] = useState(false);

  const anyDirty = providerDirty || shareDirty || guardrailsDirty || credentialDirty;

  // Save every dirty admin config section, then fold in a typed-but-unsaved
  // per-user key. Returns true only if all succeed.
  const saveAll = useCallback(async (): Promise<boolean> => {
    const handles = [providerRef, shareRef, guardrailsRef]
      .map((r) => r.current)
      .filter((h): h is ConfigSectionHandle => !!h && h.isDirty());
    let ok = true;
    for (const h of handles) {
      if (!(await h.save())) ok = false;
    }
    // A blank key resolves true (no-op); a typed key is persisted here.
    if (credentialRef.current && !(await credentialRef.current.saveIfDirty())) ok = false;
    return ok;
  }, []);

  // Report combined dirty + register the saver so Config can drive the guard.
  useEffect(() => {
    onDirtyChange?.(anyDirty);
  }, [anyDirty, onDirtyChange]);
  useEffect(() => {
    registerSave?.(saveAll);
  }, [registerSave, saveAll]);

  return (
    <div className="flex flex-col gap-10">
      {isAdmin && (
        <AiProviderSettingsSection
          ref={providerRef}
          onDirtyChange={setProviderDirty}
          onProviderSaved={() => setCredRefreshKey((k) => k + 1)}
          onProviderChange={setSelectedProvider}
        />
      )}
      {isAdmin && <ShareConversationSwitch ref={shareRef} onDirtyChange={setShareDirty} />}
      {isAdmin && <StepGuardrailsSection ref={guardrailsRef} onDirtyChange={setGuardrailsDirty} />}

      <MyCredentialsSection
        refreshKey={credRefreshKey}
        providerOverride={selectedProvider ?? undefined}
        onDirtyChange={setCredentialDirty}
        panelRef={credentialRef}
        deferSave
      />
    </div>
  );
}

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
 * The per-user credential (`MyCredentialsSection`) keeps its own
 * Save/Replace/Delete buttons (a credential is a discrete secret action, not a
 * batched setting), but a typed-but-unsaved key still contributes to the
 * unsaved-changes warning the parent (`Config`) shows on navigation.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { AiProviderSettingsSection } from "./admin/AiProviderSettingsSection";
import { ShareConversationSwitch } from "./admin/ShareConversationSwitch";
import { StepGuardrailsSection } from "./admin/StepGuardrailsSection";
import { MyCredentialsSection } from "./MyCredentialsSection";
import type { ConfigSectionHandle } from "./admin/configSection";

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

  const providerRef = useRef<ConfigSectionHandle>(null);
  const shareRef = useRef<ConfigSectionHandle>(null);
  const guardrailsRef = useRef<ConfigSectionHandle>(null);

  const [providerDirty, setProviderDirty] = useState(false);
  const [shareDirty, setShareDirty] = useState(false);
  const [guardrailsDirty, setGuardrailsDirty] = useState(false);
  const [credentialDirty, setCredentialDirty] = useState(false);
  const [saving, setSaving] = useState(false);

  const configDirty = providerDirty || shareDirty || guardrailsDirty;
  const anyDirty = configDirty || credentialDirty;

  // Save every dirty admin config section. Returns true only if all succeed.
  const saveAll = useCallback(async (): Promise<boolean> => {
    setSaving(true);
    try {
      const handles = [providerRef, shareRef, guardrailsRef]
        .map((r) => r.current)
        .filter((h): h is ConfigSectionHandle => !!h && h.isDirty());
      let ok = true;
      for (const h of handles) {
        if (!(await h.save())) ok = false;
      }
      return ok;
    } finally {
      setSaving(false);
    }
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
        />
      )}
      {isAdmin && <ShareConversationSwitch ref={shareRef} onDirtyChange={setShareDirty} />}
      {isAdmin && <StepGuardrailsSection ref={guardrailsRef} onDirtyChange={setGuardrailsDirty} />}

      {isAdmin && (
        <div className="flex items-center justify-end gap-3 sticky bottom-0 bg-gray-950/80 backdrop-blur-sm py-3 border-t border-gray-800">
          {configDirty && (
            <span className="text-xs text-amber-300">Unsaved changes</span>
          )}
          <button
            type="button"
            disabled={!configDirty || saving}
            onClick={() => void saveAll()}
            className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
          >
            {saving ? "Saving…" : "Save changes"}
          </button>
        </div>
      )}

      <MyCredentialsSection refreshKey={credRefreshKey} onDirtyChange={setCredentialDirty} />
    </div>
  );
}

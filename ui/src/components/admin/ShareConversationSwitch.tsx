// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Switch for `[agent].share_conversation_across_steps`, rendered as its own
 * card below the provider settings.
 *
 * Edits are BATCHED behind the AI Settings tab's single Save button (the tab
 * holds a ref and calls `save()`). Toggling only updates the local draft and
 * reports dirty via `onDirtyChange`; nothing persists until the tab saves.
 */

import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { apiJson } from "../../api/http";
import { putAgentConfig } from "../../api/agentConfig";
import type { AgentConfig, ConfigResponse } from "../../api/types";
import type { ConfigSectionHandle, ConfigSectionProps } from "./configSection";

export const ShareConversationSwitch = forwardRef<
  ConfigSectionHandle,
  ConfigSectionProps
>(function ShareConversationSwitch({ onDirtyChange }: ConfigSectionProps, ref) {
  const { t } = useTranslation("config");
  // `loaded` is the persisted value; `enabled` is the draft. Dirty when they differ.
  const [loaded, setLoaded] = useState(false);
  const [enabled, setEnabled] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    let cancelled = false;
    apiJson<ConfigResponse>("/api/config")
      .then((c) => {
        if (cancelled) return;
        const agent = (c.agent ?? {}) as AgentConfig;
        const v = agent.share_conversation_across_steps === true;
        setLoaded(v);
        setEnabled(v);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const dirty = enabled !== loaded;

  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);

  const save = useCallback(async (): Promise<boolean> => {
    if (enabled === loaded) return true;
    setError("");
    try {
      await putAgentConfig({ share_conversation_across_steps: enabled });
      setLoaded(enabled);
      return true;
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
      return false;
    }
  }, [enabled, loaded]);

  useImperativeHandle(ref, () => ({ isDirty: () => dirty, save }), [dirty, save]);

  return (
    <section className="border border-gray-800 rounded-xl bg-gray-950 p-5">
      <div className="flex items-start justify-between gap-6">
        <div className="min-w-0">
          <h3 className="text-base font-semibold text-gray-200">
            {t("ai.share.title")}
          </h3>
          <p className="text-sm text-gray-500 mt-1 max-w-2xl">
            {t("ai.share.help")}
          </p>
          {error && <p className="text-sm text-red-400 mt-2">{t("errors.saveFailed", { error })}</p>}
        </div>

        <button
          type="button"
          role="switch"
          aria-checked={enabled}
          aria-label={t("ai.share.title")}
          disabled={loading}
          onClick={() => setEnabled((v) => !v)}
          className={`relative inline-flex h-7 w-12 flex-shrink-0 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500/50 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer ${
            enabled ? "bg-blue-600" : "bg-gray-700"
          }`}
        >
          <span
            className={`inline-block h-5 w-5 transform rounded-full bg-white transition-transform ${
              enabled ? "translate-x-6" : "translate-x-1"
            }`}
          />
        </button>
      </div>
    </section>
  );
});

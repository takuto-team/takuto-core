// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Standalone, auto-saving switch for `[agent].share_conversation_across_steps`.
 *
 * Unlike the provider form (which batches edits behind a Save button), this
 * setting is a single boolean that persists the moment it is flipped: the
 * switch issues `PUT /api/config/agent` with just the one field, disables
 * itself while the request is in flight, and reverts on failure. Rendered as
 * its own card below the provider settings.
 */

import { useCallback, useEffect, useState } from "react";
import { apiJson } from "../../api/http";
import { putAgentConfig } from "../../api/agentConfig";
import type { AgentConfig, ConfigResponse } from "../../api/types";

export function ShareConversationSwitch() {
  const [enabled, setEnabled] = useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let cancelled = false;
    apiJson<ConfigResponse>("/api/config")
      .then((c) => {
        if (cancelled) return;
        const agent = (c.agent ?? {}) as AgentConfig;
        setEnabled(agent.share_conversation_across_steps === true);
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

  const toggle = useCallback(async () => {
    if (saving || loading) return;
    const next = !enabled;
    setEnabled(next); // optimistic
    setSaving(true);
    setError("");
    try {
      await putAgentConfig({ share_conversation_across_steps: next });
    } catch (e: unknown) {
      setEnabled(!next); // revert
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }, [enabled, saving, loading]);

  const busy = loading || saving;

  return (
    <section className="border border-gray-800 rounded-xl bg-gray-950 p-5">
      <div className="flex items-start justify-between gap-6">
        <div className="min-w-0">
          <h3 className="text-base font-semibold text-gray-200">
            Share one conversation across a flow's steps
          </h3>
          <p className="text-sm text-gray-500 mt-1 max-w-2xl">
            When on, each step resumes the previous step's session, so the agent
            carries full context forward — it remembers what it implemented when it
            reviews. When off, every step runs in a fresh session with no memory of
            earlier steps (the default; safer for smaller local models that get
            confused by long transcripts). Applies to all providers and saves
            immediately.
          </p>
          {error && <p className="text-sm text-red-400 mt-2">Could not save: {error}</p>}
        </div>

        <button
          type="button"
          role="switch"
          aria-checked={enabled}
          aria-label="Share one conversation across a flow's steps"
          disabled={busy}
          onClick={toggle}
          className={`relative inline-flex h-7 w-12 flex-shrink-0 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500/50 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer ${
            enabled ? "bg-blue-600" : "bg-gray-700"
          }`}
        >
          <span
            className={`inline-block h-5 w-5 transform rounded-full bg-white transition-transform ${
              enabled ? "translate-x-6" : "translate-x-1"
            } ${saving ? "animate-pulse" : ""}`}
          />
        </button>
      </div>
    </section>
  );
}

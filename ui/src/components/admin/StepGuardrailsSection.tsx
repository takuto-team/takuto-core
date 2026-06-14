// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Admin-only "Step guardrails" panel on the AI Settings tab.
 *
 * Tunes the three `[agent]` runtime guards — the per-step timeout, the
 * "improve description" timeout, and the no-progress repeated-output line
 * count — via `PUT /api/config/agent`. Kept as its own card (sibling to the
 * provider form and the share-conversation switch) so each concern owns one
 * file per CODING_STANDARDS §1/§3. Server-side `validate()` enforces the
 * floors; this UI only surfaces the values and the persist warning.
 */

import { useCallback, useEffect, useState } from "react";
import { apiJson } from "../../api/http";
import { putAgentConfig, AgentConfigError } from "../../api/agentConfig";
import { useToast } from "../../hooks/useToast";
import type { AgentConfig, AgentConfigPatch, ConfigResponse } from "../../api/types";

/** Editable form state. Numeric inputs are strings so they stay controlled. */
interface GuardrailsDraft {
  step_timeout_secs: string;
  improve_timeout_secs: string;
  max_repeated_output_lines: string;
}

const EMPTY_DRAFT: GuardrailsDraft = {
  step_timeout_secs: "",
  improve_timeout_secs: "",
  max_repeated_output_lines: "",
};

function draftFromConfig(agent: AgentConfig | undefined): GuardrailsDraft {
  return {
    step_timeout_secs:
      typeof agent?.step_timeout_secs === "number" ? String(agent.step_timeout_secs) : "",
    improve_timeout_secs:
      typeof agent?.improve_timeout_secs === "number"
        ? String(agent.improve_timeout_secs)
        : "",
    max_repeated_output_lines:
      typeof agent?.max_repeated_output_lines === "number"
        ? String(agent.max_repeated_output_lines)
        : "",
  };
}

/**
 * Parse a guardrail input. Empty / non-numeric → `undefined` (omit the key so
 * the server leaves it unchanged). A parseable integer is sent verbatim so the
 * server's `validate()` floor check can reject out-of-range values with a 400.
 */
function parseField(raw: string): number | undefined {
  const t = raw.trim();
  if (t === "") return undefined;
  const n = Number.parseInt(t, 10);
  return Number.isFinite(n) ? n : undefined;
}

function patchFromDraft(draft: GuardrailsDraft): AgentConfigPatch {
  return {
    step_timeout_secs: parseField(draft.step_timeout_secs),
    improve_timeout_secs: parseField(draft.improve_timeout_secs),
    max_repeated_output_lines: parseField(draft.max_repeated_output_lines),
  };
}

export function StepGuardrailsSection() {
  const { showToast } = useToast();
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [draft, setDraft] = useState<GuardrailsDraft>(EMPTY_DRAFT);

  useEffect(() => {
    let cancelled = false;
    apiJson<ConfigResponse>("/api/config")
      .then((c) => {
        if (!cancelled) setDraft(draftFromConfig((c.agent ?? {}) as AgentConfig));
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

  const update = (patch: Partial<GuardrailsDraft>) => setDraft((d) => ({ ...d, ...patch }));

  const handleSave = useCallback(() => {
    setSaving(true);
    putAgentConfig(patchFromDraft(draft))
      .then((updated) => {
        setDraft(draftFromConfig((updated.agent ?? {}) as AgentConfig));
        if (updated.persisted === false) {
          const reason = updated.persist_warning ?? "unknown error";
          showToast(
            `Step guardrails applied in memory but NOT persisted to disk: ${reason}. The change will be lost on next restart — fix the config volume and save again.`,
            "error",
          );
        } else {
          showToast("Step guardrails saved.", "success");
        }
      })
      .catch((e: unknown) => {
        if (e instanceof AgentConfigError) {
          showToast(`${e.message} (code: ${e.code})`, "error");
        } else {
          showToast(e instanceof Error ? e.message : String(e), "error");
        }
      })
      .finally(() => setSaving(false));
  }, [draft, showToast]);

  return (
    <section aria-labelledby="step-guardrails-title" className="flex flex-col gap-3">
      <h2 id="step-guardrails-title" className="text-lg font-semibold text-white">
        Step guardrails
      </h2>
      <p className="text-xs text-gray-500">
        Admin-only. Runtime limits applied to every agent step: how long a step
        may run, how long the &ldquo;improve description&rdquo; call may run, and
        the no-progress loop guard.
      </p>

      {loading && <p className="text-sm text-gray-500">Loading…</p>}
      {!loading && error && (
        <p className="text-sm text-red-400">Could not load config: {error}</p>
      )}
      {!loading && !error && (
        <div className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-6">
          <section className="flex flex-col gap-2">
            <label htmlFor="step-timeout-input" className="text-xs text-gray-400">
              Step timeout (seconds)
            </label>
            <input
              id="step-timeout-input"
              type="number"
              min={1}
              value={draft.step_timeout_secs}
              onChange={(e) => update({ step_timeout_secs: e.target.value })}
              placeholder="Leave empty for the default"
              className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
            />
            <p className="text-xs text-gray-500">
              Maximum wall-clock time for a single agent or command step before
              it is aborted.
            </p>
          </section>

          <section className="flex flex-col gap-2">
            <label htmlFor="improve-timeout-input" className="text-xs text-gray-400">
              Improve-description timeout (seconds)
            </label>
            <input
              id="improve-timeout-input"
              type="number"
              min={1}
              value={draft.improve_timeout_secs}
              onChange={(e) => update({ improve_timeout_secs: e.target.value })}
              placeholder="Leave empty for the default"
              className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
            />
            <p className="text-xs text-gray-500">
              Time budget for the one-shot &ldquo;improve description&rdquo; helper
              used on the manual paste-description modal.
            </p>
          </section>

          <section className="flex flex-col gap-2">
            <label htmlFor="max-repeated-output-input" className="text-xs text-gray-400">
              Max repeated output lines
            </label>
            <input
              id="max-repeated-output-input"
              type="number"
              min={0}
              value={draft.max_repeated_output_lines}
              onChange={(e) => update({ max_repeated_output_lines: e.target.value })}
              placeholder="8"
              className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
            />
            <p className="text-xs text-gray-500">
              Consecutive identical output lines that trip the no-progress guard
              and fail the step. <code className="text-gray-400">0</code> = off.
            </p>
          </section>

          <div className="flex justify-end">
            <button
              type="button"
              disabled={saving}
              onClick={handleSave}
              className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
            >
              {saving ? "Saving…" : "Save changes"}
            </button>
          </div>
        </div>
      )}
    </section>
  );
}

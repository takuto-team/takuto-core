// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Admin-only, deployment-global "Jira context" section.
 *
 * The Jira-context *processing* fields of the `[jira]` section — how linked
 * issues / the ticket description are embedded in agent prompts, and the
 * Mark-as-Done target — saved via `PUT /api/config/jira`. These are consumed
 * globally by the engine (it reads `[jira]`, not the per-repo blob); making
 * them per-repository is a deferred follow-up. The per-repo `jql_filter` poll
 * filter lives in the per-repo polling section, not here.
 *
 * Exposes the shared `ConfigSectionHandle` ({ isDirty, save }) via `forwardRef`
 * so the Ticketing tab footer can drive a single Save, and reports dirtiness
 * via `onDirtyChange`.
 */

import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { apiJson } from "../../api/client";
import { JiraConfigError, putJiraConfig } from "../../api/jiraConfig";
import { useToast } from "../../hooks/useToast";
import type { ConfigResponse, LinkedItemsInPrompt } from "../../api/types";
import type { ConfigSectionHandle } from "./configSection";
import { JiraContextFields } from "./JiraContextFields";

interface JiraContextDraft {
  linked_items_in_prompt: LinkedItemsInPrompt;
  ticket_context_max_description_bytes: string;
  linked_issue_description_max_bytes: string;
  done_status: string;
}

const EMPTY_DRAFT: JiraContextDraft = {
  linked_items_in_prompt: "full",
  ticket_context_max_description_bytes: "0",
  linked_issue_description_max_bytes: "0",
  done_status: "",
};

/** Stringify a numeric config field, falling back to `fallback` when absent. */
function numToStr(value: unknown, fallback: string): string {
  return typeof value === "number" ? String(value) : fallback;
}

function draftFromConfig(config: ConfigResponse): JiraContextDraft {
  const jira = config.jira ?? {};
  const linked: LinkedItemsInPrompt =
    jira.linked_items_in_prompt === "summary_only" || jira.linked_items_in_prompt === "omit"
      ? jira.linked_items_in_prompt
      : "full";
  return {
    linked_items_in_prompt: linked,
    ticket_context_max_description_bytes: numToStr(jira.ticket_context_max_description_bytes, "0"),
    linked_issue_description_max_bytes: numToStr(jira.linked_issue_description_max_bytes, "0"),
    done_status: typeof jira.done_status === "string" ? jira.done_status : "",
  };
}

/** Parse a "0 = unlimited" input; empty / negative / non-numeric → 0. */
function parseNonNegInt(raw: string): number {
  const n = Number.parseInt(raw.trim(), 10);
  return Number.isFinite(n) && n > 0 ? n : 0;
}

interface Props {
  /** Reports unsaved edits so the parent's footer Save enables. */
  onDirtyChange?: (dirty: boolean) => void;
}

export const GlobalJiraContextSection = forwardRef<ConfigSectionHandle, Props>(
  function GlobalJiraContextSection({ onDirtyChange }, ref) {
    const { t } = useTranslation("config");
    const { showToast } = useToast();
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState("");
    const [draft, setDraft] = useState<JiraContextDraft>(EMPTY_DRAFT);
    const [original, setOriginal] = useState<JiraContextDraft>(EMPTY_DRAFT);

    useEffect(() => {
      let mounted = true;
      setLoading(true);
      setError("");
      apiJson<ConfigResponse>("/api/config")
        .then((config) => {
          if (!mounted) return;
          const seeded = draftFromConfig(config);
          setDraft(seeded);
          setOriginal(seeded);
        })
        .catch((e: unknown) => {
          if (mounted) setError(e instanceof Error ? e.message : String(e));
        })
        .finally(() => {
          if (mounted) setLoading(false);
        });
      return () => {
        mounted = false;
      };
    }, []);

    const dirty = JSON.stringify(draft) !== JSON.stringify(original);

    const runSave = useCallback(async (): Promise<boolean> => {
      if (!dirty) return true;
      const doneStatus = draft.done_status.trim();
      try {
        // Empty done_status is rejected by the server (blank status is invalid),
        // so omit it when blank rather than sending an empty string.
        const updated = await putJiraConfig({
          linked_items_in_prompt: draft.linked_items_in_prompt,
          ticket_context_max_description_bytes: parseNonNegInt(
            draft.ticket_context_max_description_bytes,
          ),
          linked_issue_description_max_bytes: parseNonNegInt(
            draft.linked_issue_description_max_bytes,
          ),
          ...(doneStatus !== "" ? { done_status: doneStatus } : {}),
        });
        const fresh = draftFromConfig(updated);
        setDraft(fresh);
        setOriginal(fresh);
        if (updated.persisted === false) {
          showToast(
            t("polling.persistWarning", { reason: updated.persist_warning ?? "unknown error" }),
            "error",
          );
        } else {
          showToast(t("polling.savedToast"), "success");
        }
        return true;
      } catch (e: unknown) {
        if (e instanceof JiraConfigError) {
          showToast(t("errors.withCode", { message: e.message, code: e.code }), "error");
        } else {
          showToast(e instanceof Error ? e.message : String(e), "error");
        }
        return false;
      }
    }, [dirty, draft, showToast, t]);

    useEffect(() => {
      onDirtyChange?.(dirty);
    }, [dirty, onDirtyChange]);
    useImperativeHandle(ref, () => ({ isDirty: () => dirty, save: runSave }), [dirty, runSave]);

    const update = (patch: Partial<JiraContextDraft>) => setDraft((d) => ({ ...d, ...patch }));

    return (
      <section aria-labelledby="jira-context-title" className="flex flex-col gap-3">
        <h2 id="jira-context-title" className="text-lg font-semibold text-white">
          {t("polling.jiraContext.title")}
        </h2>
        <p className="text-xs text-gray-500">{t("polling.jiraContext.sectionHelp")}</p>

        {loading && <p className="text-sm text-gray-500">{t("actions.loading")}</p>}
        {!loading && error && (
          <p className="text-sm text-red-400">{t("errors.loadConfig", { error })}</p>
        )}
        {!loading && !error && (
          <div className="bg-gray-900 border border-gray-800 rounded-xl p-6">
            <JiraContextFields
              linkedItemsInPrompt={draft.linked_items_in_prompt}
              ticketContextMaxDescriptionBytes={draft.ticket_context_max_description_bytes}
              linkedIssueDescriptionMaxBytes={draft.linked_issue_description_max_bytes}
              doneStatus={draft.done_status}
              onLinkedItemsInPromptChange={(v) => update({ linked_items_in_prompt: v })}
              onTicketContextMaxDescriptionBytesChange={(v) =>
                update({ ticket_context_max_description_bytes: v })
              }
              onLinkedIssueDescriptionMaxBytesChange={(v) =>
                update({ linked_issue_description_max_bytes: v })
              }
              onDoneStatusChange={(v) => update({ done_status: v })}
            />
          </div>
        )}
      </section>
    );
  },
);

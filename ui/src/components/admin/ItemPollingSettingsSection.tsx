// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Admin-only Item Polling settings section.
 *
 * Lives on the "Item Polling" tab of /config.html. The parent tab gate decides
 * whether to render this; server-side enforcement at `PUT /api/config/polling`
 * (403 for non-admins) is the real security boundary. A single PUT panel that
 * tunes the `[polling]` policy: the auto-start flow, the parallel-item cap, and
 * the per-system Jira / GitHub filters.
 */

import { useCallback, useEffect, useState } from "react";
import { apiJson } from "../../api/client";
import { getMyFlows, slugify } from "../../api/flows";
import {
  ItemPollingConfigError,
  putItemPollingConfig,
} from "../../api/itemPollingConfig";
import { JiraConfigError, putJiraConfig } from "../../api/jiraConfig";
import { useToast } from "../../hooks/useToast";
import type {
  ConfigResponse,
  ItemPollingConfigPatch,
  JiraConfigPatch,
  LinkedItemsInPrompt,
  PollingConfig,
} from "../../api/types";
import {
  EMPTY_POLLING_DRAFT,
  ItemPollingForm,
  type AutoStartFlowOption,
  type ItemPollingDraft,
} from "./ItemPollingForm";

/** Stringify a numeric config field, falling back to `fallback` when absent. */
function numToStr(value: unknown, fallback: string): string {
  return typeof value === "number" ? String(value) : fallback;
}

function draftFromConfig(config: ConfigResponse): ItemPollingDraft {
  const polling: Partial<PollingConfig> = config.polling ?? {};
  const general = config.general ?? {};
  const jira = config.jira ?? {};
  const jiraItemTypes = Array.isArray(jira.item_types)
    ? (jira.item_types as string[])
    : [];
  const pollInterval =
    typeof general.poll_interval_secs === "number" ? general.poll_interval_secs : 60;
  const linkedItemsInPrompt: LinkedItemsInPrompt =
    jira.linked_items_in_prompt === "summary_only" || jira.linked_items_in_prompt === "omit"
      ? jira.linked_items_in_prompt
      : "full";
  return {
    auto_polling: general.auto_polling ?? true,
    poll_interval_secs: String(pollInterval),
    auto_start_flow: polling.auto_start_flow ?? "",
    max_parallel_items: String(polling.max_parallel_items ?? 0),
    max_parallel_per_user: polling.max_parallel_per_user ?? false,
    item_types: jiraItemTypes,
    jira_summary_keywords: polling.jira?.summary_keywords ?? [],
    github_labels: polling.github?.labels ?? [],
    github_title_keywords: polling.github?.title_keywords ?? [],
    max_concurrent_manual_workflows: numToStr(general.max_concurrent_manual_workflows, "0"),
    pr_merge_poll_interval_secs: numToStr(general.pr_merge_poll_interval_secs, ""),
    generate_report: general.generate_report === true,
    work_item_log_retention_days: numToStr(general.work_item_log_retention_days, "0"),
    linked_items_in_prompt: linkedItemsInPrompt,
    ticket_context_max_description_bytes: numToStr(
      jira.ticket_context_max_description_bytes,
      "0",
    ),
    linked_issue_description_max_bytes: numToStr(jira.linked_issue_description_max_bytes, "0"),
    jql_filter: typeof jira.jql_filter === "string" ? jira.jql_filter : "",
    done_status: typeof jira.done_status === "string" ? jira.done_status : "",
    project_keys: Array.isArray(jira.project_keys) ? jira.project_keys : [],
  };
}

/** Parse a "0 = unlimited / forever" input; empty / negative / non-numeric → 0. */
function parseNonNegInt(raw: string): number {
  const n = Number.parseInt(raw.trim(), 10);
  return Number.isFinite(n) && n > 0 ? n : 0;
}

/** Parse a positive-only input; non-positive / non-numeric → undefined (omit,
 *  leave unchanged). Out-of-range values are sent as-is so the server returns
 *  its floor message. */
function parsePositiveOrOmit(raw: string): number | undefined {
  const n = Number.parseInt(raw.trim(), 10);
  return Number.isFinite(n) && n > 0 ? n : undefined;
}

/** Trim and drop blank entries — the validator rejects whitespace-only items with 400. */
function cleanList(values: string[]): string[] {
  return values.map((v) => v.trim()).filter((v) => v.length > 0);
}

function pollingPatchFromDraft(draft: ItemPollingDraft): ItemPollingConfigPatch {
  return {
    auto_polling: draft.auto_polling,
    poll_interval_secs: parsePositiveOrOmit(draft.poll_interval_secs),
    auto_start_flow: draft.auto_start_flow,
    max_parallel_items: parseNonNegInt(draft.max_parallel_items),
    max_parallel_per_user: draft.max_parallel_per_user,
    jira: { summary_keywords: cleanList(draft.jira_summary_keywords) },
    github: {
      labels: cleanList(draft.github_labels),
      title_keywords: cleanList(draft.github_title_keywords),
    },
    item_types: cleanList(draft.item_types),
    max_concurrent_manual_workflows: parseNonNegInt(draft.max_concurrent_manual_workflows),
    pr_merge_poll_interval_secs: parsePositiveOrOmit(draft.pr_merge_poll_interval_secs),
    generate_report: draft.generate_report,
    work_item_log_retention_days: parseNonNegInt(draft.work_item_log_retention_days),
  };
}

function jiraPatchFromDraft(draft: ItemPollingDraft): JiraConfigPatch {
  const doneStatus = draft.done_status.trim();
  const patch: JiraConfigPatch = {
    linked_items_in_prompt: draft.linked_items_in_prompt,
    ticket_context_max_description_bytes: parseNonNegInt(
      draft.ticket_context_max_description_bytes,
    ),
    linked_issue_description_max_bytes: parseNonNegInt(
      draft.linked_issue_description_max_bytes,
    ),
    // Empty jql_filter is a valid "clear it" signal; empty done_status is not
    // (the server rejects a blank status with 400), so omit it when blank.
    jql_filter: draft.jql_filter.trim(),
    project_keys: cleanList(draft.project_keys),
  };
  if (doneStatus !== "") patch.done_status = doneStatus;
  return patch;
}

export function ItemPollingSettingsSection() {
  const { showToast } = useToast();
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [draft, setDraft] = useState<ItemPollingDraft>(EMPTY_POLLING_DRAFT);
  const [flows, setFlows] = useState<AutoStartFlowOption[]>([]);
  const [ticketingSystem, setTicketingSystem] = useState("none");

  const refresh = useCallback(() => {
    setLoading(true);
    setError("");
    Promise.all([apiJson<ConfigResponse>("/api/config"), getMyFlows()])
      .then(([config, flowsResponse]) => {
        setDraft(draftFromConfig(config));
        setTicketingSystem(config.ticketing_system ?? "none");
        setFlows(
          flowsResponse.flows.map((f) => ({ slug: slugify(f.name), name: f.name })),
        );
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const handleSave = useCallback(() => {
    setSaving(true);
    // The General-limits + polling fields ride PUT /api/config/polling; the
    // Jira-context fields ride the separate PUT /api/config/jira. Fire the
    // polling patch first, then the Jira patch only when Jira is active, and
    // rebuild the draft from the freshest response.
    void (async () => {
      try {
        let updated = await putItemPollingConfig(pollingPatchFromDraft(draft));
        let persistWarning =
          updated.persisted === false ? (updated.persist_warning ?? "unknown error") : null;
        if (ticketingSystem === "jira") {
          updated = await putJiraConfig(jiraPatchFromDraft(draft));
          if (updated.persisted === false) {
            persistWarning = updated.persist_warning ?? "unknown error";
          }
        }
        // Don't touch ticketingSystem here: these PUTs return the flattened
        // Config without the synthesized `ticketing_system` field, so reading
        // it would clobber the loaded value with "none" and hide the filters
        // until refresh. It can't change as a result of this save.
        setDraft(draftFromConfig(updated));
        if (persistWarning) {
          showToast(
            `Item polling settings applied in memory but NOT persisted to disk: ${persistWarning}. The change will be lost on next restart — fix the config volume and save again.`,
            "error",
          );
        } else {
          showToast("Item polling settings saved.", "success");
        }
      } catch (e: unknown) {
        if (e instanceof ItemPollingConfigError || e instanceof JiraConfigError) {
          showToast(`${e.message} (code: ${e.code})`, "error");
        } else {
          showToast(e instanceof Error ? e.message : String(e), "error");
        }
      } finally {
        setSaving(false);
      }
    })();
  }, [draft, ticketingSystem, showToast]);

  return (
    <section aria-labelledby="item-polling-section-title" className="flex flex-col gap-3">
      <h2 id="item-polling-section-title" className="text-lg font-semibold text-white">
        Item polling
      </h2>
      <p className="text-xs text-gray-500">
        Admin-only. Control which polled work items become workflows: the flow
        they auto-start, how many run in parallel, and the Jira / GitHub filters.
      </p>

      {loading && <p className="text-sm text-gray-500">Loading…</p>}
      {!loading && error && (
        <p className="text-sm text-red-400">Could not load config: {error}</p>
      )}
      {!loading && !error && (
        <ItemPollingForm
          draft={draft}
          onDraftChange={setDraft}
          flows={flows}
          ticketingSystem={ticketingSystem}
          onSave={handleSave}
          saving={saving}
        />
      )}
    </section>
  );
}

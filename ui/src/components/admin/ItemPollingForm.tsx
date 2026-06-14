// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Pure form for the Item Polling settings section. The connected section owns
 * the fetch / save flow and feeds this component a draft plus the available
 * auto-start flows (CODING_STANDARDS §3 — fetch and render are split).
 */

import type { LinkedItemsInPrompt } from "../../api/types";
import { ChipInput } from "./ChipInput";
import { GeneralLimitsFields } from "./GeneralLimitsFields";
import { JiraContextFields } from "./JiraContextFields";

/** A flow the auto-start <select> can target: stable slug + display name. */
export interface AutoStartFlowOption {
  slug: string;
  name: string;
}

/** Editable form state. Numeric inputs are strings so they stay controlled. */
export interface ItemPollingDraft {
  auto_polling: boolean;
  poll_interval_secs: string;
  auto_start_flow: string;
  max_parallel_items: string;
  max_parallel_per_user: boolean;
  item_types: string[];
  jira_summary_keywords: string[];
  github_labels: string[];
  github_title_keywords: string[];
  // General limits — ride PUT /api/config/polling ([general] block).
  max_concurrent_manual_workflows: string;
  pr_merge_poll_interval_secs: string;
  generate_report: boolean;
  work_item_log_retention_days: string;
  // Jira context — ride PUT /api/config/jira ([jira] block).
  linked_items_in_prompt: LinkedItemsInPrompt;
  ticket_context_max_description_bytes: string;
  linked_issue_description_max_bytes: string;
  jql_filter: string;
  done_status: string;
  project_keys: string[];
}

export const EMPTY_POLLING_DRAFT: ItemPollingDraft = {
  auto_polling: true,
  poll_interval_secs: "60",
  auto_start_flow: "",
  max_parallel_items: "0",
  max_parallel_per_user: false,
  item_types: [],
  jira_summary_keywords: [],
  github_labels: [],
  github_title_keywords: [],
  max_concurrent_manual_workflows: "0",
  pr_merge_poll_interval_secs: "",
  generate_report: false,
  work_item_log_retention_days: "0",
  linked_items_in_prompt: "full",
  ticket_context_max_description_bytes: "0",
  linked_issue_description_max_bytes: "0",
  jql_filter: "",
  done_status: "",
  project_keys: [],
};

interface ItemPollingFormProps {
  draft: ItemPollingDraft;
  onDraftChange: (d: ItemPollingDraft) => void;
  flows: AutoStartFlowOption[];
  /** Active ticketing system ("jira" | "github" | "none"); decides which filter block shows. */
  ticketingSystem: string;
  onSave: () => void;
  saving: boolean;
}

export function ItemPollingForm({
  draft,
  onDraftChange,
  flows,
  ticketingSystem,
  onSave,
  saving,
}: ItemPollingFormProps) {
  const update = (patch: Partial<ItemPollingDraft>) => onDraftChange({ ...draft, ...patch });
  // Filters are per-system; only the active ticketing system's block is
  // actionable. With no ticketing system the poller is idle, so neither shows.
  const showJira = ticketingSystem === "jira";
  const showGithub = ticketingSystem === "github";

  // Surface a saved-but-now-missing slug as its own option so the admin sees
  // what is configured rather than a silently empty select.
  const selectedMissing =
    draft.auto_start_flow !== "" && !flows.some((f) => f.slug === draft.auto_start_flow);

  return (
    <div className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-6">
      {/* Enable / disable item polling */}
      <section className="flex items-start justify-between gap-4">
        <div className="flex flex-col gap-0.5">
          <span className="text-sm text-gray-200">Enable item polling</span>
          <span className="text-xs text-gray-500">
            When off, the poller is paused and no items are auto-added. Takes
            effect immediately and persists across restarts.
          </span>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={draft.auto_polling}
          aria-label="Enable item polling"
          onClick={() => update({ auto_polling: !draft.auto_polling })}
          className={`relative inline-flex h-7 w-12 flex-shrink-0 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500/50 cursor-pointer ${
            draft.auto_polling ? "bg-blue-600" : "bg-gray-700"
          }`}
        >
          <span
            className={`inline-block h-5 w-5 transform rounded-full bg-white transition-transform ${
              draft.auto_polling ? "translate-x-6" : "translate-x-1"
            }`}
          />
        </button>
      </section>

      {/* Poll interval */}
      <section className="flex flex-col gap-2">
        <label htmlFor="poll-interval-input" className="text-xs text-gray-400">
          Poll interval (seconds)
        </label>
        <input
          id="poll-interval-input"
          type="number"
          min={10}
          value={draft.poll_interval_secs}
          onChange={(e) => update({ poll_interval_secs: e.target.value })}
          placeholder="60"
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        <p className="text-xs text-gray-500">
          How often the poller checks for new work items. Minimum{" "}
          <code className="text-gray-400">10</code>.
        </p>
      </section>

      {/* Auto-start flow */}
      <section className="flex flex-col gap-2">
        <label htmlFor="auto-start-flow-select" className="text-xs text-gray-400">
          Auto-start flow
        </label>
        <select
          id="auto-start-flow-select"
          value={draft.auto_start_flow}
          onChange={(e) => update({ auto_start_flow: e.target.value })}
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
        >
          <option value="">All dep-free flows (default)</option>
          {flows.map((f) => (
            <option key={f.slug} value={f.slug}>
              {f.name}
            </option>
          ))}
          {selectedMissing && (
            <option value={draft.auto_start_flow}>
              {draft.auto_start_flow} (not in your flows)
            </option>
          )}
        </select>
        <p className="text-xs text-gray-500">
          The single flow each polled item auto-starts. Leave on the default to
          start every dependency-free flow.
        </p>
      </section>

      {/* Parallel-item cap */}
      <section className="flex flex-col gap-2">
        <label htmlFor="max-parallel-items-input" className="text-xs text-gray-400">
          Max parallel items
        </label>
        <input
          id="max-parallel-items-input"
          type="number"
          min={0}
          value={draft.max_parallel_items}
          onChange={(e) => update({ max_parallel_items: e.target.value })}
          placeholder="0"
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        <p className="text-xs text-gray-500">
          Cap on items occupying a slot at once. <code className="text-gray-400">0</code> = unlimited.
        </p>
        <label
          htmlFor="max-parallel-per-user-input"
          className="flex items-start gap-2 text-xs text-gray-300"
        >
          <input
            id="max-parallel-per-user-input"
            type="checkbox"
            checked={draft.max_parallel_per_user}
            onChange={(e) => update({ max_parallel_per_user: e.target.checked })}
            className="mt-0.5 accent-blue-500"
          />
          <span>
            Apply the cap per user
            <span className="block text-gray-500 mt-0.5">
              When off, the cap is global across all users.
            </span>
          </span>
        </label>
      </section>

      {/* General limits — always shown; rides PUT /api/config/polling. */}
      <GeneralLimitsFields
        maxConcurrentManual={draft.max_concurrent_manual_workflows}
        prMergePollInterval={draft.pr_merge_poll_interval_secs}
        generateReport={draft.generate_report}
        workItemLogRetention={draft.work_item_log_retention_days}
        onMaxConcurrentManualChange={(v) => update({ max_concurrent_manual_workflows: v })}
        onPrMergePollIntervalChange={(v) => update({ pr_merge_poll_interval_secs: v })}
        onGenerateReportChange={(v) => update({ generate_report: v })}
        onWorkItemLogRetentionChange={(v) => update({ work_item_log_retention_days: v })}
      />

      {/* Jira filters — only when Jira is the active ticketing system */}
      {showJira && (
        <section className="flex flex-col gap-4">
          <h3 className="text-sm font-medium text-gray-300">Jira</h3>
          <ChipInput
            id="jira-item-types-input"
            label="Issue types"
            values={draft.item_types}
            onChange={(item_types) => update({ item_types })}
            placeholder="Bug, Task…"
            helpText="Issue types the poller pulls. Empty = no type filter."
          />
          <ChipInput
            id="jira-summary-keywords-input"
            label="Summary keywords"
            values={draft.jira_summary_keywords}
            onChange={(jira_summary_keywords) => update({ jira_summary_keywords })}
            placeholder="crash, regression…"
            helpText="Case-insensitive substring match (ANY). Empty = no filter."
          />
        </section>
      )}

      {/* Jira context — only when Jira is the active ticketing system. Saved
          via the separate PUT /api/config/jira endpoint. */}
      {showJira && (
        <JiraContextFields
          linkedItemsInPrompt={draft.linked_items_in_prompt}
          ticketContextMaxDescriptionBytes={draft.ticket_context_max_description_bytes}
          linkedIssueDescriptionMaxBytes={draft.linked_issue_description_max_bytes}
          jqlFilter={draft.jql_filter}
          doneStatus={draft.done_status}
          projectKeys={draft.project_keys}
          onLinkedItemsInPromptChange={(v) => update({ linked_items_in_prompt: v })}
          onTicketContextMaxDescriptionBytesChange={(v) =>
            update({ ticket_context_max_description_bytes: v })
          }
          onLinkedIssueDescriptionMaxBytesChange={(v) =>
            update({ linked_issue_description_max_bytes: v })
          }
          onJqlFilterChange={(v) => update({ jql_filter: v })}
          onDoneStatusChange={(v) => update({ done_status: v })}
          onProjectKeysChange={(v) => update({ project_keys: v })}
        />
      )}

      {/* GitHub filters — only when GitHub is the active ticketing system */}
      {showGithub && (
        <section className="flex flex-col gap-4">
          <h3 className="text-sm font-medium text-gray-300">GitHub</h3>
          <ChipInput
            id="github-labels-input"
            label="Labels"
            values={draft.github_labels}
            onChange={(github_labels) => update({ github_labels })}
            placeholder="bug, good first issue…"
            helpText="Exact label membership (ANY). Empty = no filter."
          />
          <ChipInput
            id="github-title-keywords-input"
            label="Title keywords"
            values={draft.github_title_keywords}
            onChange={(github_title_keywords) => update({ github_title_keywords })}
            placeholder="crash, regression…"
            helpText="Case-insensitive substring match (ANY). Empty = no filter."
          />
        </section>
      )}

      {/* No ticketing system → the poller is idle, so per-system filters don't apply */}
      {!showJira && !showGithub && (
        <p className="text-xs text-gray-500">
          No ticketing system is configured, so the poller is idle and item
          filters don&apos;t apply. Set <code className="text-gray-400">ticketing_system</code>{" "}
          to <code className="text-gray-400">jira</code> or{" "}
          <code className="text-gray-400">github</code> to filter polled items.
        </p>
      )}

      {/* Save */}
      <div className="flex justify-end">
        <button
          type="button"
          disabled={saving}
          onClick={onSave}
          className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
        >
          {saving ? "Saving…" : "Save changes"}
        </button>
      </div>
    </div>
  );
}

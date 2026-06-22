// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Pure form for the Item Polling settings section. The connected section owns
 * the fetch / save flow and feeds this component a draft plus the available
 * auto-start flows (CODING_STANDARDS §3 — fetch and render are split).
 */

import { Trans, useTranslation } from "react-i18next";
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
  /** Hide this form's own Save button — persisted by a page-level Save. */
  hideSave?: boolean;
}

export function ItemPollingForm({
  draft,
  onDraftChange,
  flows,
  ticketingSystem,
  onSave,
  saving,
  hideSave = false,
}: ItemPollingFormProps) {
  const { t } = useTranslation("config");
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
          <span className="text-sm text-gray-200">{t("polling.enable")}</span>
          <span className="text-xs text-gray-500">
            {t("polling.enableHelp")}
          </span>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={draft.auto_polling}
          aria-label={t("polling.enable")}
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
          {t("polling.interval")}
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
          <Trans
            i18nKey="polling.intervalHelp"
            ns="config"
            components={{ code: <code className="text-gray-400" /> }}
          />
        </p>
      </section>

      {/* Auto-start flow */}
      <section className="flex flex-col gap-2">
        <label htmlFor="auto-start-flow-select" className="text-xs text-gray-400">
          {t("polling.autoStartFlow")}
        </label>
        <select
          id="auto-start-flow-select"
          value={draft.auto_start_flow}
          onChange={(e) => update({ auto_start_flow: e.target.value })}
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
        >
          <option value="">{t("polling.autoStartAll")}</option>
          {flows.map((f) => (
            <option key={f.slug} value={f.slug}>
              {f.name}
            </option>
          ))}
          {selectedMissing && (
            <option value={draft.auto_start_flow}>
              {t("polling.autoStartMissing", { slug: draft.auto_start_flow })}
            </option>
          )}
        </select>
        <p className="text-xs text-gray-500">
          {t("polling.autoStartHelp")}
        </p>
      </section>

      {/* Parallel-item cap */}
      <section className="flex flex-col gap-2">
        <label htmlFor="max-parallel-items-input" className="text-xs text-gray-400">
          {t("polling.maxParallel")}
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
          <Trans
            i18nKey="polling.maxParallelHelp"
            ns="config"
            components={{ code: <code className="text-gray-400" /> }}
          />
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
            {t("polling.perUser")}
            <span className="block text-gray-500 mt-0.5">
              {t("polling.perUserHelp")}
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
          <h3 className="text-sm font-medium text-gray-300">{t("polling.jira")}</h3>
          <ChipInput
            id="jira-item-types-input"
            label={t("polling.issueTypes")}
            values={draft.item_types}
            onChange={(item_types) => update({ item_types })}
            placeholder={t("polling.issueTypesPlaceholder")}
            helpText={t("polling.issueTypesHelp")}
          />
          <ChipInput
            id="jira-summary-keywords-input"
            label={t("polling.summaryKeywords")}
            values={draft.jira_summary_keywords}
            onChange={(jira_summary_keywords) => update({ jira_summary_keywords })}
            placeholder={t("polling.summaryKeywordsPlaceholder")}
            helpText={t("polling.keywordsHelp")}
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
          <h3 className="text-sm font-medium text-gray-300">{t("polling.github")}</h3>
          <ChipInput
            id="github-labels-input"
            label={t("polling.labels")}
            values={draft.github_labels}
            onChange={(github_labels) => update({ github_labels })}
            placeholder={t("polling.labelsPlaceholder")}
            helpText={t("polling.labelsHelp")}
          />
          <ChipInput
            id="github-title-keywords-input"
            label={t("polling.titleKeywords")}
            values={draft.github_title_keywords}
            onChange={(github_title_keywords) => update({ github_title_keywords })}
            placeholder={t("polling.titleKeywordsPlaceholder")}
            helpText={t("polling.keywordsHelp")}
          />
        </section>
      )}

      {/* No ticketing system → the poller is idle, so per-system filters don't apply */}
      {!showJira && !showGithub && (
        <p className="text-xs text-gray-500">
          {t("polling.noTicketing")}
        </p>
      )}

      {/* Save */}
      {!hideSave && (
        <div className="flex justify-end">
          <button
            type="button"
            disabled={saving}
            onClick={onSave}
            className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
          >
            {saving ? t("actions.saving") : t("actions.saveChanges")}
          </button>
        </div>
      )}
    </div>
  );
}

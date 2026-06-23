// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Pure form for the Item Polling settings section. The connected section owns
 * the fetch / save flow and feeds this component a draft plus the available
 * auto-start flows (CODING_STANDARDS §3 — fetch and render are split).
 */

import { Trans, useTranslation } from "react-i18next";
import { ChipInput } from "./ChipInput";

/** A flow the auto-start <select> can target: stable slug + display name. */
export interface AutoStartFlowOption {
  slug: string;
  name: string;
}

/**
 * Editable per-repository polling state. Numeric inputs are strings so they
 * stay controlled. The deployment-global "general limits" are NOT here — they
 * live in their own global admin section (`GeneralLimitsSection`).
 */
export interface ItemPollingDraft {
  auto_polling: boolean;
  auto_start_flow: string;
  max_parallel_items: string;
  item_types: string[];
  jira_summary_keywords: string[];
  /** Jira project keys the poller pulls from for this repository. */
  project_keys: string[];
  github_labels: string[];
  github_title_keywords: string[];
  /** Extra JQL appended to the poll/manual query for this repository. The
   *  other Jira-context "processing" fields are deployment-global (see
   *  GlobalJiraContextSection), so they are NOT here. */
  jql_filter: string;
}

export const EMPTY_POLLING_DRAFT: ItemPollingDraft = {
  auto_polling: false,
  auto_start_flow: "",
  max_parallel_items: "0",
  item_types: [],
  jira_summary_keywords: [],
  project_keys: [],
  github_labels: [],
  github_title_keywords: [],
  jql_filter: "",
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

      {/* Jira project keys — ALWAYS shown for Jira, independent of the enable
          toggle: they map this repository to its Jira projects and are required
          by the manual "Add item" picker, not just auto-polling. */}
      {showJira && (
        <ChipInput
          id="jira-project-keys-input"
          label={t("polling.projectKeys")}
          values={draft.project_keys}
          onChange={(project_keys) => update({ project_keys })}
          placeholder={t("polling.projectKeysPlaceholder")}
          helpText={t("polling.projectKeysHelp")}
        />
      )}

      {/* The rest of the settings only apply while polling is enabled. */}
      {draft.auto_polling && (
        <>
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
      </section>

      {/* Jira filters — only when Jira is the active ticketing system. Project
          keys are rendered above (always visible); these auto-polling filters
          stay behind the enable toggle. */}
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
          <div className="flex flex-col gap-2">
            <label htmlFor="jql-filter-input" className="text-xs text-gray-400">
              {t("polling.jiraContext.jqlFilter")}
            </label>
            <input
              id="jql-filter-input"
              type="text"
              value={draft.jql_filter}
              onChange={(e) => update({ jql_filter: e.target.value })}
              placeholder={t("polling.jiraContext.jqlPlaceholder")}
              className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
            />
            <p className="text-xs text-gray-500">{t("polling.jiraContext.jqlHelp")}</p>
          </div>
        </section>
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
        </>
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

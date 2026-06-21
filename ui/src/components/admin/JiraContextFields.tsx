// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * "Jira context" subsection of the Item Polling form. Pure presentational
 * fields for the Jira-context portion of the `[jira]` section, saved via
 * `PUT /api/config/jira`. Only rendered when Jira is the active ticketing
 * system. Extracted from `ItemPollingForm` so each subsection owns one file
 * (CODING_STANDARDS §1/§3).
 */

import { Trans, useTranslation } from "react-i18next";
import type { LinkedItemsInPrompt } from "../../api/types";
import { ChipInput } from "./ChipInput";

interface JiraContextFieldsProps {
  linkedItemsInPrompt: LinkedItemsInPrompt;
  ticketContextMaxDescriptionBytes: string;
  linkedIssueDescriptionMaxBytes: string;
  jqlFilter: string;
  doneStatus: string;
  projectKeys: string[];
  onLinkedItemsInPromptChange: (value: LinkedItemsInPrompt) => void;
  onTicketContextMaxDescriptionBytesChange: (value: string) => void;
  onLinkedIssueDescriptionMaxBytesChange: (value: string) => void;
  onJqlFilterChange: (value: string) => void;
  onDoneStatusChange: (value: string) => void;
  onProjectKeysChange: (value: string[]) => void;
}

export function JiraContextFields({
  linkedItemsInPrompt,
  ticketContextMaxDescriptionBytes,
  linkedIssueDescriptionMaxBytes,
  jqlFilter,
  doneStatus,
  projectKeys,
  onLinkedItemsInPromptChange,
  onTicketContextMaxDescriptionBytesChange,
  onLinkedIssueDescriptionMaxBytesChange,
  onJqlFilterChange,
  onDoneStatusChange,
  onProjectKeysChange,
}: JiraContextFieldsProps) {
  const { t } = useTranslation("config");
  return (
    <section className="flex flex-col gap-4">
      <h3 className="text-sm font-medium text-gray-300">{t("polling.jiraContext.title")}</h3>

      <div className="flex flex-col gap-2">
        <label htmlFor="linked-items-in-prompt-select" className="text-xs text-gray-400">
          {t("polling.jiraContext.linkedItems")}
        </label>
        <select
          id="linked-items-in-prompt-select"
          value={linkedItemsInPrompt}
          onChange={(e) =>
            onLinkedItemsInPromptChange(e.target.value as LinkedItemsInPrompt)
          }
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
        >
          <option value="full">{t("polling.jiraContext.linkedFull")}</option>
          <option value="summary_only">{t("polling.jiraContext.linkedSummary")}</option>
          <option value="omit">{t("polling.jiraContext.linkedOmit")}</option>
        </select>
        <p className="text-xs text-gray-500">
          <Trans
            i18nKey="polling.jiraContext.linkedItemsHelp"
            ns="config"
            values={{ token: "{ticket_context}" }}
            components={{ code: <code className="text-gray-400" /> }}
          />
        </p>
      </div>

      <div className="flex flex-col gap-2">
        <label
          htmlFor="ticket-context-max-description-bytes-input"
          className="text-xs text-gray-400"
        >
          {t("polling.jiraContext.ticketCap")}
        </label>
        <input
          id="ticket-context-max-description-bytes-input"
          type="number"
          min={0}
          value={ticketContextMaxDescriptionBytes}
          onChange={(e) => onTicketContextMaxDescriptionBytesChange(e.target.value)}
          placeholder="0"
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        <p className="text-xs text-gray-500">
          <Trans
            i18nKey="polling.jiraContext.ticketCapHelp"
            ns="config"
            components={{ code: <code className="text-gray-400" /> }}
          />
        </p>
      </div>

      <div className="flex flex-col gap-2">
        <label
          htmlFor="linked-issue-description-max-bytes-input"
          className="text-xs text-gray-400"
        >
          {t("polling.jiraContext.linkedCap")}
        </label>
        <input
          id="linked-issue-description-max-bytes-input"
          type="number"
          min={0}
          value={linkedIssueDescriptionMaxBytes}
          onChange={(e) => onLinkedIssueDescriptionMaxBytesChange(e.target.value)}
          placeholder="0"
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        <p className="text-xs text-gray-500">
          <Trans
            i18nKey="polling.jiraContext.linkedCapHelp"
            ns="config"
            components={{ code: <code className="text-gray-400" /> }}
          />
        </p>
      </div>

      <div className="flex flex-col gap-2">
        <label htmlFor="jql-filter-input" className="text-xs text-gray-400">
          {t("polling.jiraContext.jqlFilter")}
        </label>
        <input
          id="jql-filter-input"
          type="text"
          value={jqlFilter}
          onChange={(e) => onJqlFilterChange(e.target.value)}
          placeholder={t("polling.jiraContext.jqlPlaceholder")}
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        <p className="text-xs text-gray-500">
          {t("polling.jiraContext.jqlHelp")}
        </p>
      </div>

      <div className="flex flex-col gap-2">
        <label htmlFor="done-status-input" className="text-xs text-gray-400">
          {t("polling.jiraContext.doneStatus")}
        </label>
        <input
          id="done-status-input"
          type="text"
          value={doneStatus}
          onChange={(e) => onDoneStatusChange(e.target.value)}
          placeholder="Done"
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        <p className="text-xs text-gray-500">
          {t("polling.jiraContext.doneStatusHelp")}
        </p>
      </div>

      <ChipInput
        id="jira-project-keys-input"
        label={t("polling.jiraContext.projectKeys")}
        values={projectKeys}
        onChange={onProjectKeysChange}
        placeholder={t("polling.jiraContext.projectKeysPlaceholder")}
        helpText={t("polling.jiraContext.projectKeysHelp")}
      />
    </section>
  );
}

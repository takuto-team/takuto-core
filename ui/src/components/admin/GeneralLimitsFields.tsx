// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * "General limits" subsection of the Item Polling form. Pure presentational
 * fields for the four `[general]` runtime limits that ride the existing
 * `PUT /api/config/polling` endpoint. Extracted from `ItemPollingForm` so each
 * subsection owns one file (CODING_STANDARDS §1/§3).
 */

import { Trans, useTranslation } from "react-i18next";

interface GeneralLimitsFieldsProps {
  pollInterval: string;
  maxParallelPerUser: boolean;
  maxConcurrentManual: string;
  prMergePollInterval: string;
  generateReport: boolean;
  workItemLogRetention: string;
  onPollIntervalChange: (value: string) => void;
  onMaxParallelPerUserChange: (value: boolean) => void;
  onMaxConcurrentManualChange: (value: string) => void;
  onPrMergePollIntervalChange: (value: string) => void;
  onGenerateReportChange: (value: boolean) => void;
  onWorkItemLogRetentionChange: (value: string) => void;
}

export function GeneralLimitsFields({
  pollInterval,
  maxParallelPerUser,
  maxConcurrentManual,
  prMergePollInterval,
  generateReport,
  workItemLogRetention,
  onPollIntervalChange,
  onMaxParallelPerUserChange,
  onMaxConcurrentManualChange,
  onPrMergePollIntervalChange,
  onGenerateReportChange,
  onWorkItemLogRetentionChange,
}: GeneralLimitsFieldsProps) {
  const { t } = useTranslation("config");
  return (
    <section className="flex flex-col gap-4">
      <h3 className="text-sm font-medium text-gray-300">{t("polling.general.title")}</h3>

      <div className="flex flex-col gap-2">
        <label htmlFor="poll-interval-input" className="text-xs text-gray-400">
          {t("polling.interval")}
        </label>
        <input
          id="poll-interval-input"
          type="number"
          min={10}
          value={pollInterval}
          onChange={(e) => onPollIntervalChange(e.target.value)}
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
      </div>

      <label
        htmlFor="max-parallel-per-user-input"
        className="flex items-start gap-2 text-xs text-gray-300"
      >
        <input
          id="max-parallel-per-user-input"
          type="checkbox"
          checked={maxParallelPerUser}
          onChange={(e) => onMaxParallelPerUserChange(e.target.checked)}
          className="mt-0.5 accent-blue-500"
        />
        <span>
          {t("polling.perUser")}
          <span className="block text-gray-500 mt-0.5">{t("polling.perUserHelp")}</span>
        </span>
      </label>

      <div className="flex flex-col gap-2">
        <label htmlFor="max-concurrent-manual-input" className="text-xs text-gray-400">
          {t("polling.general.maxConcurrentManual")}
        </label>
        <input
          id="max-concurrent-manual-input"
          type="number"
          min={0}
          value={maxConcurrentManual}
          onChange={(e) => onMaxConcurrentManualChange(e.target.value)}
          placeholder="0"
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        <p className="text-xs text-gray-500">
          <Trans
            i18nKey="polling.general.maxConcurrentManualHelp"
            ns="config"
            components={{ code: <code className="text-gray-400" /> }}
          />
        </p>
      </div>

      <div className="flex flex-col gap-2">
        <label htmlFor="pr-merge-poll-interval-input" className="text-xs text-gray-400">
          {t("polling.general.prMergeInterval")}
        </label>
        <input
          id="pr-merge-poll-interval-input"
          type="number"
          min={1}
          value={prMergePollInterval}
          onChange={(e) => onPrMergePollIntervalChange(e.target.value)}
          placeholder={t("ai.guardrails.defaultPlaceholder")}
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        <p className="text-xs text-gray-500">
          {t("polling.general.prMergeIntervalHelp")}
        </p>
      </div>

      <div className="flex flex-col gap-2">
        <label htmlFor="work-item-log-retention-input" className="text-xs text-gray-400">
          {t("polling.general.logRetention")}
        </label>
        <input
          id="work-item-log-retention-input"
          type="number"
          min={0}
          value={workItemLogRetention}
          onChange={(e) => onWorkItemLogRetentionChange(e.target.value)}
          placeholder="0"
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        <p className="text-xs text-gray-500">
          <Trans
            i18nKey="polling.general.logRetentionHelp"
            ns="config"
            components={{ code: <code className="text-gray-400" /> }}
          />
        </p>
      </div>

      <div className="flex items-start justify-between gap-4">
        <div className="flex flex-col gap-0.5">
          <span className="text-sm text-gray-200">{t("polling.general.generateReport")}</span>
          <span className="text-xs text-gray-500">
            {t("polling.general.generateReportHelp")}
          </span>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={generateReport}
          aria-label={t("polling.general.generateReport")}
          onClick={() => onGenerateReportChange(!generateReport)}
          className={`relative inline-flex h-7 w-12 flex-shrink-0 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500/50 cursor-pointer ${
            generateReport ? "bg-blue-600" : "bg-gray-700"
          }`}
        >
          <span
            className={`inline-block h-5 w-5 transform rounded-full bg-white transition-transform ${
              generateReport ? "translate-x-6" : "translate-x-1"
            }`}
          />
        </button>
      </div>
    </section>
  );
}

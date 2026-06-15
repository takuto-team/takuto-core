// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * "General limits" subsection of the Item Polling form. Pure presentational
 * fields for the four `[general]` runtime limits that ride the existing
 * `PUT /api/config/polling` endpoint. Extracted from `ItemPollingForm` so each
 * subsection owns one file (CODING_STANDARDS §1/§3).
 */

interface GeneralLimitsFieldsProps {
  maxConcurrentManual: string;
  prMergePollInterval: string;
  generateReport: boolean;
  workItemLogRetention: string;
  onMaxConcurrentManualChange: (value: string) => void;
  onPrMergePollIntervalChange: (value: string) => void;
  onGenerateReportChange: (value: boolean) => void;
  onWorkItemLogRetentionChange: (value: string) => void;
}

export function GeneralLimitsFields({
  maxConcurrentManual,
  prMergePollInterval,
  generateReport,
  workItemLogRetention,
  onMaxConcurrentManualChange,
  onPrMergePollIntervalChange,
  onGenerateReportChange,
  onWorkItemLogRetentionChange,
}: GeneralLimitsFieldsProps) {
  return (
    <section className="flex flex-col gap-4">
      <h3 className="text-sm font-medium text-gray-300">General limits</h3>

      <div className="flex flex-col gap-2">
        <label htmlFor="max-concurrent-manual-input" className="text-xs text-gray-400">
          Max concurrent manual workflows
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
          Manual starts that may occupy a slot at once.{" "}
          <code className="text-gray-400">0</code> = unlimited.
        </p>
      </div>

      <div className="flex flex-col gap-2">
        <label htmlFor="pr-merge-poll-interval-input" className="text-xs text-gray-400">
          PR-merge poll interval (seconds)
        </label>
        <input
          id="pr-merge-poll-interval-input"
          type="number"
          min={1}
          value={prMergePollInterval}
          onChange={(e) => onPrMergePollIntervalChange(e.target.value)}
          placeholder="Leave empty for the default"
          className="bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
        <p className="text-xs text-gray-500">
          How often Takuto checks GitHub for whether a workflow&apos;s PR has
          been merged.
        </p>
      </div>

      <div className="flex flex-col gap-2">
        <label htmlFor="work-item-log-retention-input" className="text-xs text-gray-400">
          Work-item log retention (days)
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
          Days of per-work-item logs to keep.{" "}
          <code className="text-gray-400">0</code> = keep forever.
        </p>
      </div>

      <div className="flex items-start justify-between gap-4">
        <div className="flex flex-col gap-0.5">
          <span className="text-sm text-gray-200">Generate run report</span>
          <span className="text-xs text-gray-500">
            Default for whether each workflow produces an end-of-run report.
          </span>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={generateReport}
          aria-label="Generate run report"
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

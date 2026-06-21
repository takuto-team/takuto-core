// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Presentational per-workspace "generate work-item reports" switch.
 *
 * Controlled: the parent owns the boolean and persists it (in the settings
 * tab it rides the worktree-commands PUT; in the wizard it persists for the
 * chosen workspace). This component renders only the label, explanation, and
 * switch — no fetching, no saving.
 */

import { Trans, useTranslation } from "react-i18next";

interface GenerateReportToggleProps {
  value: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  /** When true, a transient green "Saved" appears directly beneath the switch. */
  saved?: boolean;
}

export function GenerateReportToggle({
  value,
  onChange,
  disabled,
  saved,
}: GenerateReportToggleProps) {
  const { t } = useTranslation("config");
  return (
    <section className="flex items-start justify-between gap-6 border border-gray-800 rounded-lg bg-gray-900/40 p-4">
      <div className="min-w-0">
        <h4 className="text-sm font-semibold text-gray-200">{t("repositories.report.title")}</h4>
        <p className="text-xs text-gray-500 mt-1 max-w-2xl">
          <Trans
            i18nKey="repositories.report.help"
            ns="config"
            values={{ file: "lore/reports/<key>_report.md" }}
            components={{ mono: <span className="font-mono" /> }}
          />
        </p>
      </div>

      <div className="flex flex-col items-end gap-1 flex-shrink-0">
        <button
          type="button"
          role="switch"
          aria-checked={value}
          aria-label={t("repositories.report.title")}
          disabled={disabled}
          onClick={() => onChange(!value)}
          className={`relative inline-flex h-7 w-12 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500/50 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer ${
            value ? "bg-blue-600" : "bg-gray-700"
          }`}
        >
          <span
            className={`inline-block h-5 w-5 transform rounded-full bg-white transition-transform ${
              value ? "translate-x-6" : "translate-x-1"
            }`}
          />
        </button>
        {saved && <span className="text-xs text-green-400">{t("repositories.report.saved")}</span>}
      </div>
    </section>
  );
}

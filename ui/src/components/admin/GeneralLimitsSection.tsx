// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Admin-only, deployment-global "General limits" section.
 *
 * The four `[general]` runtime limits — max concurrent manual workflows, the
 * PR-merge poll interval, the generate-report default, and work-item log
 * retention — that ride the trimmed `PUT /api/config/polling`. These are
 * deployment-wide (NOT per repository); the per-repo polling knobs live in
 * `RepoPollingSettingsSection`.
 *
 * Exposes the shared `ConfigSectionHandle` ({ isDirty, save }) via `forwardRef`
 * so a parent (the Ticketing tab footer, or the onboarding wizard) can drive a
 * single Save, and reports dirtiness via `onDirtyChange`.
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
import {
  ItemPollingConfigError,
  putItemPollingConfig,
} from "../../api/itemPollingConfig";
import { useToast } from "../../hooks/useToast";
import type { ConfigResponse } from "../../api/types";
import type { ConfigSectionHandle } from "./configSection";
import { GeneralLimitsFields } from "./GeneralLimitsFields";

/** Editable form state. Numeric inputs are strings so they stay controlled. */
interface GeneralLimitsDraft {
  poll_interval_secs: string;
  max_parallel_per_user: boolean;
  max_concurrent_manual_workflows: string;
  pr_merge_poll_interval_secs: string;
  generate_report: boolean;
  work_item_log_retention_days: string;
}

const EMPTY_DRAFT: GeneralLimitsDraft = {
  poll_interval_secs: "60",
  max_parallel_per_user: false,
  max_concurrent_manual_workflows: "0",
  pr_merge_poll_interval_secs: "",
  generate_report: false,
  work_item_log_retention_days: "0",
};

/** Stringify a numeric config field, falling back to `fallback` when absent. */
function numToStr(value: unknown, fallback: string): string {
  return typeof value === "number" ? String(value) : fallback;
}

function draftFromConfig(config: ConfigResponse): GeneralLimitsDraft {
  const general = config.general ?? {};
  return {
    poll_interval_secs: numToStr(general.poll_interval_secs, "60"),
    max_parallel_per_user: config.polling?.max_parallel_per_user === true,
    max_concurrent_manual_workflows: numToStr(general.max_concurrent_manual_workflows, "0"),
    pr_merge_poll_interval_secs: numToStr(general.pr_merge_poll_interval_secs, ""),
    generate_report: general.generate_report === true,
    work_item_log_retention_days: numToStr(general.work_item_log_retention_days, "0"),
  };
}

/** Parse a "0 = unlimited / forever" input; empty / negative / non-numeric → 0. */
function parseNonNegInt(raw: string): number {
  const n = Number.parseInt(raw.trim(), 10);
  return Number.isFinite(n) && n > 0 ? n : 0;
}

/** Parse a positive-only input; non-positive / non-numeric → undefined (omit). */
function parsePositiveOrOmit(raw: string): number | undefined {
  const n = Number.parseInt(raw.trim(), 10);
  return Number.isFinite(n) && n > 0 ? n : undefined;
}

interface Props {
  /** Reports unsaved edits so a parent page-level Save can fold this section in. */
  onDirtyChange?: (dirty: boolean) => void;
}

export const GeneralLimitsSection = forwardRef<ConfigSectionHandle, Props>(
  function GeneralLimitsSection({ onDirtyChange }, ref) {
    const { t } = useTranslation("config");
    const { showToast } = useToast();
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState("");
    const [draft, setDraft] = useState<GeneralLimitsDraft>(EMPTY_DRAFT);
    const [original, setOriginal] = useState<GeneralLimitsDraft>(EMPTY_DRAFT);

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
      try {
        const updated = await putItemPollingConfig({
          poll_interval_secs: parsePositiveOrOmit(draft.poll_interval_secs),
          max_parallel_per_user: draft.max_parallel_per_user,
          max_concurrent_manual_workflows: parseNonNegInt(draft.max_concurrent_manual_workflows),
          pr_merge_poll_interval_secs: parsePositiveOrOmit(draft.pr_merge_poll_interval_secs),
          generate_report: draft.generate_report,
          work_item_log_retention_days: parseNonNegInt(draft.work_item_log_retention_days),
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
        if (e instanceof ItemPollingConfigError) {
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

    const update = (patch: Partial<GeneralLimitsDraft>) => setDraft((d) => ({ ...d, ...patch }));

    return (
      <section aria-labelledby="general-limits-title" className="flex flex-col gap-3">
        <h2 id="general-limits-title" className="text-lg font-semibold text-white">
          {t("polling.general.sectionTitle")}
        </h2>
        <p className="text-xs text-gray-500">{t("polling.general.sectionHelp")}</p>

        {loading && <p className="text-sm text-gray-500">{t("actions.loading")}</p>}
        {!loading && error && (
          <p className="text-sm text-red-400">{t("errors.loadConfig", { error })}</p>
        )}
        {!loading && !error && (
          <div className="bg-gray-900 border border-gray-800 rounded-xl p-6">
            <GeneralLimitsFields
              pollInterval={draft.poll_interval_secs}
              maxParallelPerUser={draft.max_parallel_per_user}
              maxConcurrentManual={draft.max_concurrent_manual_workflows}
              prMergePollInterval={draft.pr_merge_poll_interval_secs}
              generateReport={draft.generate_report}
              workItemLogRetention={draft.work_item_log_retention_days}
              onPollIntervalChange={(v) => update({ poll_interval_secs: v })}
              onMaxParallelPerUserChange={(v) => update({ max_parallel_per_user: v })}
              onMaxConcurrentManualChange={(v) => update({ max_concurrent_manual_workflows: v })}
              onPrMergePollIntervalChange={(v) => update({ pr_merge_poll_interval_secs: v })}
              onGenerateReportChange={(v) => update({ generate_report: v })}
              onWorkItemLogRetentionChange={(v) => update({ work_item_log_retention_days: v })}
            />
          </div>
        )}
      </section>
    );
  },
);

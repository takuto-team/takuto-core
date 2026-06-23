// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Per-user-per-repository item-polling section.
 *
 * A `RepoSidebar` on the left selects the repository; the right pane is the
 * existing `ItemPollingForm` (enable toggle, auto-start flow, parallel caps,
 * Jira/GitHub filters, Jira project keys, Jira context) loaded and saved per
 * repository via `/api/me/polling-settings/{workspace}`. The deployment-global
 * "general limits" are NOT here — they live in `GeneralLimitsSection`.
 *
 * Exposes the shared `ConfigSectionHandle` ({ isDirty, save }) via `forwardRef`
 * so the parent Ticketing tab can fold this into a single page-level Save, and
 * reports dirtiness via `onDirtyChange`.
 */

import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import {
  getMyPollingSettings,
  listMyPollingSettings,
  putMyPollingSettings,
  type RepoPollingSettings,
  type RepoPollingSettingsInput,
} from "../../api/client";
import { getMyFlows, slugify } from "../../api/flows";
import { useMyRepositories } from "../../hooks/useMyRepositories";
import { useRepoAccess } from "../../hooks/useRepoAccess";
import { pickDefaultRepo } from "../../utils/pickDefaultRepo";
import { RepoSidebar, type RepoSidebarItem } from "../RepoSidebar";
import type { ConfigSectionHandle } from "./configSection";
import {
  EMPTY_POLLING_DRAFT,
  ItemPollingForm,
  type AutoStartFlowOption,
  type ItemPollingDraft,
} from "./ItemPollingForm";

/** Parse a "0 = unlimited / forever" input; empty / negative / non-numeric → 0. */
function parseNonNegInt(raw: string): number {
  const n = Number.parseInt(raw.trim(), 10);
  return Number.isFinite(n) && n > 0 ? n : 0;
}

/** Trim and drop blank entries — the validator rejects whitespace-only items. */
function cleanList(values: string[]): string[] {
  return values.map((v) => v.trim()).filter((v) => v.length > 0);
}

function settingsToDraft(s: RepoPollingSettings): ItemPollingDraft {
  return {
    auto_polling: s.auto_polling,
    auto_start_flow: s.auto_start_flow,
    max_parallel_items: String(s.max_parallel_items),
    item_types: s.item_types,
    jira_summary_keywords: s.jira_summary_keywords,
    project_keys: s.project_keys,
    github_labels: s.github_labels,
    github_title_keywords: s.github_title_keywords,
    jql_filter: s.jql_filter,
  };
}

function draftToSettings(d: ItemPollingDraft): RepoPollingSettingsInput {
  return {
    auto_polling: d.auto_polling,
    auto_start_flow: d.auto_start_flow,
    max_parallel_items: parseNonNegInt(d.max_parallel_items),
    project_keys: cleanList(d.project_keys),
    item_types: cleanList(d.item_types),
    jira_summary_keywords: cleanList(d.jira_summary_keywords),
    github_labels: cleanList(d.github_labels),
    github_title_keywords: cleanList(d.github_title_keywords),
    jql_filter: d.jql_filter.trim(),
  };
}

interface Props {
  /** Active ticketing system ("jira" | "github" | "none"); decides which filter block shows. */
  ticketingSystem: string;
  /** Reports unsaved edits so the parent's footer Save enables. */
  onDirtyChange?: (dirty: boolean) => void;
}

export const RepoPollingSettingsSection = forwardRef<ConfigSectionHandle, Props>(
  function RepoPollingSettingsSection({ ticketingSystem, onDirtyChange }, ref) {
    const { t } = useTranslation("config");
    const { myRepos, activeRepoName } = useMyRepositories();
    const { access } = useRepoAccess();
    // Names of repos the caller has a settings row for (badge source).
    const [withSettings, setWithSettings] = useState<Set<string>>(new Set());

    const [selected, setSelected] = useState<string | null>(null);
    const [loadingEditor, setLoadingEditor] = useState(false);
    const [saving, setSaving] = useState(false);
    const [error, setError] = useState("");
    const [success, setSuccess] = useState("");
    const [draft, setDraft] = useState<ItemPollingDraft>(EMPTY_POLLING_DRAFT);
    const [original, setOriginal] = useState<ItemPollingDraft>(EMPTY_POLLING_DRAFT);
    const [flows, setFlows] = useState<AutoStartFlowOption[]>([]);

    // Best-effort badge data: which repos already have a row.
    useEffect(() => {
      let cancelled = false;
      listMyPollingSettings()
        .then((rows) => {
          if (!cancelled) setWithSettings(new Set(rows.map((r) => r.workspace_name)));
        })
        .catch(() => {
          /* badge is non-essential; ignore */
        });
      return () => {
        cancelled = true;
      };
    }, []);

    const loadWorkspace = useCallback((name: string) => {
      setSelected(name);
      setError("");
      setSuccess("");
      setLoadingEditor(true);
      Promise.all([getMyPollingSettings(name), getMyFlows(name)])
        .then(([row, flowsResponse]) => {
          const seeded = row ? settingsToDraft(row.settings) : EMPTY_POLLING_DRAFT;
          setDraft(seeded);
          setOriginal(seeded);
          setFlows(flowsResponse.flows.map((f) => ({ slug: slugify(f.name), name: f.name })));
        })
        .catch((e) => setError(String((e as Error).message || e)))
        .finally(() => setLoadingEditor(false));
    }, []);

    // Default selection: the active repo if accessible, else the first
    // accessible (falling back to the first), once the list loads.
    const loadingRepos = myRepos === null;
    useEffect(() => {
      if (myRepos === null || selected !== null) return;
      const def = pickDefaultRepo(
        myRepos.map((r) => r.name),
        activeRepoName,
        access,
      );
      if (def) loadWorkspace(def);
    }, [myRepos, activeRepoName, access, selected, loadWorkspace]);

    const repos: RepoSidebarItem[] = useMemo(
      () =>
        (myRepos ?? []).map((r) => ({
          name: r.name,
          hasCommands: withSettings.has(r.name),
          accessible: access[r.name] !== false,
        })),
      [myRepos, withSettings, access],
    );

    const dirty = JSON.stringify(draft) !== JSON.stringify(original);

    const handleSave = useCallback(async (): Promise<boolean> => {
      if (!selected || !dirty) return true;
      setError("");
      setSuccess("");
      setSaving(true);
      try {
        const row = await putMyPollingSettings(selected, draftToSettings(draft));
        const fresh = settingsToDraft(row.settings);
        setDraft(fresh);
        setOriginal(fresh);
        setSuccess(t("polling.savedToast"));
        setWithSettings((prev) => new Set(prev).add(selected));
        return true;
      } catch (e) {
        setError(String((e as Error).message || e));
        return false;
      } finally {
        setSaving(false);
      }
    }, [selected, dirty, draft, t]);

    // Fold into the page-level Save footer / parent tab handle.
    useEffect(() => {
      onDirtyChange?.(dirty);
    }, [dirty, onDirtyChange]);
    useImperativeHandle(ref, () => ({ isDirty: () => dirty, save: handleSave }), [dirty, handleSave]);

    return (
      <section aria-labelledby="repo-polling-title" className="flex flex-col gap-3">
        <header>
          <h2 id="repo-polling-title" className="text-lg font-semibold text-white">
            {t("polling.title")}
          </h2>
          <p className="text-sm text-gray-500 mt-1">{t("polling.repoHelp")}</p>
        </header>

        <div className="flex flex-col md:flex-row gap-4 min-h-[24rem]">
          <RepoSidebar
            repos={repos}
            loading={loadingRepos}
            selected={selected}
            onSelect={loadWorkspace}
          />

          <section className="flex-1 min-w-0">
            {!selected ? (
              <div className="h-full flex items-center justify-center text-sm text-gray-500 italic min-h-[16rem] border border-gray-800 rounded-lg bg-gray-950 p-4">
                {t("polling.selectRepo")}
              </div>
            ) : loadingEditor ? (
              <p className="text-sm text-gray-500">{t("actions.loading")}</p>
            ) : (
              <div className="space-y-3">
                <h3 className="text-base font-semibold text-gray-200">
                  {t("worktreeSettings.repositoryLabel")}{" "}
                  <span className="font-mono">{selected}</span>
                </h3>
                <ItemPollingForm
                  draft={draft}
                  onDraftChange={setDraft}
                  flows={flows}
                  ticketingSystem={ticketingSystem}
                  onSave={() => void handleSave()}
                  saving={saving}
                  hideSave
                />
                {error && <p className="text-sm text-red-400">{error}</p>}
                {success && <p className="text-sm text-green-400">{success}</p>}
              </div>
            )}
          </section>
        </div>
      </section>
    );
  },
);

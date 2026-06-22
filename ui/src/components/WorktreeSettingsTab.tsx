// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Per-user-per-repository Repository Settings tab.
 *
 * - No admin gate, no global default. Each authenticated user manages their
 *   own rows; admins have no special access here.
 * - The repo list and the active-repo default come from `useMyRepositories`
 *   (shared with the dashboard header). The sidebar's "set"/"none" badge is
 *   derived from `listMyWorktreeCommands` (which repos the caller has a row
 *   for). The selected repo's name IS the workspace key for the per-repo row.
 * - A row is either present (with init + run commands) or absent. Single PUT
 *   atomically updates init_commands, run_commands, and the preserved
 *   `generate_report` flag (edited on the Workflows tab).
 *
 * Diff-aware editing state lives in `useDiffForm`; the pure validator is in
 * `./WorktreeSettings/validateCommands`. This file owns the page layout, the
 * default-selection + load-on-select effects, and the Save / Delete flows.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  deleteMyWorktreeCommands,
  getMyWorktreeCommands,
  listMyWorktreeCommands,
  putMyWorktreeCommands,
  type RunCommand,
} from "../api/client";
import { useDiffForm } from "../hooks/useDiffForm";
import { useMyRepositories } from "../hooks/useMyRepositories";
import { useRepoAccess } from "../hooks/useRepoAccess";
import { pickDefaultRepo } from "../utils/pickDefaultRepo";
import { ConfirmModal } from "./modals/ConfirmModal";
import { RepoSidebar, type RepoSidebarItem } from "./RepoSidebar";
import { WorktreeCommandList } from "./WorktreeCommandList";
import { WorktreeRunCommandList } from "./WorktreeRunCommandList";
import { validateCommands } from "./WorktreeSettings/validateCommands";

interface Props {
  /** Reports unsaved init/run command edits so Config's footer enables. */
  onDirtyChange?: (dirty: boolean) => void;
  /** Registers the per-repo save fn so the page-level Save can drive it. */
  registerSave?: (save: () => Promise<boolean>) => void;
}

export function WorktreeSettingsTab({ onDirtyChange, registerSave }: Props = {}) {
  const { t } = useTranslation("config");
  const { myRepos, activeRepoName } = useMyRepositories();
  const { access } = useRepoAccess();
  // Names of repos the caller has a worktree-commands row for (badge source).
  const [withCommands, setWithCommands] = useState<Set<string>>(new Set());

  const [selected, setSelected] = useState<string | null>(null);
  const [hasRow, setHasRow] = useState(false);
  const [loadingEditor, setLoadingEditor] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);

  const init = useDiffForm<string[]>([]);
  const run = useDiffForm<RunCommand[]>([]);
  // Preserved (not edited here): the per-repository report toggle lives on the
  // Workflows tab. We still load it and write it back unchanged so saving init
  // /run commands never silently resets it.
  const [generateReport, setGenerateReport] = useState(false);

  // Best-effort badge data: which repos already have a row.
  useEffect(() => {
    let cancelled = false;
    listMyWorktreeCommands()
      .then((rows) => {
        if (!cancelled) setWithCommands(new Set(rows.map((r) => r.workspace_name)));
      })
      .catch(() => {
        /* badge is non-essential; ignore */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const loadWorkspace = useCallback(
    (name: string) => {
      setSelected(name);
      setError("");
      setSuccess("");
      setLoadingEditor(true);
      getMyWorktreeCommands(name)
        .then((row) => {
          if (row) {
            setHasRow(true);
            init.replaceOriginal(row.init_commands);
            run.replaceOriginal(row.run_commands);
            setGenerateReport(row.generate_report);
          } else {
            setHasRow(false);
            init.replaceOriginal([]);
            run.replaceOriginal([]);
            setGenerateReport(false);
          }
        })
        .catch((e) => setError(String((e as Error).message || e)))
        .finally(() => setLoadingEditor(false));
    },
    [init, run],
  );

  // Default selection: the active repo if accessible, else the first accessible
  // (falling back to the first), once the list loads.
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
        hasCommands: withCommands.has(r.name),
        accessible: access[r.name] !== false,
      })),
    [myRepos, withCommands, access],
  );

  const validationError = useMemo(
    () => validateCommands(init.value, run.value),
    [init.value, run.value],
  );
  const dirty = init.dirty || run.dirty;

  const handleSave = useCallback(async (): Promise<boolean> => {
    if (!selected) return true;
    if (validationError) {
      setError(validationError);
      return false;
    }
    setError("");
    setSuccess("");
    setSaving(true);
    try {
      const row = await putMyWorktreeCommands(selected, init.value, run.value, generateReport);
      setHasRow(true);
      init.replaceOriginal(row.init_commands);
      run.replaceOriginal(row.run_commands);
      setGenerateReport(row.generate_report);
      setSuccess(t("worktreeSettings.commandsSaved"));
      setWithCommands((prev) => new Set(prev).add(selected));
      return true;
    } catch (e) {
      setError(String((e as Error).message || e));
      return false;
    } finally {
      setSaving(false);
    }
  }, [selected, validationError, init, run, generateReport, t]);

  // Fold into the page-level Save footer.
  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);
  useEffect(() => {
    registerSave?.(handleSave);
  }, [registerSave, handleSave]);

  const handleDelete = async () => {
    if (!selected) return;
    setConfirmDelete(false);
    setError("");
    setSuccess("");
    setSaving(true);
    try {
      await deleteMyWorktreeCommands(selected);
      setHasRow(false);
      init.replaceOriginal([]);
      run.replaceOriginal([]);
      setGenerateReport(false);
      setSuccess(t("worktreeSettings.commandsDeleted"));
      setWithCommands((prev) => {
        const next = new Set(prev);
        next.delete(selected);
        return next;
      });
    } catch (e) {
      setError(String((e as Error).message || e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="space-y-4">
      <header>
        <h2 className="text-base font-semibold text-gray-300 mb-1">{t("worktreeSettings.title")}</h2>
        <p className="text-sm text-gray-500">{t("worktreeSettings.description")}</p>
      </header>

      <div className="flex flex-col md:flex-row gap-4 min-h-[24rem]">
        <RepoSidebar
          repos={repos}
          loading={loadingRepos}
          selected={selected}
          onSelect={loadWorkspace}
        />

        <section className="flex-1 border border-gray-800 rounded-lg bg-gray-950 p-4">
          {!selected ? (
            <div className="h-full flex items-center justify-center text-sm text-gray-500 italic min-h-[16rem]">
              {t("worktreeSettings.selectRepo")}
            </div>
          ) : loadingEditor ? (
            <p className="text-sm text-gray-500">{t("actions.loading")}</p>
          ) : (
            <div className="space-y-6">
              <div className="flex items-center justify-between gap-3 flex-wrap">
                <h3 className="text-base font-semibold text-gray-200">
                  {t("worktreeSettings.repositoryLabel")} <span className="font-mono">{selected}</span>
                </h3>
              </div>

              {!hasRow && !dirty && (
                <p className="text-sm text-gray-500 italic">{t("worktreeSettings.noRowHint")}</p>
              )}

              <section className="space-y-2">
                <div>
                  <h4 className="text-sm font-semibold text-gray-200">{t("worktreeSettings.initCommands")}</h4>
                  <p className="text-xs text-gray-500">{t("worktreeSettings.initCommandsHelp")}</p>
                </div>
                <WorktreeCommandList
                  commands={init.value}
                  onChange={init.setValue}
                  disabled={saving}
                />
              </section>

              <section className="space-y-2">
                <div>
                  <h4 className="text-sm font-semibold text-gray-200">{t("worktreeSettings.runCommands")}</h4>
                  <p className="text-xs text-gray-500">{t("worktreeSettings.runCommandsHelp")}</p>
                </div>
                <WorktreeRunCommandList
                  commands={run.value}
                  onChange={run.setValue}
                  disabled={saving}
                />
              </section>

              {validationError && <p className="text-sm text-red-400">{validationError}</p>}
              {error && <p className="text-sm text-red-400">{error}</p>}
              {success && <p className="text-sm text-green-400">{success}</p>}

              {/* Save is the page-level footer; only the discrete Delete-for-repo
                  action stays inline here. */}
              <div className="flex items-center gap-3 pt-2 border-t border-gray-800 flex-wrap">
                <button
                  type="button"
                  onClick={() => setConfirmDelete(true)}
                  disabled={saving || !hasRow}
                  className="px-4 py-1.5 rounded-lg bg-red-900/40 text-red-300 text-sm font-medium border border-red-800 hover:bg-red-900/70 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
                >
                  {t("worktreeSettings.deleteForRepo")}
                </button>
              </div>
            </div>
          )}
        </section>
      </div>

      {confirmDelete && (
        <ConfirmModal
          title={t("worktreeSettings.deleteConfirmTitle")}
          message={t("worktreeSettings.deleteConfirmMessage", { name: selected })}
          onConfirm={handleDelete}
          onCancel={() => setConfirmDelete(false)}
        />
      )}
    </div>
  );
}

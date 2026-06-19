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
import {
  deleteMyWorktreeCommands,
  getMyWorktreeCommands,
  listMyWorktreeCommands,
  putMyWorktreeCommands,
  type RunCommand,
} from "../api/client";
import { useDiffForm } from "../hooks/useDiffForm";
import { useMyRepositories } from "../hooks/useMyRepositories";
import { ConfirmModal } from "./modals/ConfirmModal";
import { RepoSidebar, type RepoSidebarItem } from "./RepoSidebar";
import { WorktreeCommandList } from "./WorktreeCommandList";
import { WorktreeRunCommandList } from "./WorktreeRunCommandList";
import { validateCommands } from "./WorktreeSettings/validateCommands";

export function WorktreeSettingsTab() {
  const { myRepos, activeRepoName } = useMyRepositories();
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

  // Default selection: the active repo (else the first) once the list loads.
  const loadingRepos = myRepos === null;
  useEffect(() => {
    if (myRepos === null || selected !== null) return;
    const def =
      activeRepoName && myRepos.some((r) => r.name === activeRepoName)
        ? activeRepoName
        : (myRepos[0]?.name ?? null);
    if (def) loadWorkspace(def);
  }, [myRepos, activeRepoName, selected, loadWorkspace]);

  const repos: RepoSidebarItem[] = useMemo(
    () => (myRepos ?? []).map((r) => ({ name: r.name, hasCommands: withCommands.has(r.name) })),
    [myRepos, withCommands],
  );

  const validationError = useMemo(
    () => validateCommands(init.value, run.value),
    [init.value, run.value],
  );
  const dirty = init.dirty || run.dirty;

  const handleSave = async () => {
    if (!selected) return;
    if (validationError) {
      setError(validationError);
      return;
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
      setSuccess("Commands saved.");
      setWithCommands((prev) => new Set(prev).add(selected));
    } catch (e) {
      setError(String((e as Error).message || e));
    } finally {
      setSaving(false);
    }
  };

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
      setSuccess(
        "Commands deleted. Future work items on this repository will run no init commands and show no run-command buttons.",
      );
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
        <h2 className="text-base font-semibold text-gray-300 mb-1">Repository Settings</h2>
        <p className="text-sm text-gray-500">
          Per-repository init commands (run when the item's container is brought up — before agent
          steps, the IDE, the terminal, or a run command use it) and run commands (buttons shown on
          your completed work item cards). These settings are private to your user account — every
          user manages their own.
        </p>
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
              Select a repository to configure its commands.
            </div>
          ) : loadingEditor ? (
            <p className="text-sm text-gray-500">Loading…</p>
          ) : (
            <div className="space-y-6">
              <div className="flex items-center justify-between gap-3 flex-wrap">
                <h3 className="text-base font-semibold text-gray-200">
                  Repository: <span className="font-mono">{selected}</span>
                </h3>
              </div>

              {!hasRow && !dirty && (
                <p className="text-sm text-gray-500 italic">
                  No commands set for this repository. Workflows here will run no init commands
                  and show no run-command buttons. Add commands below to customize.
                </p>
              )}

              <section className="space-y-2">
                <div>
                  <h4 className="text-sm font-semibold text-gray-200">Init commands</h4>
                  <p className="text-xs text-gray-500">
                    Run sequentially in the worktree each time the item's container starts —
                    before agent steps, the IDE, the terminal, or a run command use it.
                  </p>
                </div>
                <WorktreeCommandList
                  commands={init.value}
                  onChange={init.setValue}
                  disabled={saving}
                />
              </section>

              <section className="space-y-2">
                <div>
                  <h4 className="text-sm font-semibold text-gray-200">Run commands</h4>
                  <p className="text-xs text-gray-500">
                    Buttons shown on your completed work item cards.
                  </p>
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

              <div className="flex items-center gap-3 pt-2 border-t border-gray-800 flex-wrap">
                <button
                  type="button"
                  onClick={handleSave}
                  disabled={saving || !!validationError || !dirty}
                  className="px-4 py-1.5 rounded-lg bg-blue-600 text-white text-sm font-medium hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
                >
                  {saving ? "Saving…" : "Save"}
                </button>
                <button
                  type="button"
                  onClick={() => setConfirmDelete(true)}
                  disabled={saving || !hasRow}
                  className="px-4 py-1.5 rounded-lg bg-red-900/40 text-red-300 text-sm font-medium border border-red-800 hover:bg-red-900/70 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
                >
                  Delete commands for this repository
                </button>
                {dirty && !validationError && (
                  <span className="text-xs text-gray-500 italic">Unsaved changes</span>
                )}
              </div>
            </div>
          )}
        </section>
      </div>

      {confirmDelete && (
        <ConfirmModal
          title="Delete commands for this repository"
          message={`Remove your init commands and run commands for "${selected}"? Future work items on this repository will run no init commands and show no run-command buttons until you add them again.`}
          onConfirm={handleDelete}
          onCancel={() => setConfirmDelete(false)}
        />
      )}
    </div>
  );
}

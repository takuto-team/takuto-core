// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Per-user-per-workspace Worktree Settings tab.
 *
 * - No admin gate, no global default. Each authenticated user manages their
 *   own rows; admins have no special access here.
 * - A row is either present (with init + run commands) or absent. There is
 *   nothing to "revert to" — the only ways to leave a workspace are: never
 *   save a row, or DELETE the row entirely.
 * - Single PUT atomically updates BOTH init_commands and run_commands.
 *
 * Diff-aware editing state lives in `useDiffForm`; workspace list state lives
 * in `useWorktreeWorkspaces`; the pure validator is in
 * `./WorktreeSettings/validateCommands`. This file owns the page layout,
 * the load-on-select effect, and the Save / Delete flows.
 */

import { useCallback, useMemo, useState } from "react";
import {
  deleteMyWorktreeCommands,
  getMyWorktreeCommands,
  putMyWorktreeCommands,
  type RunCommand,
} from "../api/client";
import { useDiffForm } from "../hooks/useDiffForm";
import { useWorktreeWorkspaces } from "../hooks/useWorktreeWorkspaces";
import { ConfirmModal } from "./modals/ConfirmModal";
import { WorktreeCommandList } from "./WorktreeCommandList";
import { WorktreeRunCommandList } from "./WorktreeRunCommandList";
import { WorkspaceSidebar } from "./WorktreeSettings/WorkspaceSidebar";
import { validateCommands } from "./WorktreeSettings/validateCommands";

export function WorktreeSettingsTab() {
  const { workspaces, loading: loadingWorkspaces, setHasMyCommands } = useWorktreeWorkspaces();

  const [selected, setSelected] = useState<string | null>(null);
  const [hasRow, setHasRow] = useState(false);
  const [loadingEditor, setLoadingEditor] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);

  const init = useDiffForm<string[]>([]);
  const run = useDiffForm<RunCommand[]>([]);

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
          } else {
            setHasRow(false);
            init.replaceOriginal([]);
            run.replaceOriginal([]);
          }
        })
        .catch((e) => setError(String((e as Error).message || e)))
        .finally(() => setLoadingEditor(false));
    },
    [init, run],
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
      const row = await putMyWorktreeCommands(selected, init.value, run.value);
      setHasRow(true);
      init.replaceOriginal(row.init_commands);
      run.replaceOriginal(row.run_commands);
      setSuccess("Commands saved.");
      setHasMyCommands(selected, true);
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
      setSuccess(
        "Commands deleted. Future workflows on this workspace will run no init commands and show no run-command buttons.",
      );
      setHasMyCommands(selected, false);
    } catch (e) {
      setError(String((e as Error).message || e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="space-y-4">
      <header>
        <h2 className="text-base font-semibold text-gray-300 mb-1">Worktree Settings</h2>
        <p className="text-sm text-gray-500">
          Per-workspace init commands (run before agent steps when a worktree is bootstrapped) and
          run commands (buttons shown on your completed work item cards). These settings are
          private to your user account — every user manages their own.
        </p>
      </header>

      <div className="flex flex-col md:flex-row gap-4 min-h-[24rem]">
        <WorkspaceSidebar
          workspaces={workspaces}
          loading={loadingWorkspaces}
          selected={selected}
          onSelect={loadWorkspace}
        />

        <section className="flex-1 border border-gray-800 rounded-lg bg-gray-950 p-4">
          {!selected ? (
            <div className="h-full flex items-center justify-center text-sm text-gray-500 italic min-h-[16rem]">
              Select a workspace to configure its commands.
            </div>
          ) : loadingEditor ? (
            <p className="text-sm text-gray-500">Loading…</p>
          ) : (
            <div className="space-y-6">
              <div className="flex items-center justify-between gap-3 flex-wrap">
                <h3 className="text-base font-semibold text-gray-200">
                  Workspace: <span className="font-mono">{selected}</span>
                </h3>
              </div>

              {!hasRow && !dirty && (
                <p className="text-sm text-gray-500 italic">
                  No commands set for this workspace. Workflows here will run no init commands
                  and show no run-command buttons. Add commands below to customize.
                </p>
              )}

              <section className="space-y-2">
                <div>
                  <h4 className="text-sm font-semibold text-gray-200">Init commands</h4>
                  <p className="text-xs text-gray-500">
                    Run sequentially in the worktree before agent steps.
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
                  Delete commands for this workspace
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
          title="Delete commands for this workspace"
          message={`Remove your init commands and run commands for "${selected}"? Future workflows on this workspace will run no init commands and show no run-command buttons until you add them again.`}
          onConfirm={handleDelete}
          onCancel={() => setConfirmDelete(false)}
        />
      )}
    </div>
  );
}

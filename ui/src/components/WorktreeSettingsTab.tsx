// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Plan-09 per-user-per-workspace Worktree Settings tab.
 *
 * - No admin gate, no global default. Each authenticated user manages their
 *   own rows; admins have no special access here.
 * - A row is either present (with init + run commands) or absent. There is
 *   nothing to "revert to" — the only ways to leave a workspace are: never
 *   save a row, or DELETE the row entirely.
 * - Single PUT atomically updates BOTH init_commands and run_commands.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  deleteMyWorktreeCommands,
  getMyWorktreeCommands,
  listWorktreeCommandsWorkspaces,
  putMyWorktreeCommands,
  type RunCommand,
  type WorktreeCommandsWorkspaceEntry,
} from "../api/client";
import { ConfirmModal } from "./modals/ConfirmModal";
import { WorktreeCommandList } from "./WorktreeCommandList";
import { WorktreeRunCommandList } from "./WorktreeRunCommandList";

const MAX_COMMANDS = 50;
const MAX_COMMAND_LEN = 2000;
const MAX_NAME_LEN = 100;

export function WorktreeSettingsTab() {
  const [workspaces, setWorkspaces] = useState<WorktreeCommandsWorkspaceEntry[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [editingInit, setEditingInit] = useState<string[]>([]);
  const [editingRun, setEditingRun] = useState<RunCommand[]>([]);
  const [hasRow, setHasRow] = useState(false);
  const [originalInit, setOriginalInit] = useState<string[]>([]);
  const [originalRun, setOriginalRun] = useState<RunCommand[]>([]);
  const [loadingWorkspaces, setLoadingWorkspaces] = useState(true);
  const [loadingEditor, setLoadingEditor] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);

  const refreshWorkspaces = useCallback(() => {
    setLoadingWorkspaces(true);
    listWorktreeCommandsWorkspaces()
      .then(setWorkspaces)
      .catch((e) => setError(String((e as Error).message || e)))
      .finally(() => setLoadingWorkspaces(false));
  }, []);

  useEffect(() => {
    refreshWorkspaces();
  }, [refreshWorkspaces]);

  const loadWorkspace = useCallback((name: string) => {
    setSelected(name);
    setError("");
    setSuccess("");
    setLoadingEditor(true);
    getMyWorktreeCommands(name)
      .then((row) => {
        if (row) {
          setHasRow(true);
          setEditingInit(row.init_commands);
          setEditingRun(row.run_commands);
          setOriginalInit(row.init_commands);
          setOriginalRun(row.run_commands);
        } else {
          setHasRow(false);
          setEditingInit([]);
          setEditingRun([]);
          setOriginalInit([]);
          setOriginalRun([]);
        }
      })
      .catch((e) => setError(String((e as Error).message || e)))
      .finally(() => setLoadingEditor(false));
  }, []);

  const validationError = useMemo(() => {
    if (editingInit.length > MAX_COMMANDS) {
      return `Too many init commands (limit ${MAX_COMMANDS}).`;
    }
    for (let i = 0; i < editingInit.length; i += 1) {
      const trimmed = editingInit[i].trim();
      if (trimmed.length === 0) {
        return `Init command #${i + 1} is empty.`;
      }
      if (trimmed.length > MAX_COMMAND_LEN) {
        return `Init command #${i + 1} exceeds ${MAX_COMMAND_LEN} characters.`;
      }
      if (editingInit[i].includes("\0")) {
        return `Init command #${i + 1} contains a NUL byte.`;
      }
    }

    if (editingRun.length > MAX_COMMANDS) {
      return `Too many run commands (limit ${MAX_COMMANDS}).`;
    }
    const seenNames = new Set<string>();
    for (let i = 0; i < editingRun.length; i += 1) {
      const rc = editingRun[i];
      const name = rc.name.trim();
      const cmd = rc.command.trim();
      if (name.length === 0) {
        return `Run command #${i + 1}: name is empty.`;
      }
      if (name.length > MAX_NAME_LEN) {
        return `Run command #${i + 1}: name exceeds ${MAX_NAME_LEN} characters.`;
      }
      if (cmd.length === 0) {
        return `Run command #${i + 1}: command is empty.`;
      }
      if (cmd.length > MAX_COMMAND_LEN) {
        return `Run command #${i + 1}: command exceeds ${MAX_COMMAND_LEN} characters.`;
      }
      if (rc.name.includes("\0") || rc.command.includes("\0")) {
        return `Run command #${i + 1}: contains a NUL byte.`;
      }
      if (seenNames.has(name)) {
        return `Run command #${i + 1}: duplicate name "${name}".`;
      }
      seenNames.add(name);
    }
    return null;
  }, [editingInit, editingRun]);

  const dirty = useMemo(() => {
    if (editingInit.length !== originalInit.length) return true;
    for (let i = 0; i < originalInit.length; i += 1) {
      if (originalInit[i] !== editingInit[i]) return true;
    }
    if (editingRun.length !== originalRun.length) return true;
    for (let i = 0; i < originalRun.length; i += 1) {
      if (
        originalRun[i].name !== editingRun[i].name ||
        originalRun[i].command !== editingRun[i].command
      ) {
        return true;
      }
    }
    return false;
  }, [editingInit, editingRun, originalInit, originalRun]);

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
      const row = await putMyWorktreeCommands(selected, editingInit, editingRun);
      setHasRow(true);
      setEditingInit(row.init_commands);
      setEditingRun(row.run_commands);
      setOriginalInit(row.init_commands);
      setOriginalRun(row.run_commands);
      setSuccess("Commands saved.");
      setWorkspaces((prev) =>
        prev.map((w) => (w.name === selected ? { ...w, has_my_commands: true } : w)),
      );
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
      setEditingInit([]);
      setEditingRun([]);
      setOriginalInit([]);
      setOriginalRun([]);
      setSuccess(
        "Commands deleted. Future workflows on this workspace will run no init commands and show no run-command buttons.",
      );
      setWorkspaces((prev) =>
        prev.map((w) => (w.name === selected ? { ...w, has_my_commands: false } : w)),
      );
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
          run commands (buttons shown on your completed workflow cards). These settings are
          private to your user account — every user manages their own.
        </p>
      </header>

      <div className="flex flex-col md:flex-row gap-4 min-h-[24rem]">
        {/* Left: workspace list */}
        <aside className="md:w-1/3 md:max-w-xs border border-gray-800 rounded-lg bg-gray-950 overflow-hidden">
          <div className="px-3 py-2 border-b border-gray-800 text-xs uppercase tracking-wide text-gray-500">
            Workspaces
          </div>
          {loadingWorkspaces ? (
            <p className="text-sm text-gray-500 p-3">Loading…</p>
          ) : workspaces.length === 0 ? (
            <p className="text-sm text-gray-500 p-3">No workspaces found.</p>
          ) : (
            <ul className="divide-y divide-gray-800">
              {workspaces.map((w) => {
                const isSelected = selected === w.name;
                return (
                  <li key={w.name}>
                    <button
                      type="button"
                      onClick={() => loadWorkspace(w.name)}
                      className={`w-full flex items-center justify-between gap-2 px-3 py-2 text-left text-sm cursor-pointer transition-colors ${
                        isSelected
                          ? "bg-blue-950/40 text-blue-200"
                          : "text-gray-300 hover:bg-gray-900"
                      }`}
                    >
                      <span className="truncate font-medium">{w.name}</span>
                      <span className="flex items-center gap-1.5 shrink-0">
                        {w.has_my_commands ? (
                          <span
                            className="inline-flex items-center gap-1 text-[11px] text-emerald-300"
                            title="You have commands set for this workspace"
                          >
                            <span className="w-2 h-2 rounded-full bg-emerald-400" />
                            set
                          </span>
                        ) : (
                          <span
                            className="inline-flex items-center gap-1 text-[11px] text-gray-500"
                            title="No commands set for this workspace"
                          >
                            <span className="w-2 h-2 rounded-full bg-gray-600" />
                            none
                          </span>
                        )}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </aside>

        {/* Right: editor */}
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
                  commands={editingInit}
                  onChange={setEditingInit}
                  disabled={saving}
                />
              </section>

              <section className="space-y-2">
                <div>
                  <h4 className="text-sm font-semibold text-gray-200">Run commands</h4>
                  <p className="text-xs text-gray-500">
                    Buttons shown on your completed workflow cards.
                  </p>
                </div>
                <WorktreeRunCommandList
                  commands={editingRun}
                  onChange={setEditingRun}
                  disabled={saving}
                />
              </section>

              {validationError && (
                <p className="text-sm text-red-400">{validationError}</p>
              )}
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

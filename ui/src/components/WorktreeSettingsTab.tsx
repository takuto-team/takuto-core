// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  deleteWorktreeCommandsOverride,
  getWorktreeCommands,
  getWorktreeCommandsOverride,
  listWorktreeCommandsWorkspaces,
  putWorktreeCommandsOverride,
  type WorktreeCommandsWorkspaceEntry,
} from "../api/client";
import { ConfirmModal } from "./modals/ConfirmModal";
import { WorktreeCommandList } from "./WorktreeCommandList";

const MAX_COMMANDS = 50;
const MAX_COMMAND_LEN = 2000;

export function WorktreeSettingsTab() {
  const [workspaces, setWorkspaces] = useState<WorktreeCommandsWorkspaceEntry[]>([]);
  const [globalDefault, setGlobalDefault] = useState<string[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditing] = useState<string[]>([]);
  const [hasOverride, setHasOverride] = useState(false);
  const [originalCommands, setOriginalCommands] = useState<string[] | null>(null);
  const [loadingWorkspaces, setLoadingWorkspaces] = useState(true);
  const [loadingEditor, setLoadingEditor] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [confirmRevert, setConfirmRevert] = useState(false);

  const refreshWorkspaces = useCallback(() => {
    setLoadingWorkspaces(true);
    Promise.all([listWorktreeCommandsWorkspaces(), getWorktreeCommands()])
      .then(([ws, top]) => {
        setWorkspaces(ws);
        setGlobalDefault(top.default);
      })
      .catch((e) => setError(String((e as Error).message || e)))
      .finally(() => setLoadingWorkspaces(false));
  }, []);

  useEffect(() => {
    refreshWorkspaces();
  }, [refreshWorkspaces]);

  const loadWorkspace = useCallback(
    (name: string) => {
      setSelected(name);
      setError("");
      setSuccess("");
      setLoadingEditor(true);
      getWorktreeCommandsOverride(name)
        .then((row) => {
          if (row) {
            setHasOverride(true);
            setEditing(row.commands);
            setOriginalCommands(row.commands);
          } else {
            setHasOverride(false);
            setEditing(globalDefault.slice());
            setOriginalCommands(null);
          }
        })
        .catch((e) => setError(String((e as Error).message || e)))
        .finally(() => setLoadingEditor(false));
    },
    [globalDefault],
  );

  const validationError = useMemo(() => {
    if (editing.length > MAX_COMMANDS) {
      return `Too many commands (limit ${MAX_COMMANDS}).`;
    }
    for (let i = 0; i < editing.length; i += 1) {
      const trimmed = editing[i].trim();
      if (trimmed.length === 0) {
        return `Command #${i + 1} is empty.`;
      }
      if (trimmed.length > MAX_COMMAND_LEN) {
        return `Command #${i + 1} exceeds ${MAX_COMMAND_LEN} characters.`;
      }
      if (editing[i].includes("\0")) {
        return `Command #${i + 1} contains a NUL byte.`;
      }
    }
    return null;
  }, [editing]);

  const dirty = useMemo(() => {
    const baseline = originalCommands ?? globalDefault;
    if (baseline.length !== editing.length) return true;
    for (let i = 0; i < baseline.length; i += 1) {
      if (baseline[i] !== editing[i]) return true;
    }
    return false;
  }, [editing, originalCommands, globalDefault]);

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
      const row = await putWorktreeCommandsOverride(selected, editing);
      setHasOverride(true);
      setOriginalCommands(row.commands);
      setEditing(row.commands);
      setSuccess("Override saved.");
      // Reflect override status in the workspace list.
      setWorkspaces((prev) =>
        prev.map((w) => (w.name === selected ? { ...w, has_override: true } : w)),
      );
    } catch (e) {
      setError(String((e as Error).message || e));
    } finally {
      setSaving(false);
    }
  };

  const handleRevert = async () => {
    if (!selected) return;
    setConfirmRevert(false);
    setError("");
    setSuccess("");
    setSaving(true);
    try {
      await deleteWorktreeCommandsOverride(selected);
      setHasOverride(false);
      setOriginalCommands(null);
      setEditing(globalDefault.slice());
      setSuccess("Override removed — workspace now uses the global default.");
      setWorkspaces((prev) =>
        prev.map((w) => (w.name === selected ? { ...w, has_override: false } : w)),
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
          Per-workspace overrides for <code className="text-gray-300">worktree_init_commands</code>.
          When a workspace has an override, those commands run after worktree creation instead of the
          global config. Removing an override falls back to the global default.
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
                        {w.has_override ? (
                          <span
                            className="inline-flex items-center gap-1 text-[11px] text-amber-300"
                            title="Has per-workspace override"
                          >
                            <span className="w-2 h-2 rounded-full bg-amber-400" />
                            override
                          </span>
                        ) : (
                          <span
                            className="inline-flex items-center gap-1 text-[11px] text-gray-500"
                            title="Uses global default"
                          >
                            <span className="w-2 h-2 rounded-full bg-gray-600" />
                            default
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
              Select a workspace on the left to view or edit its init commands.
            </div>
          ) : loadingEditor ? (
            <p className="text-sm text-gray-500">Loading…</p>
          ) : (
            <div className="space-y-4">
              <div className="flex items-center justify-between gap-3 flex-wrap">
                <h3 className="text-base font-semibold text-gray-200">{selected}</h3>
                <span
                  className={`text-[11px] px-2 py-0.5 rounded-full ${
                    hasOverride
                      ? "bg-amber-950 border border-amber-700 text-amber-200"
                      : "bg-gray-900 border border-gray-700 text-gray-400"
                  }`}
                >
                  Currently: {hasOverride ? "override" : "using global default"}
                </span>
              </div>

              <WorktreeCommandList
                commands={editing}
                onChange={setEditing}
                disabled={saving}
              />

              {validationError && (
                <p className="text-sm text-red-400">{validationError}</p>
              )}
              {error && <p className="text-sm text-red-400">{error}</p>}
              {success && <p className="text-sm text-green-400">{success}</p>}

              <div className="flex items-center gap-3 pt-2 border-t border-gray-800">
                <button
                  type="button"
                  onClick={handleSave}
                  disabled={saving || !!validationError || !dirty}
                  className="px-4 py-1.5 rounded-lg bg-blue-600 text-white text-sm font-medium hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
                >
                  {saving ? "Saving…" : hasOverride ? "Save override" : "Create override"}
                </button>
                <button
                  type="button"
                  onClick={() => setConfirmRevert(true)}
                  disabled={saving || !hasOverride}
                  className="px-4 py-1.5 rounded-lg bg-gray-800 text-gray-300 text-sm font-medium border border-gray-700 hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
                >
                  Revert to default
                </button>
                {dirty && !validationError && (
                  <span className="text-xs text-gray-500 italic">Unsaved changes</span>
                )}
              </div>
            </div>
          )}
        </section>
      </div>

      {confirmRevert && (
        <ConfirmModal
          title="Revert to global default"
          message={`Remove the per-workspace override for "${selected}"? Future workflow bootstraps for this workspace will use the global default again.`}
          onConfirm={handleRevert}
          onCancel={() => setConfirmRevert(false)}
        />
      )}
    </div>
  );
}

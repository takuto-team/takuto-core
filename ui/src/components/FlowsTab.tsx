// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Per-user-per-workspace Flows tab.
 *
 * - No admin gate. Each authenticated user owns their own ordered flow list
 *   for the active workspace; the workspace name comes from the
 *   `GET /api/me/flows` response, not a selector — switching workspace happens
 *   elsewhere and this tab reflects whatever is active.
 * - The whole list is read and written atomically: add, delete, reorder,
 *   inline edits, and re-seed all resolve to a single PUT (or reseed POST)
 *   that replaces the row.
 * - An empty list (`flows: []`) is a deliberate user state, distinct from
 *   "never seeded" (which the backend resolves before the UI ever sees it).
 * - Only one flow can be expanded at a time. Expanding a flow shows an inline
 *   editor; the "Add flow" button appends a draft card that opens directly
 *   into the editor. Cancelling the draft drops it; cancelling an existing
 *   flow just collapses the editor without writing.
 */

import { useCallback, useEffect, useState } from "react";
import { getMyFlows, putMyFlows, reseedMyFlows, MAX_FLOWS, type UserFlow } from "../api/flows";
import { ConfirmModal } from "./modals/ConfirmModal";
import { FlowCard } from "./FlowCard";
import { FlowEditor } from "./FlowEditor";

/** `expanded` selects an existing index, the literal "new" for the draft card, or none. */
type Expanded = number | "new" | null;

export function FlowsTab() {
  const [flows, setFlows] = useState<UserFlow[]>([]);
  const [workspace, setWorkspace] = useState("");
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [saving, setSaving] = useState(false);
  const [actionError, setActionError] = useState("");
  const [confirmDelete, setConfirmDelete] = useState<number | null>(null);
  const [confirmReseed, setConfirmReseed] = useState(false);
  const [expanded, setExpanded] = useState<Expanded>(null);
  const [dragIndex, setDragIndex] = useState<number | null>(null);

  const load = useCallback(() => {
    setLoading(true);
    setLoadError("");
    getMyFlows()
      .then((res) => {
        setFlows(res.flows);
        setWorkspace(res.workspace);
      })
      .catch((e) => setLoadError(String((e as Error).message || e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const submitList = useCallback(
    async (next: UserFlow[]) => {
      const prev = flows;
      setSaving(true);
      setActionError("");
      setFlows(next);
      try {
        const res = await putMyFlows(next);
        setFlows(res.flows);
        setWorkspace(res.workspace);
      } catch (e) {
        setFlows(prev);
        setActionError(String((e as Error).message || e));
        throw e;
      } finally {
        setSaving(false);
      }
    },
    [flows],
  );

  const handleEditorSubmit = useCallback(
    async (next: UserFlow[]) => {
      await submitList(next);
      setExpanded(null);
    },
    [submitList],
  );

  const handleDelete = async () => {
    if (confirmDelete === null) return;
    const idx = confirmDelete;
    setConfirmDelete(null);
    if (expanded === idx) setExpanded(null);
    try {
      await submitList(flows.filter((_, i) => i !== idx));
    } catch {
      /* error already surfaced by submitList */
    }
  };

  const handleReseed = async () => {
    setConfirmReseed(false);
    setSaving(true);
    setActionError("");
    setExpanded(null);
    try {
      const res = await reseedMyFlows();
      setFlows(res.flows);
      setWorkspace(res.workspace);
    } catch (e) {
      setActionError(String((e as Error).message || e));
    } finally {
      setSaving(false);
    }
  };

  const handleDrop = (targetIndex: number) => {
    const from = dragIndex;
    setDragIndex(null);
    if (from === null || from === targetIndex) return;
    const next = [...flows];
    const [moved] = next.splice(from, 1);
    next.splice(targetIndex, 0, moved);
    setExpanded(null);
    submitList(next).catch(() => {
      /* error already surfaced */
    });
  };

  const toggleExpand = (i: number) => {
    setExpanded((prev) => (prev === i ? null : i));
  };

  const addFlow = () => {
    setExpanded("new");
  };

  const atCap = flows.length >= MAX_FLOWS;
  const capTooltip = atCap ? "You've reached the 20-flow limit for this workspace." : undefined;

  const addButton = (
    <button
      type="button"
      onClick={addFlow}
      disabled={atCap || saving || expanded === "new"}
      title={capTooltip}
      className="px-4 py-1.5 rounded-lg bg-blue-600 text-white text-sm font-medium hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
    >
      + Add flow
    </button>
  );

  const reseedButton = (
    <button
      type="button"
      onClick={() => setConfirmReseed(true)}
      disabled={saving}
      className="px-4 py-1.5 rounded-lg bg-gray-800 text-gray-300 text-sm font-medium border border-gray-700 hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
    >
      Re-seed from defaults
    </button>
  );

  return (
    <div className="space-y-4">
      <header className="flex items-start justify-between gap-4 flex-wrap">
        <div>
          <h2 className="text-base font-semibold text-gray-300 mb-1">
            Flows — <span className="font-mono">{workspace || "…"}</span>
          </h2>
          <p className="text-sm text-gray-500 max-w-2xl">
            Click a flow on a work-item card to run its steps in order. Dependencies require an
            upstream flow to have completed at least once on that work item.
          </p>
        </div>
        <span className={`text-sm font-mono ${atCap ? "text-amber-400" : "text-gray-500"}`}>
          {flows.length} / {MAX_FLOWS}
        </span>
      </header>

      {loading ? (
        <p className="text-sm text-gray-500">Loading…</p>
      ) : loadError ? (
        <p className="text-sm text-red-400">
          Could not load flows.{" "}
          <button
            type="button"
            onClick={load}
            className="underline hover:text-red-300 cursor-pointer"
          >
            Retry
          </button>
        </p>
      ) : flows.length === 0 && expanded !== "new" ? (
        <div className="flex justify-center py-8">
          <div className="border border-gray-800 rounded-lg bg-gray-950 p-6 max-w-md text-center space-y-4">
            <p className="text-sm text-gray-400">
              You have no flows configured for <span className="font-mono">{workspace}</span>.
            </p>
            <p className="text-sm text-gray-500">
              Work-item cards in this workspace will show an empty state until you add at least one.
            </p>
            {actionError && <p className="text-sm text-red-400">{actionError}</p>}
            <div className="flex items-center justify-center gap-3">
              {addButton}
              {reseedButton}
            </div>
          </div>
        </div>
      ) : (
        <>
          <div className={`space-y-2 ${saving ? "opacity-50 pointer-events-none" : ""}`}>
            {flows.map((flow, i) => (
              <FlowCard
                key={flow.name}
                flow={flow}
                flows={flows}
                index={i}
                expanded={expanded === i}
                draggable={!saving}
                isDragging={dragIndex === i}
                onToggleExpand={() => toggleExpand(i)}
                onDelete={() => setConfirmDelete(i)}
                onSubmit={handleEditorSubmit}
                onCancelEdit={() => setExpanded(null)}
                onDragStart={() => setDragIndex(i)}
                onDrop={() => handleDrop(i)}
                onDragEnd={() => setDragIndex(null)}
              />
            ))}

            {expanded === "new" && (
              <div className="border border-blue-700/60 rounded-lg bg-gray-950">
                <div className="px-3 py-2.5 text-sm text-gray-400 italic border-b border-gray-800">
                  New flow
                </div>
                <FlowEditor
                  flows={flows}
                  editIndex={null}
                  onSubmit={handleEditorSubmit}
                  onCancel={() => setExpanded(null)}
                />
              </div>
            )}
          </div>

          {actionError && <p className="text-sm text-red-400">{actionError}</p>}

          <div className="flex items-center gap-3 pt-3 border-t border-gray-800">
            {addButton}
            {reseedButton}
          </div>
        </>
      )}

      {confirmDelete !== null && (
        <ConfirmModal
          title="Delete flow"
          message={`Delete flow "${flows[confirmDelete]?.name ?? ""}"? This removes it from every work-item card in this workspace.`}
          confirmLabel="Delete"
          onConfirm={handleDelete}
          onCancel={() => setConfirmDelete(null)}
        />
      )}

      {confirmReseed && (
        <ConfirmModal
          title="Re-seed flows from defaults"
          message={`This replaces all flows for ${workspace} with the defaults shipped with Maestro. Your current flows for this workspace will be lost. Other workspaces are unaffected.`}
          confirmLabel="Re-seed"
          onConfirm={handleReseed}
          onCancel={() => setConfirmReseed(false)}
        />
      )}
    </div>
  );
}

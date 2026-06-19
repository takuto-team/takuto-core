// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Auto-saving per-workspace "generate work-item reports" switch.
 *
 * Container around the presentational {@link GenerateReportToggle}. The flag
 * lives in the same `user_worktree_commands` row as the workspace's init/run
 * commands, so flipping it must NOT clobber those: it loads the current row
 * first, then PUTs the full row back with only `generate_report` changed.
 * Persists the moment it is flipped (optimistic, reverts on failure).
 *
 * Rendered at the top of the Workflows page (and so, in the setup wizard).
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  getMyWorktreeCommands,
  putMyWorktreeCommands,
  type RunCommand,
} from "../api/worktreeCommands";
import { GenerateReportToggle } from "./GenerateReportToggle";

interface Props {
  /** Active workspace; empty while the parent is still resolving it. */
  workspace: string;
}

export function GenerateReportSwitch({ workspace }: Props) {
  const [enabled, setEnabled] = useState(false);
  // Preserve the row's commands so saving the toggle never wipes them.
  const [initCommands, setInitCommands] = useState<string[]>([]);
  const [runCommands, setRunCommands] = useState<RunCommand[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  // Transient "Saved" confirmation: shown on a successful flip, auto-hidden
  // after 2s, and cleared immediately if the toggle is flipped again.
  const [saved, setSaved] = useState(false);
  const savedTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  // Drop the pending hide-timer on unmount so it never fires after teardown.
  useEffect(() => () => clearTimeout(savedTimer.current), []);

  useEffect(() => {
    if (!workspace) return;
    let cancelled = false;
    setLoading(true);
    setError("");
    getMyWorktreeCommands(workspace)
      .then((row) => {
        if (cancelled) return;
        setEnabled(row?.generate_report ?? false);
        setInitCommands(row?.init_commands ?? []);
        setRunCommands(row?.run_commands ?? []);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [workspace]);

  const onChange = useCallback(
    async (next: boolean) => {
      if (saving || loading || !workspace) return;
      // A fresh flip cancels any lingering "Saved" message from the last one.
      clearTimeout(savedTimer.current);
      setSaved(false);
      setEnabled(next); // optimistic
      setSaving(true);
      setError("");
      try {
        const row = await putMyWorktreeCommands(workspace, initCommands, runCommands, next);
        setEnabled(row.generate_report);
        setInitCommands(row.init_commands);
        setRunCommands(row.run_commands);
        setSaved(true);
        savedTimer.current = setTimeout(() => setSaved(false), 2000);
      } catch (e: unknown) {
        setEnabled(!next); // revert
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setSaving(false);
      }
    },
    [workspace, initCommands, runCommands, saving, loading],
  );

  return (
    <div>
      <GenerateReportToggle value={enabled} onChange={onChange} disabled={loading || saving} />
      {saved && <p className="text-sm text-green-400 mt-1">Saved</p>}
      {error && <p className="text-sm text-red-400 mt-1">Could not save: {error}</p>}
    </div>
  );
}

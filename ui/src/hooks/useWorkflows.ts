// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback, useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "../api/client";
import { queryKeys } from "../api/queryClient";
import type { WorkflowSummary, WorkflowEvent, TerminalLine, WorkflowCounts } from "../api/types";

export interface TerminalState {
  stepName: string;
  lines: TerminalLine[];
  completed: boolean;
}

const TERMINAL_MAX_LINES = 500;

const EMPTY_COUNTS: WorkflowCounts = { running: 0, completed: 0, errors: 0, paused: 0 };

/** Dynamic port forwards from API, keyed by ticket_key → [container_port, proxy_url][] */
export type DynamicForwards = Record<string, [number, string][]>;

export interface SystemError {
  id: number;
  ticketKey: string;
  message: string;
  timestamp: Date;
}

let errorIdCounter = 0;

function isTerminalState(state: string): boolean {
  const lower = state.toLowerCase();
  return lower === "done" || lower.startsWith("error") || lower === "stopped";
}

export function useWorkflows() {
  const queryClient = useQueryClient();

  const { data: list } = useQuery({
    queryKey: queryKeys.workItems,
    queryFn: () => apiJson<WorkflowSummary[]>("/api/work-items"),
  });
  const { data: countsData } = useQuery({
    queryKey: queryKeys.workItemCounts,
    queryFn: () => apiJson<WorkflowCounts>("/api/work-items/counts"),
  });

  // Workflows and dynamic forwards are pure projections of the server list.
  const workflows = useMemo(() => {
    const next: Record<string, WorkflowSummary> = {};
    for (const w of list ?? []) next[w.ticket_key] = w;
    return next;
  }, [list]);

  const dynamicForwards = useMemo(() => {
    const next: DynamicForwards = {};
    for (const w of list ?? []) {
      const mappings = w.editor_port_mappings ?? [];
      if (mappings.length > 0) next[w.ticket_key] = mappings;
    }
    return next;
  }, [list]);

  const counts = countsData ?? EMPTY_COUNTS;

  // Terminal output is streamed over the WebSocket and never re-fetched, so
  // it lives in local state seeded from each workflow's `terminal_lines`.
  const [terminalStates, setTerminalStates] = useState<Record<string, TerminalState>>({});
  const [systemErrors, setSystemErrors] = useState<SystemError[]>([]);
  // Display order is the server order, but with memory: a refetch never
  // re-sorts existing rows — new keys are appended. This reconciliation of
  // server data into a stable client order keeps state (prior order), so it
  // is a genuine sync effect rather than a pure derivation.
  const [orderKeys, setOrderKeys] = useState<string[]>([]);

  useEffect(() => {
    if (!list) return;
    const newKeys = list.map((w) => w.ticket_key);
    setOrderKeys((prev) => {
      if (prev.length === 0) return newKeys;
      const existing = prev.filter((k) => newKeys.includes(k));
      const added = newKeys.filter((k) => !prev.includes(k));
      return [...existing, ...added];
    });
    setTerminalStates((prev) => {
      let changed = false;
      const next = { ...prev };
      for (const w of list) {
        if (!next[w.ticket_key]) {
          next[w.ticket_key] = {
            stepName: w.state || "Waiting...",
            lines: w.terminal_lines || [],
            completed: false,
          };
          changed = true;
        }
      }
      return changed ? next : prev;
    });
  }, [list]);

  const fetchWorkflows = useCallback(
    () => queryClient.invalidateQueries({ queryKey: queryKeys.workItems }),
    [queryClient]
  );
  const fetchCounts = useCallback(
    () => queryClient.invalidateQueries({ queryKey: queryKeys.workItemCounts }),
    [queryClient]
  );

  const patchWorkItem = useCallback(
    (ticketKey: string, mut: (w: WorkflowSummary) => WorkflowSummary) => {
      queryClient.setQueryData<WorkflowSummary[]>(queryKeys.workItems, (prev) =>
        prev ? prev.map((w) => (w.ticket_key === ticketKey ? mut(w) : w)) : prev
      );
    },
    [queryClient]
  );

  const handleEvent = useCallback(
    (evt: WorkflowEvent) => {
      const { event_type, ticket_key } = evt;

      if (event_type === "work_item_removed") {
        queryClient.setQueryData<WorkflowSummary[]>(queryKeys.workItems, (prev) =>
          prev ? prev.filter((w) => w.ticket_key !== ticket_key) : prev
        );
        setTerminalStates((prev) => {
          const next = { ...prev };
          delete next[ticket_key];
          return next;
        });
        fetchCounts();
        return;
      }

      // Terminal output events — update terminal state only (skip for other-workspace workflows)
      if (event_type === "step_output") {
        setTerminalStates((prev) => {
          if (!prev[ticket_key]) return prev;
          const ts = prev[ticket_key];
          const line: TerminalLine = {
            text: evt.output_line || "",
            stream: evt.stream || "stdout",
          };
          const lines = [...ts.lines, line].slice(-TERMINAL_MAX_LINES);
          return { ...prev, [ticket_key]: { ...ts, lines } };
        });
        return;
      }

      if (event_type === "step_started") {
        setTerminalStates((prev) => {
          if (!prev[ticket_key]) return prev;
          return {
            ...prev,
            [ticket_key]: {
              stepName: evt.step_name || "",
              lines: [],
              completed: false,
            },
          };
        });
        // Reflect the active step name on the card immediately.
        patchWorkItem(ticket_key, (wf) => ({ ...wf, state: evt.step_name || wf.state }));
        return;
      }

      if (event_type === "step_completed") {
        setTerminalStates((prev) => {
          const ts = prev[ticket_key];
          if (!ts) return prev;
          return { ...prev, [ticket_key]: { ...ts, completed: true } };
        });
        // Re-fetch to get updated steps_log and progress.
        fetchWorkflows();
        return;
      }

      // Run command events — re-fetch run_commands; surface stop failures.
      if (
        event_type === "run_command_port_forwarded" ||
        event_type === "run_command_port_unforwarded" ||
        event_type === "run_command_stopped"
      ) {
        if (event_type === "run_command_stopped" && evt.error) {
          setSystemErrors((prev) => [
            ...prev,
            {
              id: ++errorIdCounter,
              ticketKey: ticket_key,
              message: evt.error!,
              timestamp: new Date(),
            },
          ]);
        }
        fetchWorkflows();
        return;
      }

      // Port forwarding events — re-fetch to get proxy URLs from the server.
      if (event_type === "port_forwarded" || event_type === "port_unforwarded") {
        fetchWorkflows();
        return;
      }

      // Workflow state change.
      const current = queryClient.getQueryData<WorkflowSummary[]>(queryKeys.workItems);
      const wf = current?.find((w) => w.ticket_key === ticket_key);
      if (!wf) {
        // Unknown workflow (likely from another workspace) — refresh global counts only.
        if (ticket_key) fetchCounts();
        return;
      }

      const updated: WorkflowSummary = { ...wf };
      if (evt.state) updated.state = evt.state;
      if (typeof evt.progress_percent === "number") updated.progress_percent = evt.progress_percent;
      if (typeof evt.progress_steps_total === "number") updated.progress_steps_total = evt.progress_steps_total;
      if (typeof evt.pr_merged === "boolean") updated.pr_merged = evt.pr_merged;
      if (evt.error) updated.error = evt.error;

      // Terminal states or newly-active workflows: re-fetch for action flags
      // (can_open_editor, pr_url, etc.) that can only be computed server-side.
      const becameActive = !wf.can_open_editor && !!evt.state && evt.state !== wf.state;
      if (isTerminalState(updated.state) || becameActive) {
        fetchWorkflows();
        fetchCounts();
        return;
      }
      patchWorkItem(ticket_key, () => updated);
    },
    [queryClient, fetchWorkflows, fetchCounts, patchWorkItem]
  );

  const dismissError = useCallback((id: number) => {
    setSystemErrors((prev) => prev.filter((e) => e.id !== id));
  }, []);

  const resetState = useCallback(() => {
    setTerminalStates({});
    setSystemErrors([]);
    setOrderKeys([]);
    queryClient.removeQueries({ queryKey: queryKeys.workItems });
    queryClient.removeQueries({ queryKey: queryKeys.workItemCounts });
  }, [queryClient]);

  return {
    workflows,
    orderKeys,
    terminalStates,
    dynamicForwards,
    systemErrors,
    counts,
    dismissError,
    fetchWorkflows,
    fetchCounts,
    handleEvent,
    resetState,
  };
}

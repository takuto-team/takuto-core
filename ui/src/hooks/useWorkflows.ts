// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useCallback, useRef, useEffect } from "react";
import { apiJson } from "../api/client";
import type { WorkflowSummary, WorkflowEvent, TerminalLine } from "../api/types";

export interface TerminalState {
  stepName: string;
  lines: TerminalLine[];
  completed: boolean;
}

const TERMINAL_MAX_LINES = 500;

/** Dynamic port forwards from WebSocket events, keyed by ticket_key → [container_port, host_port][] */
export type DynamicForwards = Record<string, [number, number][]>;

export interface SystemError {
  id: number;
  ticketKey: string;
  message: string;
  timestamp: Date;
}

let errorIdCounter = 0;

export function useWorkflows() {
  const [workflows, setWorkflows] = useState<Record<string, WorkflowSummary>>({});
  const [orderKeys, setOrderKeys] = useState<string[]>([]);
  const [terminalStates, setTerminalStates] = useState<Record<string, TerminalState>>({});
  const [dynamicForwards, setDynamicForwards] = useState<DynamicForwards>({});
  const [systemErrors, setSystemErrors] = useState<SystemError[]>([]);
  const initialLoadDone = useRef(false);

  const fetchWorkflows = useCallback(async () => {
    try {
      const list = await apiJson<WorkflowSummary[]>("/api/workflows");
      setWorkflows(() => {
        const next: Record<string, WorkflowSummary> = {};
        for (const w of list) next[w.ticket_key] = w;
        return next;
      });
      setOrderKeys((prev) => {
        const newKeys = list.map((w) => w.ticket_key);
        if (prev.length === 0) return newKeys;
        // Preserve existing order, append new keys
        const existing = prev.filter((k) => newKeys.includes(k));
        const added = newKeys.filter((k) => !prev.includes(k));
        return [...existing, ...added];
      });
      // Initialize terminal states from workflow terminal_lines
      setTerminalStates((prev) => {
        const next = { ...prev };
        for (const w of list) {
          if (!next[w.ticket_key]) {
            next[w.ticket_key] = {
              stepName: w.state || "Waiting...",
              lines: w.terminal_lines || [],
              completed: false,
            };
          }
        }
        return next;
      });
      initialLoadDone.current = true;
    } catch {
      // Silently ignore fetch errors (e.g. 401 handled by api client)
    }
  }, []);

  useEffect(() => {
    fetchWorkflows();
  }, [fetchWorkflows]);

  const handleEvent = useCallback(
    (evt: WorkflowEvent) => {
      const { event_type, ticket_key } = evt;

      if (event_type === "workflow_removed") {
        setWorkflows((prev) => {
          const next = { ...prev };
          delete next[ticket_key];
          return next;
        });
        setOrderKeys((prev) => prev.filter((k) => k !== ticket_key));
        setTerminalStates((prev) => {
          const next = { ...prev };
          delete next[ticket_key];
          return next;
        });
        setDynamicForwards((prev) => {
          const next = { ...prev };
          delete next[ticket_key];
          return next;
        });
        return;
      }

      // Terminal output events — update terminal state only
      if (event_type === "step_output") {
        setTerminalStates((prev) => {
          const ts = prev[ticket_key] || { stepName: "", lines: [], completed: false };
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
        setTerminalStates((prev) => ({
          ...prev,
          [ticket_key]: {
            stepName: evt.step_name || "",
            lines: [],
            completed: false,
          },
        }));
        // Update workflow state to show step name
        setWorkflows((prev) => {
          const wf = prev[ticket_key];
          if (!wf) return prev;
          return { ...prev, [ticket_key]: { ...wf, state: evt.step_name || wf.state } };
        });
        return;
      }

      if (event_type === "step_completed") {
        setTerminalStates((prev) => {
          const ts = prev[ticket_key];
          if (!ts) return prev;
          return { ...prev, [ticket_key]: { ...ts, completed: true } };
        });
        // Re-fetch to get updated steps_log and progress
        fetchWorkflows();
        return;
      }

      // Run command events — update run_commands in workflow state
      if (
        event_type === "run_command_port_forwarded" ||
        event_type === "run_command_port_unforwarded" ||
        event_type === "run_command_stopped"
      ) {
        // Surface error from run command failure
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

      // Port forwarding events — update dynamic forwards map
      if (event_type === "port_forwarded" && evt.forwarded_port) {
        const [cp, hp] = evt.forwarded_port;
        setDynamicForwards((prev) => {
          const existing = prev[ticket_key] || [];
          if (existing.some(([c]) => c === cp)) return prev;
          return { ...prev, [ticket_key]: [...existing, [cp, hp]] };
        });
        return;
      }
      if (event_type === "port_unforwarded" && evt.forwarded_port) {
        const [cp] = evt.forwarded_port;
        setDynamicForwards((prev) => {
          const existing = prev[ticket_key];
          if (!existing) return prev;
          const filtered = existing.filter(([c]) => c !== cp);
          return { ...prev, [ticket_key]: filtered };
        });
        return;
      }

      // Workflow state change — update locally
      let needsRefetch = false;

      setWorkflows((prev) => {
        const wf = prev[ticket_key];
        if (wf) {
          const updated = { ...wf };
          if (evt.state) updated.state = evt.state;
          if (typeof evt.progress_percent === "number") updated.progress_percent = evt.progress_percent;
          if (typeof evt.progress_steps_total === "number") updated.progress_steps_total = evt.progress_steps_total;
          if (typeof evt.pr_merged === "boolean") updated.pr_merged = evt.pr_merged;
          if (evt.error) updated.error = evt.error;

          // Terminal states: schedule re-fetch for action flags / pr_url
          const lower = updated.state.toLowerCase();
          const isTerminal = lower === "done" || lower.startsWith("error") || lower === "stopped";
          if (isTerminal) {
            needsRefetch = true;
            return prev;
          }
          return { ...prev, [ticket_key]: updated };
        }
        // Unknown workflow — schedule fetch
        if (ticket_key) needsRefetch = true;
        return prev;
      });

      if (needsRefetch) fetchWorkflows();
    },
    [fetchWorkflows]
  );

  const dismissError = useCallback((id: number) => {
    setSystemErrors((prev) => prev.filter((e) => e.id !== id));
  }, []);

  return {
    workflows,
    orderKeys,
    terminalStates,
    dynamicForwards,
    systemErrors,
    dismissError,
    fetchWorkflows,
    handleEvent,
  };
}

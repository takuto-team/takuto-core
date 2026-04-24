// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useCallback, useEffect, useRef } from "react";
import { api, apiPost } from "../api/client";
import type { WorkflowSummary, WorkflowDefinition, RunCommandStatus } from "../api/types";
import type { TerminalState } from "../hooks/useWorkflows";
import { TerminalOutput } from "./TerminalOutput";
import { WorkflowDefButtons } from "./WorkflowDefButtons";
import { useToast } from "../hooks/useToast";
import { ConfirmModal } from "./modals/ConfirmModal";

interface Props {
  workflow: WorkflowSummary;
  terminalState?: TerminalState;
  dynamicForwards: [number, number][];
  workflowDefs: WorkflowDefinition[];
  onRefresh: () => void;
  onShowDescription: (ticketKey: string, summary: string, description?: string) => void;
  onReport: (ticketKey: string) => void;
}

interface StatusInfo {
  label: string;
  color: string;
}

/** Map color keys to concrete Tailwind color hex values — avoids dynamic class names
 *  that Tailwind v4 would purge at build time. */
const COLOR_HEX: Record<string, { bg: string; text: string; border: string; bgFaint: string }> = {
  green:  { bg: "#22c55e", text: "#4ade80", border: "rgba(34,197,94,0.2)",  bgFaint: "rgba(34,197,94,0.15)" },
  red:    { bg: "#ef4444", text: "#f87171", border: "rgba(239,68,68,0.2)",  bgFaint: "rgba(239,68,68,0.15)" },
  yellow: { bg: "#eab308", text: "#facc15", border: "rgba(234,179,8,0.2)",  bgFaint: "rgba(234,179,8,0.15)" },
  gray:   { bg: "#6b7280", text: "#9ca3af", border: "rgba(107,114,128,0.2)", bgFaint: "rgba(107,114,128,0.15)" },
  blue:   { bg: "#3b82f6", text: "#60a5fa", border: "rgba(59,130,246,0.2)", bgFaint: "rgba(59,130,246,0.15)" },
};

function getStatusInfo(state: string, canStart?: boolean): StatusInfo {
  const s = state.toLowerCase();
  if (s === "done" || s.startsWith("completed")) return { label: "Completed", color: "green" };
  if (s.startsWith("error")) return { label: "Error", color: "red" };
  if (s === "paused") return { label: "Paused", color: "yellow" };
  if (s === "stopped") return { label: "Stopped", color: "gray" };
  if (s === "pending" && canStart) return { label: "Pending", color: "gray" };
  return { label: "Running", color: "blue" };
}

function progressInfo(w: WorkflowSummary) {
  const pct = Math.max(0, Math.min(100, Math.round(w.progress_percent || 0)));
  const total = w.progress_steps_total > 0 ? Math.floor(w.progress_steps_total) : 0;
  const filled = total > 0 ? Math.min(total, Math.round((pct * total) / 100)) : 0;
  return { pct, total, filled };
}

function formatDuration(start: Date, end: Date): string {
  const secs = Math.max(0, Math.floor((end.getTime() - start.getTime()) / 1000));
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return `${h}h ${m}m ${s}s`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

export function WorkflowCard({ workflow: w, terminalState: ts, dynamicForwards, workflowDefs, onRefresh, onShowDescription, onReport }: Props) {
  const [loading, setLoading] = useState<false | "generic" | string>(false);
  const [confirm, setConfirm] = useState<{ action: string; label: string; fn: () => Promise<void> } | null>(null);
  const [terminalCollapsed, setTerminalCollapsed] = useState(true);
  const { showToast } = useToast();

  const status = getStatusInfo(w.state, w.can_start);
  const { pct, total, filled } = progressInfo(w);
  const prUrl = w.pr_url?.trim() || "";
  const isTerminal = ["Completed", "Error", "Stopped"].includes(status.label);
  const isPending = status.label === "Pending" && w.can_start;
  const isActive = status.label === "Running" || status.label === "Paused";

  const duration = isTerminal && w.started_at && w.updated_at
    ? formatDuration(new Date(w.started_at), new Date(w.updated_at))
    : null;

  const withLoading = useCallback(
    async (fn: () => Promise<void>, message?: string) => {
      setLoading(message || "generic");
      try {
        await fn();
        onRefresh();
      } catch (e) {
        showToast(e instanceof Error ? e.message : "Action failed");
      } finally {
        setLoading(false);
      }
    },
    [onRefresh]
  );

  const confirmAction = (label: string, action: string, fn: () => Promise<void>) => {
    setConfirm({ action, label, fn });
  };

  const doAction = (endpoint: string) => async () => {
    const res = await apiPost(`/api/workflows/${encodeURIComponent(w.ticket_key)}/${endpoint}`);
    if (!res.ok) {
      const t = await res.text();
      throw new Error(t || `Failed: ${endpoint}`);
    }
  };

  const openEditor = async () => {
    const res = await api(`/api/workflows/${encodeURIComponent(w.ticket_key)}/open-editor`, { method: "POST" });
    if (!res.ok) throw new Error(await res.text() || "Failed to start editor");
    const data = await res.json();
    if (data.url) window.open(data.url, "_blank");
  };

  const openTerminal = async () => {
    let res = await api(`/api/workflows/${encodeURIComponent(w.ticket_key)}/open-terminal`, { method: "POST" });
    if (res.status === 409) {
      await api(`/api/workflows/${encodeURIComponent(w.ticket_key)}/open-editor`, { method: "POST" });
      res = await api(`/api/workflows/${encodeURIComponent(w.ticket_key)}/open-terminal`, { method: "POST" });
    }
    if (!res.ok) throw new Error(await res.text() || "Failed to start terminal");
    const data = await res.json();
    if (data.url) window.open(data.url, "_blank");
  };

  const closeEditor = async () => {
    const res = await apiPost(`/api/workflows/${encodeURIComponent(w.ticket_key)}/close-editor`);
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || "Failed to close editor");
    }
  };

  // Step display
  let stepLabel = "Current step";
  if (status.label === "Completed") stepLabel = "Completed";
  else if (status.label === "Error") stepLabel = "Failed at step";
  else if (status.label === "Paused") stepLabel = "Paused at step";
  else if (status.label === "Stopped") stepLabel = "Stopped at step";
  else if (isPending) stepLabel = "Added to dashboard";

  let stateDisplay = w.state;
  if (status.label === "Completed") stateDisplay = "All steps passed";
  if (status.label === "Error" && w.state.startsWith("Error:")) stateDisplay = w.state.replace("Error: ", "");
  if (total > 0) stateDisplay += ` (${filled}/${total})`;

  const borderClass =
    status.color === "red"
      ? "border-red-500/30 hover:border-red-500/40"
      : status.color === "yellow"
      ? "border-yellow-500/30 hover:border-yellow-500/40"
      : "border-gray-800 hover:border-gray-700";

  // Effective terminal state — for completed workflows, use API terminal_lines if no live state
  const effectiveTs = ts ?? (
    isTerminal && w.terminal_lines?.length > 0
      ? { stepName: w.state, lines: w.terminal_lines, completed: true }
      : undefined
  );
  const hasTerminalLines = effectiveTs && effectiveTs.lines.length > 0;

  return (
    <>
      <div className={`workflow-card border ${borderClass} transition-colors ${status.label === "Stopped" ? "opacity-60 hover:opacity-80" : ""} relative`}>
        {loading && (
          <div className="absolute inset-0 bg-gray-900/90 z-10 flex items-center justify-center rounded-xl">
            {loading !== "generic" ? <ConnectionOverlay message={loading as string} /> : (
              <span className="text-sm text-gray-400">Working...</span>
            )}
          </div>
        )}

        {/* Header: ticket key + status badge + PR links */}
        <div className="flex items-center justify-between gap-3 min-w-0">
          <div className="flex items-center gap-2 min-w-0 flex-1">
            <span className="font-mono text-sm font-medium" style={{ color: (COLOR_HEX[status.color] || COLOR_HEX.blue).text }}>{w.ticket_key}</span>
            <StatusBadge status={status} />
          </div>
          {prUrl && (
            <div className="flex items-center gap-2 flex-shrink-0">
              {w.pr_merged && (
                <span className="text-xs text-purple-400/80">Merged</span>
              )}
              <a
                href={prUrl}
                target="_blank"
                rel="noopener noreferrer"
                className="action-btn wf-btn-secondary inline-flex items-center gap-1"
              >
                Show PR &#x2197;
              </a>
            </div>
          )}
        </div>

        {/* Summary */}
        <h3 className="text-sm font-medium text-gray-200 truncate">{w.ticket_summary}</h3>

        {/* Progress frame with Report button */}
        <div className="bg-gray-800/50 rounded-lg px-3 py-2.5 relative">
          <div className="flex items-center justify-between">
            <div className="text-xs text-gray-500">{stepLabel}</div>
            <div className="flex items-center gap-2">
              {duration && (
                <span className="flex items-center gap-1 text-xs text-gray-400">
                  <ClockIcon />
                  <span className="font-mono">{duration}</span>
                </span>
              )}
              {w.has_report && (
                <button
                  onClick={() => onReport(w.ticket_key)}
                  className="text-xs text-gray-500 hover:text-gray-300 cursor-pointer transition-colors"
                  title="View workflow report"
                >
                  Show Report
                </button>
              )}
            </div>
          </div>
          <div className="text-sm font-mono text-gray-300 mt-0.5">{stateDisplay}</div>
          <div className="mt-2">
            <ProgressBar pct={pct} total={total} filled={filled} color={status.color} />
          </div>
        </div>

        {/* Actions — three layout states: pending (not started), terminal, running/paused */}
        {isPending ? (
          /* Pending (added to dashboard, not yet started) — Start + nav + delete */
          <div className="flex flex-col gap-2">
            <div className="flex flex-wrap gap-2">
              <ActionBtn variant="secondary" onClick={() => onShowDescription(w.ticket_key, w.ticket_summary, w.ticket_description)}>
                Show description
              </ActionBtn>
              {w.jira_available && (
                <ActionBtn variant="secondary" onClick={() => window.open(w.jira_browse_url, "_blank")}>
                  Go to ticket
                </ActionBtn>
              )}
              {w.ticketing_system === "github" && (
                <ActionBtn variant="secondary" onClick={() => window.open(w.jira_browse_url, "_blank")}>
                  Go to issue
                </ActionBtn>
              )}
            </div>
            <div className="flex flex-wrap gap-2">
              <ActionBtn variant="primary" onClick={() => withLoading(doAction("start"))}>
                <PlayIcon /> Start
              </ActionBtn>
              {w.can_delete && (
                <ActionBtn variant="danger" onClick={() => confirmAction("Delete", "delete", doAction("delete"))}>
                  Delete
                </ActionBtn>
              )}
            </div>
          </div>
        ) : isTerminal ? (
          <div className="flex flex-col gap-2">
            {/* Row 1: Navigation actions */}
            <div className="flex flex-wrap gap-2">
              <ActionBtn variant="secondary" onClick={() => onShowDescription(w.ticket_key, w.ticket_summary, w.ticket_description)}>
                Show description
              </ActionBtn>
              {w.can_open_editor && (
                <>
                  {w.editor_url ? (
                    <a href={w.editor_url} target="_blank" rel="noopener" className="action-btn wf-btn-secondary inline-flex items-center gap-1">
                      Editor &#x2197;
                    </a>
                  ) : (
                    <ActionBtn variant="secondary" onClick={() => withLoading(openEditor, "Setting up a secure connection to an editor")}>Open Editor</ActionBtn>
                  )}
                  {w.terminal_url ? (
                    <a href={w.terminal_url} target="_blank" rel="noopener" className="action-btn wf-btn-secondary inline-flex items-center gap-1">
                      Terminal &#x2197;
                    </a>
                  ) : (
                    <ActionBtn variant="secondary" onClick={() => withLoading(openTerminal, "Setting up a secure connection to a terminal")}>Open Terminal</ActionBtn>
                  )}
                </>
              )}
            </div>
            {/* Row 2: Workflow actions */}
            <div className="flex flex-wrap gap-2">
              {w.can_resume_from_error && (
                <ActionBtn variant="primary" onClick={() => confirmAction("Retry from last failure", "resume-from-error", doAction("resume-from-error"))}>
                  Retry from last failure
                </ActionBtn>
              )}
              <ActionBtn variant="primary" onClick={() => confirmAction("Retry from 0", "retry", doAction("retry"))}>
                Retry from 0
              </ActionBtn>
              {w.can_merge_base && (
                <ActionBtn variant="primary" onClick={() => withLoading(doAction("merge-base-branch"))}>
                  Merge Base Branch
                </ActionBtn>
              )}
              {w.can_address_pr_comments && (
                <ActionBtn variant="primary" onClick={() => withLoading(doAction("address-pr-comments"))}>
                  Address PR Comments
                </ActionBtn>
              )}
            </div>
            {/* Row 3: Destructive / lifecycle */}
            <div className="flex flex-wrap gap-2">
              {w.can_mark_done && (
                <ActionBtn variant="success" onClick={() => confirmAction("Mark as Done", "mark-done", doAction("mark-done"))}>
                  Mark as Done
                </ActionBtn>
              )}
              {w.can_delete && (
                <ActionBtn variant="danger" onClick={() => confirmAction("Delete", "delete", doAction("delete"))}>
                  Delete
                </ActionBtn>
              )}
              {w.editor_url && (
                <ActionBtn variant="danger" onClick={() => withLoading(closeEditor)}>Close editor</ActionBtn>
              )}
            </div>
            {workflowDefs.length > 0 && (
              <WorkflowDefButtons
                definitions={workflowDefs}
                runStates={w.workflow_def_runs || {}}
                ticketKey={w.ticket_key}
                onRefresh={onRefresh}
              />
            )}
            <PortMappings apiMappings={w.editor_port_mappings} dynamicForwards={dynamicForwards} />
            {w.run_commands && w.run_commands.length > 0 && (
              <RunCommands
                ticketKey={w.ticket_key}
                commands={w.run_commands}
                withLoading={withLoading}
              />
            )}
          </div>
        ) : (
          /* Running / Paused actions — flat list */
          <>
            <div className="flex flex-wrap gap-2">
              {!w.jira_available ? null : (
                <ActionBtn variant="secondary" onClick={() => window.open(w.jira_browse_url, "_blank")}>
                  Go to ticket
                </ActionBtn>
              )}
              <ActionBtn variant="secondary" onClick={() => onShowDescription(w.ticket_key, w.ticket_summary, w.ticket_description)}>
                Show description
              </ActionBtn>
              {status.label === "Running" && (
                <ActionBtn variant="primary" onClick={() => withLoading(doAction("pause"))} title="Pause">
                  <PauseIcon /> Pause
                </ActionBtn>
              )}
              {status.label === "Paused" && (
                <ActionBtn variant="primary" onClick={() => withLoading(doAction("resume"))} title="Resume">
                  <PlayIcon /> Resume
                </ActionBtn>
              )}
            </div>
            {workflowDefs.length > 0 && (
              <WorkflowDefButtons
                definitions={workflowDefs}
                runStates={w.workflow_def_runs || {}}
                ticketKey={w.ticket_key}
                onRefresh={onRefresh}
              />
            )}
            <PortMappings apiMappings={w.editor_port_mappings} dynamicForwards={dynamicForwards} />
          </>
        )}

        {/* Terminal output — always shown for active; collapsible for terminal states */}
        {isActive && <TerminalOutput state={effectiveTs} />}
        {isTerminal && hasTerminalLines && (
          <div>
            <div className="border-t border-gray-800/60 mb-2" />
            <button
              onClick={() => setTerminalCollapsed(!terminalCollapsed)}
              className="flex items-center gap-1 text-xs text-gray-500 hover:text-gray-300 cursor-pointer transition-colors"
            >
              <svg
                className={`w-3.5 h-3.5 transition-transform ${terminalCollapsed ? "" : "rotate-180"}`}
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={2}
              >
                <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
              </svg>
              {terminalCollapsed ? "Show logs" : "Hide logs"}
            </button>
            {!terminalCollapsed && <TerminalOutput state={effectiveTs} />}
          </div>
        )}
      </div>

      {confirm && (
        <ConfirmModal
          title={confirm.label}
          message={`Are you sure you want to ${confirm.action} workflow ${w.ticket_key}?`}
          onConfirm={() => {
            setConfirm(null);
            withLoading(confirm.fn);
          }}
          onCancel={() => setConfirm(null)}
        />
      )}
    </>
  );
}

/* ── Terminal connection overlay ── */

const DOT_COUNT = 7;
const STEP_MS = 220;
const PAUSE_MS = 500;

function ConnectionOverlay({ message }: { message: string }) {
  const [lit, setLit] = useState(0);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    const tick = () => {
      setLit((prev) => {
        if (prev >= DOT_COUNT) {
          // All lit — pause then reset
          timer.current = setTimeout(tick, PAUSE_MS);
          return 0;
        }
        timer.current = setTimeout(tick, STEP_MS);
        return prev + 1;
      });
    };
    timer.current = setTimeout(tick, STEP_MS);
    return () => clearTimeout(timer.current);
  }, []);

  return (
    <div className="flex flex-col items-center gap-4">
      <span className="text-sm text-gray-300">{message}</span>
      <div className="flex items-center gap-0">
        <ComputerIcon />
        <div className="flex items-center gap-1.5 px-3">
          {Array.from({ length: DOT_COUNT }, (_, i) => (
            <span
              key={i}
              className="connection-dot"
              style={{ backgroundColor: i < lit ? "#22c55e" : undefined }}
            />
          ))}
        </div>
        <ComputerIcon />
      </div>
    </div>
  );
}

function ComputerIcon() {
  return (
    <svg className="w-8 h-8 text-gray-400" viewBox="0 0 64 64" fill="none" stroke="currentColor" strokeWidth={2}>
      <rect x="8" y="8" width="48" height="34" rx="3" />
      <rect x="12" y="12" width="40" height="24" rx="1" fill="currentColor" opacity="0.1" />
      <line x1="32" y1="42" x2="32" y2="50" />
      <line x1="22" y1="50" x2="42" y2="50" strokeLinecap="round" />
      <text x="16" y="28" fontSize="10" fill="currentColor" opacity="0.5" fontFamily="monospace">&gt;_</text>
    </svg>
  );
}

/* ── Port mappings ── */

function PortMappings({ apiMappings, dynamicForwards }: { apiMappings: [number, number][]; dynamicForwards: [number, number][] }) {
  // Merge API mappings + dynamic forwards, deduplicating by container port (dynamic wins)
  const dynPorts = new Set(dynamicForwards.map(([cp]) => cp));
  const merged: [number, number][] = [
    ...apiMappings.filter(([cp]) => !dynPorts.has(cp)),
    ...dynamicForwards,
  ];
  if (merged.length === 0) return null;

  return (
    <>
      <div className="border-t border-gray-800/60" />
      <div className="flex flex-wrap gap-2">
        {merged.map(([cp, hp]) => (
          <a
            key={`${cp}-${hp}`}
            href={`http://localhost:${hp}`}
            target="_blank"
            rel="noopener"
            className="action-btn wf-btn-secondary inline-flex items-center gap-1"
          >
            <PortIcon />
            {cp} &rarr; localhost:{hp}
          </a>
        ))}
      </div>
    </>
  );
}

function PortIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M13.828 10.172a4 4 0 00-5.656 0l-4 4a4 4 0 105.656 5.656l1.102-1.101m-.758-4.899a4 4 0 005.656 0l4-4a4 4 0 00-5.656-5.656l-1.1 1.1" />
    </svg>
  );
}

/* ── Run commands ── */

function RunCommands({
  ticketKey,
  commands,
  withLoading,
}: {
  ticketKey: string;
  commands: RunCommandStatus[];
  withLoading: (fn: () => Promise<void>, message?: string) => Promise<void>;
}) {
  const startCmd = (index: number) => async () => {
    const res = await apiPost(`/api/workflows/${encodeURIComponent(ticketKey)}/run-commands/${index}/start`);
    if (!res.ok) {
      const t = await res.text();
      throw new Error(t || "Failed to start run command");
    }
  };

  const stopCmd = (index: number) => async () => {
    const res = await apiPost(`/api/workflows/${encodeURIComponent(ticketKey)}/run-commands/${index}/stop`);
    if (!res.ok) {
      const t = await res.text();
      throw new Error(t || "Failed to stop run command");
    }
  };

  const copyUrl = (port: number) => {
    const url = `http://localhost:${port}`;
    navigator.clipboard.writeText(url).catch(() => {
      // Fallback for insecure contexts
      const ta = document.createElement("textarea");
      ta.value = url;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
    });
  };

  return (
    <>
      <div className="border-t border-gray-800/60" />
      <div className="flex flex-col gap-1.5">
        {commands.map((cmd) => (
          <div key={cmd.index} className="flex items-center gap-2 flex-wrap">
            {cmd.running ? (
              <>
                <button
                  onClick={() => withLoading(stopCmd(cmd.index))}
                  className="action-btn wf-btn-danger inline-flex items-center gap-1"
                >
                  <StopSquareIcon /> Stop {cmd.name}
                </button>
                {cmd.forwarded_port ? (
                  <>
                    <button
                      onClick={() => copyUrl(cmd.forwarded_port![1])}
                      className="action-btn wf-btn-secondary inline-flex items-center gap-1"
                      title={`Copy http://localhost:${cmd.forwarded_port[1]}`}
                    >
                      <CopyIcon /> Copy
                    </button>
                    <a
                      href={`http://localhost:${cmd.forwarded_port[1]}`}
                      target="_blank"
                      rel="noopener"
                      className="action-btn wf-btn-secondary inline-flex items-center gap-1"
                    >
                      <ExternalLinkIcon /> Open
                    </a>
                  </>
                ) : (
                  <>
                    <span
                      className="action-btn wf-btn-secondary opacity-50 cursor-not-allowed inline-flex items-center gap-1"
                      title="No listening port detected"
                    >
                      <CopyIcon /> Copy
                    </span>
                    <span
                      className="action-btn wf-btn-secondary opacity-50 cursor-not-allowed inline-flex items-center gap-1"
                      title="No listening port detected"
                    >
                      <ExternalLinkIcon /> Open
                    </span>
                  </>
                )}
              </>
            ) : (
              <button
                onClick={() => withLoading(startCmd(cmd.index), `Starting ${cmd.name}`)}
                className="action-btn wf-btn-primary inline-flex items-center gap-1"
              >
                <PlayIcon /> Run {cmd.name}
              </button>
            )}
          </div>
        ))}
      </div>
    </>
  );
}

function StopSquareIcon() {
  return (
    <svg className="w-3 h-3" fill="currentColor" viewBox="0 0 24 24">
      <rect x="6" y="6" width="12" height="12" rx="1" />
    </svg>
  );
}

function CopyIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
      <rect x="9" y="9" width="13" height="13" rx="2" />
      <path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1" />
    </svg>
  );
}

function ExternalLinkIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M18 13v6a2 2 0 01-2 2H5a2 2 0 01-2-2V8a2 2 0 012-2h6M15 3h6v6M10 14L21 3" />
    </svg>
  );
}

/* ── Button variants matching the 4-category palette from the redesign ── */

function ActionBtn({
  variant,
  onClick,
  children,
  title,
}: {
  variant: "primary" | "secondary" | "success" | "danger";
  onClick: () => void;
  children: React.ReactNode;
  title?: string;
}) {
  const cls = {
    primary: "wf-btn-primary",
    secondary: "wf-btn-secondary",
    success: "wf-btn-success",
    danger: "wf-btn-danger",
  }[variant];
  return (
    <button onClick={onClick} title={title} className={`action-btn ${cls}`}>
      {children}
    </button>
  );
}

function StatusBadge({ status }: { status: StatusInfo }) {
  const { label, color } = status;
  const c = COLOR_HEX[color] || COLOR_HEX.blue;
  return (
    <span
      className="inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full"
      style={{ backgroundColor: c.bgFaint, color: c.text, borderWidth: 1, borderColor: c.border }}
    >
      {label === "Completed" && <CheckIcon />}
      {label === "Error" && <XIcon />}
      {label === "Running" && <span className="w-1.5 h-1.5 rounded-full animate-pulse" style={{ backgroundColor: c.text }} />}
      {label}
    </span>
  );
}

function CheckIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
    </svg>
  );
}

function XIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
    </svg>
  );
}

function PauseIcon() {
  return (
    <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M10 9v6m4-6v6" />
    </svg>
  );
}

function PlayIcon() {
  return (
    <svg className="w-3.5 h-3.5" fill="currentColor" viewBox="0 0 24 24">
      <path d="M8 5v14l11-7z" />
    </svg>
  );
}

function ClockIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
    </svg>
  );
}

function ProgressBar({ pct, total, filled, color }: { pct: number; total: number; filled: number; color: string }) {
  const c = COLOR_HEX[color] || COLOR_HEX.blue;
  if (total <= 0) {
    return (
      <div className="w-full bg-gray-700 rounded-full h-1.5 overflow-hidden">
        <div className="h-1.5 rounded-full transition-all" style={{ width: `${pct}%`, backgroundColor: c.bg }} />
      </div>
    );
  }
  return (
    <div className="flex gap-0.5">
      {Array.from({ length: total }, (_, i) => (
        <div
          key={i}
          className="h-2 flex-1 rounded-full transition-all"
          style={{ backgroundColor: i < filled ? c.bg : "#4b5563" }}
        />
      ))}
    </div>
  );
}

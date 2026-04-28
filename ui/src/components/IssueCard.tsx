// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useCallback, useEffect, useRef } from "react";
import { api, apiPost } from "../api/client";
import type { WorkflowSummary, WorkflowDefinition, RunCommandStatus } from "../api/types";
import type { TerminalState } from "../hooks/useWorkflows";
import { WorkflowDefButtons } from "./WorkflowDefButtons";
import { useToast } from "../hooks/useToast";
import { ConfirmModal } from "./modals/ConfirmModal";
import { DeleteConfirmModal } from "./modals/DeleteConfirmModal";
import { ConsoleOutputModal } from "./modals/ConsoleOutputModal";
import { Button } from "./Button";
import { Label } from "./Label";
import { StatusBadge, getStatusInfo } from "./StatusBadge";
import { DeleteIconButton } from "./DeleteIconButton";
import { PauseIconButton } from "./PauseIconButton";
import { StopIconButton } from "./StopIconButton";
import { RestartIconButton } from "./RestartIconButton";
import { ResumeIconButton } from "./ResumeIconButton";

interface Props {
  workflow: WorkflowSummary;
  terminalState?: TerminalState;
  dynamicForwards: [number, number][];
  workflowDefs: WorkflowDefinition[];
  onRefresh: () => void;
  onShowDescription: (ticketKey: string, summary: string, description?: string) => void;
  onReport: (ticketKey: string) => void;
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

export function IssueCard({ workflow: w, terminalState: ts, dynamicForwards, workflowDefs, onRefresh, onShowDescription, onReport }: Props) {
  const [loading, setLoading] = useState<false | "generic" | string>(false);
  const [confirm, setConfirm] = useState<{ action: string; label: string; fn: () => Promise<void> } | null>(null);
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const [consoleOpen, setConsoleOpen] = useState(false);
  const [openMenu, setOpenMenu] = useState<"port" | "editor" | "terminal" | null>(null);
  const { showToast } = useToast();

  const status = getStatusInfo(w.state, w.can_start);
  const { pct, total, filled } = progressInfo(w);
  const prUrl = w.pr_url?.trim() || "";
  const isTerminal = ["Completed", "Error", "Stopped"].includes(status.label);
  const isPending = status.label === "Pending" && w.can_start;
  const isPreparingWorktree = isPending && !!w.branch_name && !w.worktree_path;
  const isActive = status.label === "Running" || status.label === "Paused";

  const duration = isTerminal && status.label !== "Error" && status.label !== "Completed" && status.label !== "Stopped" && w.started_at && w.updated_at
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

  const mergedPorts: [number, number][] = (() => {
    const dynPorts = new Set(dynamicForwards.map(([cp]) => cp));
    return [
      ...(w.editor_port_mappings || []).filter(([cp]) => !dynPorts.has(cp)),
      ...dynamicForwards,
    ];
  })();

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

        {/* Delete button — top-right corner */}
        {w.can_delete && (
          <div className="absolute top-1 right-1 translate-x-1/2 -translate-y-1/2 z-10">
            <DeleteIconButton onClick={() => setDeleteConfirmOpen(true)} />
          </div>
        )}

        {/* Header: ticket key + status badge + PR links */}
        <div className="flex items-center justify-between gap-3 min-w-0">
          <div className="flex items-center gap-2 min-w-0 flex-1">
            {w.jira_browse_url ? (
              <a
                href={w.jira_browse_url}
                target="_blank"
                rel="noopener noreferrer"
                className="font-mono text-base font-medium text-blue-400 hover:underline"
              >
                {w.ticket_key}
              </a>
            ) : (
              <span className="font-mono text-base font-medium text-blue-400">{w.ticket_key}</span>
            )}
            <StatusBadge status={status} />
          </div>
          {prUrl && (
            <div className="flex-shrink-0">
              <Label
                variant={w.pr_merged ? "purple" : "info"}
                href={prUrl}
              >
                PR #{prUrl.match(/\/(\d+)\/?$/)?.[1] ?? ""}
              </Label>
            </div>
          )}
        </div>

        {/* Summary — click to view/edit description */}
        <button
          onClick={() => onShowDescription(w.ticket_key, w.ticket_summary, w.ticket_description)}
          className="flex items-center leading-none gap-1.5 group text-left w-full min-w-0 cursor-pointer"
        >
          <span className="text-sm font-medium text-white group-hover:text-gray-400 transition-colors truncate min-w-0">{w.ticket_summary}</span>
          <ExternalLinkIcon className="flex-shrink-0 w-3 h-3 text-white group-hover:text-gray-400 transition-colors" />
        </button>

        {/* Progress frame */}
        <div className="bg-gray-800/50 rounded-lg px-3 pt-2.5 pb-2.5 relative h-[80px] flex flex-col justify-center">
          {isPreparingWorktree ? (
            <div className="flex items-center leading-none gap-2 text-xs text-gray-500">
              <span className="inline-block w-2 h-2 rounded-full bg-gray-500 animate-pulse flex-shrink-0" />
              Preparing worktree&hellip;
            </div>
          ) : (
            <>
              <div className="flex items-center justify-between">
                <div className="text-xs text-gray-500">{stepLabel}</div>
                <div className="flex items-center gap-2">
                  <span className={`flex items-center leading-none gap-1 text-xs text-gray-400 ${!duration ? "invisible" : ""}`}>
                    <ClockIcon />
                    <span className="font-mono">{duration ?? "0s"}</span>
                  </span>
                  {w.has_report && (
                    <button
                      onClick={() => onReport(w.ticket_key)}
                      className="text-xs text-gray-500 hover:text-gray-300 cursor-pointer transition-colors"
                      title="View workflow report"
                    >
                      Show Report
                    </button>
                  )}
                  {(status.label === "Error" || status.label === "Completed" || status.label === "Stopped") && (
                    <RestartIconButton onClick={() => withLoading(doAction("retry"))} />
                  )}
                  {(status.label === "Error" || status.label === "Stopped") && w.can_resume_from_error && (
                    <ResumeIconButton onClick={() => withLoading(doAction("resume-from-error"))} title="Retry from last failure" />
                  )}
                  {isActive && status.label === "Running" && (
                    <PauseIconButton onClick={() => withLoading(doAction("pause"))} />
                  )}
                  {isActive && status.label === "Paused" && (
                    <ResumeIconButton onClick={() => withLoading(doAction("resume"))} />
                  )}
                  {isActive && (
                    <StopIconButton onClick={() => confirmAction("Stop", "stop", doAction("stop"))} />
                  )}
                </div>
              </div>
              <div className="text-sm font-mono text-gray-300 mt-0.5">{stateDisplay}</div>
              <div className="mt-2">
                <ProgressBar pct={pct} total={total} filled={filled} color={status.color} />
              </div>
            </>
          )}
        </div>

        {/* Always-visible sections */}
        {workflowDefs.length > 0 && (
          <WorkflowDefButtons
            definitions={workflowDefs}
            runStates={w.workflow_def_runs || {}}
            ticketKey={w.ticket_key}
            onRefresh={onRefresh}
            mainRunning={isActive}
          />
        )}
        {w.run_commands && w.run_commands.length > 0 && (
          <RunCommands
            ticketKey={w.ticket_key}
            commands={w.run_commands}
            withLoading={withLoading}
          />
        )}

        {/* Console output button — always visible, disabled until workflow has run */}
        <div className="border-t border-gray-800/60" />
        <button
          onClick={hasTerminalLines ? () => setConsoleOpen(true) : undefined}
          disabled={!hasTerminalLines}
          className={`flex items-center leading-none gap-1 text-xs transition-colors ${
            hasTerminalLines
              ? "text-gray-500 hover:text-gray-300 cursor-pointer"
              : "text-gray-700 cursor-not-allowed"
          }`}
        >
          <TerminalIcon />
          Show console output
        </button>

        {/* Bottom-right icons: editor, terminal, port mappings */}
        {(mergedPorts.length > 0 || (isTerminal && w.can_open_editor)) && (
          <div className="absolute bottom-3 right-3 z-10 flex items-center gap-2">

            {/* Editor icon */}
            {isTerminal && w.can_open_editor && (
              <div className="relative">
                {openMenu === "editor" && w.editor_url && (
                  <>
                    <div className="fixed inset-0" onClick={() => setOpenMenu(null)} />
                    <div className="absolute bottom-full mb-2 right-0 bg-gray-800 border border-gray-700 rounded-lg py-1.5 shadow-xl z-20 min-w-[160px]">
                      <div className="px-3 py-1 text-xs text-gray-500 font-medium border-b border-gray-700/60 mb-1">Editor</div>
                      <a
                        href={w.editor_url}
                        target="_blank"
                        rel="noopener"
                        className="flex items-center leading-none gap-2 px-3 py-1.5 text-xs text-gray-300 hover:bg-gray-700 hover:text-white transition-colors"
                        onClick={() => setOpenMenu(null)}
                      >
                        <ExternalLinkIcon />
                        Open in browser
                      </a>
                      <button
                        onClick={() => { setOpenMenu(null); withLoading(closeEditor); }}
                        className="flex w-full items-center leading-none gap-2 px-3 py-1.5 text-xs text-red-400 hover:bg-gray-700 hover:text-red-300 transition-colors"
                      >
                        <StopSquareIcon />
                        Stop editor
                      </button>
                    </div>
                  </>
                )}
                <button
                  onClick={() => {
                    if (w.editor_url) {
                      setOpenMenu((o) => o === "editor" ? null : "editor");
                    } else {
                      withLoading(openEditor, "Setting up a secure connection to an editor");
                    }
                  }}
                  title={w.editor_url ? "Editor (open)" : "Open editor"}
                  className={`cursor-pointer transition-colors ${w.editor_url ? "text-green-400" : "text-gray-500 hover:text-gray-300"}`}
                >
                  <EditorIcon />
                </button>
              </div>
            )}

            {/* Terminal icon */}
            {isTerminal && w.can_open_editor && (
              <div className="relative">
                {openMenu === "terminal" && w.terminal_url && (
                  <>
                    <div className="fixed inset-0" onClick={() => setOpenMenu(null)} />
                    <div className="absolute bottom-full mb-2 right-0 bg-gray-800 border border-gray-700 rounded-lg py-1.5 shadow-xl z-20 min-w-[160px]">
                      <div className="px-3 py-1 text-xs text-gray-500 font-medium border-b border-gray-700/60 mb-1">Terminal</div>
                      <a
                        href={w.terminal_url}
                        target="_blank"
                        rel="noopener"
                        className="flex items-center leading-none gap-2 px-3 py-1.5 text-xs text-gray-300 hover:bg-gray-700 hover:text-white transition-colors"
                        onClick={() => setOpenMenu(null)}
                      >
                        <ExternalLinkIcon />
                        Open in browser
                      </a>
                      <button
                        onClick={() => { setOpenMenu(null); withLoading(closeEditor); }}
                        className="flex w-full items-center leading-none gap-2 px-3 py-1.5 text-xs text-red-400 hover:bg-gray-700 hover:text-red-300 transition-colors"
                      >
                        <StopSquareIcon />
                        Stop terminal
                      </button>
                    </div>
                  </>
                )}
                <button
                  onClick={() => {
                    if (w.terminal_url) {
                      setOpenMenu((o) => o === "terminal" ? null : "terminal");
                    } else {
                      withLoading(openTerminal, "Setting up a secure connection to a terminal");
                    }
                  }}
                  title={w.terminal_url ? "Terminal (open)" : "Open terminal"}
                  className={`cursor-pointer transition-colors ${w.terminal_url ? "text-green-400" : "text-gray-500 hover:text-gray-300"}`}
                >
                  <TerminalIcon className="w-4 h-4" />
                </button>
              </div>
            )}

            {/* Port mappings icon */}
            {mergedPorts.length > 0 && (
              <div className="relative">
                {openMenu === "port" && (
                  <>
                    <div className="fixed inset-0" onClick={() => setOpenMenu(null)} />
                    <div className="absolute bottom-full mb-2 right-0 bg-gray-800 border border-gray-700 rounded-lg py-1.5 shadow-xl z-20 min-w-[180px]">
                      <div className="px-3 py-1 text-xs text-gray-500 font-medium border-b border-gray-700/60 mb-1">Port mappings</div>
                      {mergedPorts.map(([cp, hp]) => (
                        <a
                          key={`${cp}-${hp}`}
                          href={`http://localhost:${hp}`}
                          target="_blank"
                          rel="noopener"
                          className="flex items-center leading-none gap-2 px-3 py-1.5 text-xs text-gray-300 hover:bg-gray-700 hover:text-white transition-colors"
                        >
                          <PortIcon />
                          {cp} → localhost:{hp}
                        </a>
                      ))}
                    </div>
                  </>
                )}
                <button
                  onClick={() => setOpenMenu((o) => o === "port" ? null : "port")}
                  title="Port mappings"
                  className="text-green-400 cursor-pointer"
                >
                  <MonitorIcon />
                </button>
              </div>
            )}

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

      {consoleOpen && effectiveTs && (
        <ConsoleOutputModal state={effectiveTs} onClose={() => setConsoleOpen(false)} />
      )}

      {deleteConfirmOpen && (
        <DeleteConfirmModal
          ticketKey={w.ticket_key}
          showMarkDone={(w.ticketing_system === "jira" || w.ticketing_system === "github") && w.can_mark_done}
          onMarkDoneAndDelete={() => {
            setDeleteConfirmOpen(false);
            withLoading(async () => {
              await doAction("mark-done")();
              await doAction("delete")();
            });
          }}
          onDelete={() => {
            setDeleteConfirmOpen(false);
            withLoading(doAction("delete"));
          }}
          onCancel={() => setDeleteConfirmOpen(false)}
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

function PortIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M13.828 10.172a4 4 0 00-5.656 0l-4 4a4 4 0 105.656 5.656l1.102-1.101m-.758-4.899a4 4 0 005.656 0l4-4a4 4 0 00-5.656-5.656l-1.1 1.1" />
    </svg>
  );
}

function MonitorIcon() {
  return (
    <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.75}>
      <rect x="2" y="3" width="20" height="14" rx="2" strokeLinejoin="round" />
      <path strokeLinecap="round" strokeLinejoin="round" d="M8 21h8M12 17v4" />
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
      <div>
        <div className="text-xs text-gray-500 mb-1.5">Commands</div>
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

function ExternalLinkIcon({ className }: { className?: string }) {
  return (
    <svg className={className ?? "w-3 h-3"} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M18 13v6a2 2 0 01-2 2H5a2 2 0 01-2-2V8a2 2 0 012-2h6M15 3h6v6M10 14L21 3" />
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

function TerminalIcon({ className }: { className?: string }) {
  return (
    <svg className={className ?? "w-3 h-3"} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M8 9l3 3-3 3m5 0h3" />
      <rect x="3" y="3" width="18" height="18" rx="2" strokeLinejoin="round" />
    </svg>
  );
}

function EditorIcon() {
  return (
    <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.75}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M10 20l4-16m4 4l4 4-4 4M6 16l-4-4 4-4" />
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

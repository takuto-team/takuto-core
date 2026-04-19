import { useState, useCallback } from "react";
import { api, apiPost } from "../api/client";
import type { WorkflowSummary } from "../api/types";
import type { TerminalState } from "../hooks/useWorkflows";
import { TerminalOutput } from "./TerminalOutput";
import { ConfirmModal } from "./modals/ConfirmModal";

interface Props {
  workflow: WorkflowSummary;
  terminalState?: TerminalState;
  onRefresh: () => void;
  onShowDescription: (ticketKey: string, summary: string) => void;
  onReport: (ticketKey: string) => void;
}

interface StatusInfo {
  label: string;
  color: string;
}

function getStatusInfo(state: string): StatusInfo {
  const s = state.toLowerCase();
  if (s === "done" || s.startsWith("completed")) return { label: "Completed", color: "green" };
  if (s.startsWith("error")) return { label: "Error", color: "red" };
  if (s === "paused") return { label: "Paused", color: "yellow" };
  if (s === "stopped") return { label: "Stopped", color: "gray" };
  return { label: "Running", color: "blue" };
}

function progressInfo(w: WorkflowSummary) {
  const pct = Math.max(0, Math.min(100, Math.round(w.progress_percent || 0)));
  const total = w.progress_steps_total > 0 ? Math.floor(w.progress_steps_total) : 0;
  const filled = total > 0 ? Math.min(total, Math.round((pct * total) / 100)) : 0;
  return { pct, total, filled };
}

export function WorkflowCard({ workflow: w, terminalState: ts, onRefresh, onShowDescription, onReport }: Props) {
  const [loading, setLoading] = useState(false);
  const [confirm, setConfirm] = useState<{ action: string; label: string; fn: () => Promise<void> } | null>(null);

  const status = getStatusInfo(w.state);
  const { pct, total, filled } = progressInfo(w);
  const prUrl = w.pr_url?.trim() || "";

  const withLoading = useCallback(
    async (fn: () => Promise<void>) => {
      setLoading(true);
      try {
        await fn();
        onRefresh();
      } catch (e) {
        alert(e instanceof Error ? e.message : "Action failed");
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
    await apiPost(`/api/workflows/${encodeURIComponent(w.ticket_key)}/close-editor`);
  };

  // Step display
  let stepLabel = "Current step";
  if (status.label === "Completed") stepLabel = "Completed";
  else if (status.label === "Error") stepLabel = "Failed at step";
  else if (status.label === "Paused") stepLabel = "Paused at step";
  else if (status.label === "Stopped") stepLabel = "Stopped at step";

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

  const isActive = status.label === "Running" || status.label === "Paused";

  return (
    <>
      <div className={`workflow-card border ${borderClass} transition-colors ${status.label === "Stopped" ? "opacity-60 hover:opacity-80" : ""} relative`}>
        {loading && (
          <div className="absolute inset-0 bg-gray-900/80 z-10 flex items-center justify-center rounded-xl">
            <span className="text-sm text-gray-400">Working...</span>
          </div>
        )}

        {/* Header */}
        <div className="flex items-center justify-between gap-3 min-w-0">
          <div className="flex items-center gap-2 min-w-0 flex-1">
            <span className={`font-mono text-sm text-${status.color}-400 font-medium`}>{w.ticket_key}</span>
            <StatusBadge status={status} />
          </div>
          {prUrl && (
            <div className="flex items-center gap-2 flex-shrink-0">
              {w.pr_merged && (
                <span className="inline-flex items-center gap-1 text-xs text-purple-400 bg-purple-500/10 px-2 py-0.5 rounded-full border border-purple-500/20">
                  Merged
                </span>
              )}
              <a
                href={prUrl}
                target="_blank"
                rel="noopener noreferrer"
                className="text-xs text-blue-400 hover:text-blue-300 bg-blue-500/10 px-2 py-0.5 rounded-full border border-blue-500/20"
              >
                Show PR
              </a>
            </div>
          )}
        </div>

        {/* Summary */}
        <h3 className="text-sm font-medium text-gray-200 truncate">{w.ticket_summary}</h3>

        {/* Progress */}
        <div className="bg-gray-800/50 rounded-lg px-3 py-2.5">
          <div className="text-xs text-gray-500 mb-1">{stepLabel}</div>
          <div className="text-sm font-mono text-gray-300">{stateDisplay}</div>
          <div className="mt-2">
            <ProgressBar pct={pct} total={total} filled={filled} color={status.color} />
          </div>
        </div>

        {/* Actions */}
        <div className="flex flex-wrap gap-2">
          {!w.jira_available ? null : (
            <ActionBtn
              color="sky"
              onClick={() => window.open(w.jira_browse_url, "_blank")}
            >
              Go to ticket
            </ActionBtn>
          )}
          <ActionBtn color="violet" onClick={() => onShowDescription(w.ticket_key, w.ticket_summary)}>
            Show description
          </ActionBtn>

          {status.label === "Running" && (
            <ActionBtn color="yellow" onClick={() => withLoading(doAction("pause"))}>Pause</ActionBtn>
          )}
          {status.label === "Paused" && (
            <ActionBtn color="green" onClick={() => withLoading(doAction("resume"))}>Resume</ActionBtn>
          )}
          {w.can_resume_from_error && (
            <ActionBtn color="teal" onClick={() => withLoading(doAction("resume-from-error"))}>
              Retry from last failure
            </ActionBtn>
          )}
          {["Error", "Stopped", "Completed"].includes(status.label) && (
            <ActionBtn color="blue" onClick={() => withLoading(doAction("retry"))}>Retry from 0</ActionBtn>
          )}
          {w.can_address_pr_comments && (
            <ActionBtn color="indigo" onClick={() => withLoading(doAction("address-pr-comments"))}>
              Address PR Comments
            </ActionBtn>
          )}
          {w.can_merge_base && (
            <ActionBtn color="amber" onClick={() => withLoading(doAction("merge-base-branch"))}>
              Merge Base Branch
            </ActionBtn>
          )}
          {w.can_mark_done && (
            <ActionBtn
              color="emerald"
              onClick={() => confirmAction("Mark as Done", "mark-done", doAction("mark-done"))}
            >
              Mark as Done
            </ActionBtn>
          )}
          {w.can_delete && (
            <ActionBtn
              color="gray"
              onClick={() => confirmAction("Delete", "delete", doAction("delete"))}
            >
              Delete
            </ActionBtn>
          )}

          {/* Editor / Terminal */}
          {w.can_open_editor && (
            <>
              {w.editor_url ? (
                <a
                  href={w.editor_url}
                  target="_blank"
                  rel="noopener"
                  className="action-btn bg-violet-500/10 text-violet-300 border-violet-500/25 hover:bg-violet-500/20 inline-flex items-center gap-1"
                >
                  Editor &#x2197;
                </a>
              ) : (
                <ActionBtn color="violet" onClick={() => withLoading(openEditor)}>Open editor</ActionBtn>
              )}
              {w.terminal_url ? (
                <a
                  href={w.terminal_url}
                  target="_blank"
                  rel="noopener"
                  className="action-btn bg-orange-500/10 text-orange-300 border-orange-500/25 hover:bg-orange-500/20 inline-flex items-center gap-1"
                >
                  Terminal &#x2197;
                </a>
              ) : (
                <ActionBtn color="orange" onClick={() => withLoading(openTerminal)}>Open terminal</ActionBtn>
              )}
              {w.editor_url && (
                <ActionBtn color="violet" onClick={() => withLoading(closeEditor)}>Close editor</ActionBtn>
              )}
            </>
          )}

          <ActionBtn color="gray" onClick={() => onReport(w.ticket_key)}>Report</ActionBtn>
        </div>

        {/* Terminal output for active workflows */}
        {isActive && <TerminalOutput state={ts} />}
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

function ActionBtn({
  color,
  onClick,
  children,
}: {
  color: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={`action-btn bg-${color}-500/10 text-${color}-400 border-${color}-500/25 hover:bg-${color}-500/20`}
    >
      {children}
    </button>
  );
}

function StatusBadge({ status }: { status: StatusInfo }) {
  const { label, color } = status;
  return (
    <span
      className={`inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full bg-${color}-500/15 text-${color}-400 border border-${color}-500/20`}
    >
      {label === "Completed" && <CheckIcon />}
      {label === "Error" && <XIcon />}
      {label === "Running" && <span className={`w-1.5 h-1.5 bg-${color}-400 rounded-full animate-pulse`} />}
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

function ProgressBar({ pct, total, filled, color }: { pct: number; total: number; filled: number; color: string }) {
  if (total <= 0) {
    return (
      <div className="w-full bg-gray-700 rounded-full h-1.5 overflow-hidden">
        <div className={`bg-${color}-500 h-1.5 rounded-full transition-all`} style={{ width: `${pct}%` }} />
      </div>
    );
  }
  return (
    <div className="flex gap-0.5">
      {Array.from({ length: total }, (_, i) => (
        <div
          key={i}
          className={`h-2 flex-1 rounded-full transition-all ${
            i < filled ? `bg-${color}-500` : "bg-gray-600"
          }`}
        />
      ))}
    </div>
  );
}

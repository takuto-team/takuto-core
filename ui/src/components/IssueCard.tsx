// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { WorkflowDefinition, WorkflowSummary } from "../api/types";
import type { TerminalState } from "../hooks/useWorkflows";
import { useIssueCardController } from "../hooks/useIssueCardController";
import { ConnectionOverlay } from "./ConnectionOverlay";
import { DeleteIconButton } from "./DeleteIconButton";
import { IssueCardFooter } from "./IssueCard/IssueCardFooter";
import { IssueCardHeader } from "./IssueCard/IssueCardHeader";
import { IssueCardModals } from "./IssueCard/IssueCardModals";
import { IssueCardProgress } from "./IssueCard/IssueCardProgress";
import { buildIssueCardView } from "./IssueCard/issueCardView";
import { RunCommands } from "./RunCommands";
import { WorkflowDefButtons } from "./WorkflowDefButtons";
import { ExternalLinkIcon, TerminalIcon } from "./icons";

interface Props {
  workflow: WorkflowSummary;
  terminalState?: TerminalState;
  dynamicForwards: [number, string][];
  workflowDefs: WorkflowDefinition[];
  onRefresh: () => void;
  onShowDescription: (ticketKey: string, summary: string, description?: string) => void;
  onReport: (ticketKey: string) => void;
}

export function IssueCard({
  workflow: w,
  terminalState: ts,
  dynamicForwards,
  workflowDefs,
  onRefresh,
  onShowDescription,
  onReport,
}: Props) {
  const view = buildIssueCardView(w, ts, dynamicForwards);
  // First open on a finished workflow may rebuild the worktree + container.
  const preparingWorkspace = view.isTerminal && !w.editor_url;
  const ctl = useIssueCardController(w.ticket_key, onRefresh, preparingWorkspace);
  const showMarkDone =
    (w.ticketing_system === "jira" || w.ticketing_system === "github") && w.can_mark_done;

  return (
    <>
      <div
        className={`work-item-card border ${view.borderClass} transition-colors ${
          view.status.status === "stopped" ? "opacity-60 hover:opacity-80" : ""
        } relative`}
      >
        {ctl.loading && (
          <div className="absolute inset-0 bg-gray-900/90 z-10 flex items-center justify-center rounded-xl">
            {ctl.loading !== "generic" ? (
              <ConnectionOverlay message={ctl.loading} />
            ) : (
              <span className="text-sm text-gray-400">Working...</span>
            )}
          </div>
        )}

        {w.can_delete && (
          <div className="absolute top-1 right-1 translate-x-1/2 -translate-y-1/2 z-10">
            <DeleteIconButton onClick={ctl.onRequestDelete} />
          </div>
        )}

        <IssueCardHeader workflow={w} status={view.status} prUrl={view.prUrl} />

        <div className="flex items-center justify-between gap-4 min-w-0">
          <span className="text-sm font-medium text-white truncate min-w-0">{w.ticket_summary}</span>
          <button
            onClick={() => onShowDescription(w.ticket_key, w.ticket_summary, w.ticket_description)}
            className="flex-shrink-0 flex items-center gap-1 text-xs text-gray-500 hover:text-gray-300 transition-colors cursor-pointer"
          >
            Show details <ExternalLinkIcon className="w-3 h-3" />
          </button>
        </div>

        <IssueCardProgress
          status={view.status}
          stepLabel={view.stepLabel}
          stateDisplay={view.stateDisplay}
          pct={view.pct}
          total={view.total}
          filled={view.filled}
          duration={view.duration}
          isActive={view.isActive}
          prepState={view.prepState}
          hasReport={w.has_report}
          canResumeFromError={w.can_resume_from_error}
          onRetry={ctl.onRetry}
          onResumeFromError={ctl.onResumeFromError}
          onPause={ctl.onPause}
          onResume={ctl.onResume}
          onStop={ctl.onStop}
          onReport={() => onReport(w.ticket_key)}
        />

        {/* WorkflowDefButtons renders the empty-state banner itself when the flow list is empty. */}
        <WorkflowDefButtons
          definitions={workflowDefs}
          runStates={w.definition_runs || {}}
          ticketKey={w.ticket_key}
          onRefresh={onRefresh}
          mainRunning={view.isActive}
          disabled={view.prepState === "preparing"}
        />
        {w.run_commands && w.run_commands.length > 0 && (
          <RunCommands
            ticketKey={w.ticket_key}
            commands={w.run_commands}
            withLoading={ctl.withLoading}
            disabled={view.prepState === "preparing"}
          />
        )}

        {/* Console output button — always visible, disabled until workflow has run */}
        <div className="border-t border-gray-800/60" />
        <button
          onClick={view.hasTerminalLines ? ctl.onShowConsole : undefined}
          disabled={!view.hasTerminalLines}
          className={`flex items-center leading-none gap-1 text-xs transition-colors ${
            view.hasTerminalLines
              ? "text-gray-500 hover:text-gray-300 cursor-pointer"
              : "text-gray-700 cursor-not-allowed"
          }`}
        >
          <TerminalIcon />
          Show console output
        </button>

        <IssueCardFooter
          canOpenEditor={w.can_open_editor}
          editorUrl={w.editor_url}
          terminalUrl={w.terminal_url}
          ports={view.mergedPorts}
          openMenu={ctl.openMenu}
          onSetMenu={ctl.setOpenMenu}
          onOpenEditor={ctl.onOpenEditor}
          onOpenTerminal={ctl.onOpenTerminal}
          onCloseEditor={ctl.onCloseEditor}
          onCloseTerminal={ctl.onCloseTerminal}
        />
      </div>

      <IssueCardModals
        ticketKey={w.ticket_key}
        showMarkDone={showMarkDone}
        confirm={ctl.confirm}
        consoleState={view.effectiveTs}
        consoleOpen={ctl.consoleOpen}
        deleteOpen={ctl.deleteOpen}
        onConfirm={ctl.onConfirm}
        onConfirmCancel={ctl.onConfirmCancel}
        onConsoleClose={ctl.onConsoleClose}
        onMarkDoneAndDelete={ctl.onMarkDoneAndDelete}
        onDelete={ctl.onDelete}
        onDeleteCancel={ctl.onDeleteCancel}
      />
    </>
  );
}

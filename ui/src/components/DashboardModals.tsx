// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `DashboardModals` — dispatcher that renders exactly one of the five
 * Dashboard modals based on the discriminated union returned by
 * `useDashboardModals`.
 *
 * The report-modal gate `workflows[reportKey] && generate_report &&
 * has_report` is preserved verbatim from the pre-extraction shell.
 */

import { TicketPickerModal } from "./modals/TicketPickerModal";
import { TicketDetailModal } from "./modals/TicketDetailModal";
import { PasteDescriptionModal } from "./modals/PasteDescriptionModal";
import { ReportModal } from "./modals/ReportModal";
import { NoJiraAlertModal } from "./modals/NoJiraAlertModal";
import type { ConfigResponse, WorkflowSummary } from "../api/types";
import type { DashboardModalState } from "../hooks/useDashboardModals";

interface Props {
  modal: DashboardModalState;
  close: () => void;
  ticketingSystem: string;
  activeRepoName: string | null;
  config: ConfigResponse | null;
  /** Used to gate the report modal — only renders when the referenced
   *  workflow exists, has `generate_report`, and has `has_report`. */
  workflows: Record<string, WorkflowSummary>;
  onTicketSelected: (key: string, summary: string, description?: string, url?: string) => void;
  onAddToDashboard: (description: string, summary: string, repositoryId: string) => Promise<void>;
  onPasteSubmit: (name: string, description: string, repositoryId: string) => Promise<void>;
  /** Called after a successful save inside the detail modal so the
   *  page can refresh workflow data. */
  onSaved: () => void;
}

export function DashboardModals({
  modal, close, ticketingSystem, activeRepoName, config, workflows,
  onTicketSelected, onAddToDashboard, onPasteSubmit, onSaved,
}: Props) {
  switch (modal.kind) {
    case "none":
      return null;
    case "picker":
      return (
        <TicketPickerModal
          ticketingSystem={ticketingSystem}
          activeRepoName={activeRepoName}
          onSelect={onTicketSelected}
          onClose={close}
        />
      );
    case "paste":
      return <PasteDescriptionModal onSubmit={onPasteSubmit} onClose={close} />;
    case "nojira":
      return <NoJiraAlertModal onClose={close} />;
    case "detail":
      return (
        <TicketDetailModal
          ticketKey={modal.ticket.key}
          summary={modal.ticket.summary}
          description={modal.ticket.description}
          ticketingSystem={ticketingSystem}
          showStartButton={modal.ticket.showStart}
          improveTimeoutSecs={config?.agent?.improve_timeout_secs}
          onStart={onAddToDashboard}
          onClose={close}
          onSaved={onSaved}
        />
      );
    case "report": {
      const wf = workflows[modal.reportKey];
      if (!wf || !wf.generate_report || !wf.has_report) return null;
      return <ReportModal workflow={wf} onClose={close} />;
    }
  }
}

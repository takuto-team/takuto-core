// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useCallback, useRef, useState, useMemo } from "react";
import { Link } from "react-router-dom";
import { apiPost, listRepoAccess } from "../api/client";
import type { WorkflowEvent } from "../api/types";
import { useToast } from "../hooks/useToast";
import { useConfig } from "../hooks/useConfig";
import { useOnboardingStatus } from "../hooks/useOnboardingStatus";
import { useMyRepositories } from "../hooks/useMyRepositories";
import { useWorkflowDefinitions } from "../hooks/useWorkflowDefinitions";
import { useDashboardModals } from "../hooks/useDashboardModals";
import { useWebSocket } from "../hooks/useWebSocket";
import { useWorkflows } from "../hooks/useWorkflows";
import { usePolling } from "../hooks/usePolling";
import { Header } from "../components/Header";
import { PollingLabel } from "../components/PollingLabel";
import { SummaryStats } from "../components/SummaryStats";
import { workflowMatchesStatus, type StatusFilterKey } from "../components/statusFilter";
import { WorkflowGrid } from "../components/WorkflowGrid";
import { DashboardModals } from "../components/DashboardModals";
import { ConfirmModal } from "../components/modals/ConfirmModal";
import { OnboardingBanner } from "../components/OnboardingBanner";
import { SystemErrorAlert } from "../components/SystemErrorAlert";
import { handleProviderChangedEvent } from "../utils/providerChanged";

interface Props {
  onLogout: () => void;
  authEnabled: boolean;
  /**
   * Whether the current user has the `admin` role. Drives the OnboardingBanner
   * deep-links (admin-only CTAs collapse to greyed-out hints for non-admins).
   * Optional + defaults to false so existing call sites don't break.
   */
  isAdmin?: boolean;
}

export function Dashboard({ onLogout, authEnabled, isAdmin = false }: Props) {
  const { showToast } = useToast();
  const config = useConfig();
  const { systemStatus, refresh: refreshOnboardingStatus } = useOnboardingStatus();
  const { myRepos, hasAnyRepo, activeRepoName, setActiveRepoName } = useMyRepositories();
  // Counts are scoped to the active repo so the summary bar matches the grid.
  const wf = useWorkflows(activeRepoName);
  const { workflows, orderKeys, terminalStates, dynamicForwards, systemErrors, counts,
          dismissError, fetchWorkflows, fetchCounts, handleEvent, resetState: _resetState } = wf;
  const { workflowDefs, refresh: fetchWorkflowDefs, scheduleRefresh: scheduleWorkflowDefsRefresh } =
    useWorkflowDefinitions();
  const modals = useDashboardModals(config);

  // Live GitHub-access check for the active repo. Re-runs on every switch and on
  // page load (no caching) so revoked access surfaces a warning and restored
  // access clears it. The repo stays selected — the user can return to it.
  const [noAccessRepo, setNoAccessRepo] = useState<string | null>(null);
  useEffect(() => {
    if (!activeRepoName) {
      setNoAccessRepo(null);
      return;
    }
    let cancelled = false;
    listRepoAccess()
      .then((rows) => {
        if (cancelled) return;
        const entry = rows.find((r) => r.name === activeRepoName);
        setNoAccessRepo(entry && !entry.accessible ? activeRepoName : null);
      })
      .catch(() => {
        /* best-effort: a failed check shows no warning */
      });
    return () => {
      cancelled = true;
    };
  }, [activeRepoName]);

  // Clicking a summary-counter card filters the grid to that status; clicking
  // the active card again clears the filter.
  const [statusFilter, setStatusFilter] = useState<StatusFilterKey | null>(null);
  const visibleOrderKeys = useMemo(() => {
    if (!statusFilter) return orderKeys;
    return orderKeys.filter((k) => {
      const w = workflows[k];
      return w != null && workflowMatchesStatus(w, statusFilter);
    });
  }, [orderKeys, workflows, statusFilter]);

  // Wrap handleEvent to also re-fetch definitions + onboarding on relevant events.
  const handleEventWithDefs = useCallback((evt: WorkflowEvent) => {
    handleEvent(evt);
    if (evt.event_type === "workflow_definitions_changed" ||
        evt.event_type === "work_item_updated" ||
        evt.event_type === "step_completed") {
      scheduleWorkflowDefsRefresh();
    }
    if (evt.event_type === "onboarding_changed") refreshOnboardingStatus();
    if (evt.event_type === "provider_changed") {
      handleProviderChangedEvent(evt, { showToast, refreshOnboardingStatus });
    }
  }, [handleEvent, scheduleWorkflowDefsRefresh, refreshOnboardingStatus, showToast]);

  const { connected } = useWebSocket(handleEventWithDefs);
  const prevConnected = useRef(false);
  const polling = usePolling();

  // Consolidated mount + reconnect refetch (designer plan). Fires once on
  // initial WebSocket connect (prevConnected.current false → connected true)
  // and again on every reconnect edge. useWorkflowDefinitions self-fetches
  // on its own mount, so the call here is the reconnect-side refresh.
  useEffect(() => {
    if (connected && !prevConnected.current) {
      fetchWorkflows();
      fetchWorkflowDefs();
      fetchCounts();
    }
    prevConnected.current = connected;
  }, [connected, fetchWorkflows, fetchWorkflowDefs, fetchCounts]);

  const ticketingSystem = config?.ticketing_system || "none";
  const dryMode = config?.general?.dry_mode || false;
  const githubAppConfigured = config?.github_app_configured || false;
  const githubAppInstallationId = config?.github?.app_installation_id || undefined;

  const handleAddWorkflow = useCallback(() => {
    if (ticketingSystem === "none") modals.openPaste();
    else modals.openPicker();
  }, [ticketingSystem, modals]);

  const handleTicketSelected = useCallback(
    (key: string, summary: string, description?: string, url?: string) =>
      modals.openDetail({ key, summary, description, url, showStart: true }),
    [modals]
  );

  const handleAddToDashboard = useCallback(
    async (description: string, summary: string, repositoryId: string) => {
      if (modals.modal.kind !== "detail") return;
      if (!repositoryId) { showToast("Pick a repository before adding a work item."); return; }
      const ticket = modals.modal.ticket;
      try {
        const res = await apiPost("/api/work-items/start-manual", {
          ticket_key: ticket.key, ticket_summary: summary, ticket_description: description,
          repository_id: repositoryId,
          ...(ticket.url ? { issue_url: ticket.url } : {}),
        });
        if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
        modals.close();
        fetchWorkflows();
      } catch (e) {
        showToast(e instanceof Error ? e.message : "Failed to add work item");
      }
    }, [modals, fetchWorkflows, showToast]);

  const handlePasteSubmit = useCallback(
    async (name: string, description: string, repositoryId: string) => {
      if (!repositoryId) { showToast("Pick a repository before adding a work item."); return; }
      try {
        const res = await apiPost("/api/work-items/start-manual", {
          ticket_key: name, ticket_summary: name || "Manual item",
          ticket_description: description, repository_id: repositoryId,
        });
        if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
        modals.close();
        fetchWorkflows();
      } catch (e) {
        showToast(e instanceof Error ? e.message : "Failed to add work item");
      }
    }, [modals, fetchWorkflows, showToast]);

  const handleShowDescription = useCallback((key: string, summary: string, description?: string) => {
    // For Jira, don't pass cached description — the modal fetches fresh from the preview API.
    // For None and GitHub, use the in-memory description (it's the source of truth or a good cache).
    const desc = ticketingSystem === "jira" ? undefined : description;
    modals.openDetail({ key, summary, description: desc, showStart: false });
  }, [ticketingSystem, modals]);

  // Empty-state CTA links to My Repositories; per-card badges show repo per work item.
  const repoExists = (hasAnyRepo ?? true);

  return (
    <div className="min-h-screen flex flex-col">
      <PollingLabel
        paused={polling.paused} toggling={polling.toggling}
        ticketingSystem={ticketingSystem} onToggle={polling.toggle}
      />
      <Header
        connected={connected} authEnabled={authEnabled}
        githubAppConfigured={githubAppConfigured}
        githubAppInstallationId={githubAppInstallationId}
        githubAppName={config?.github_app_name} isAdmin={isAdmin} onLogout={onLogout}
        repos={myRepos ?? []} activeRepoName={activeRepoName}
        onSelectRepo={setActiveRepoName}
      />
      <OnboardingBanner
        status={systemStatus}
        legacyPreflightError={config?.preflight_error ?? null}
        isAdmin={isAdmin}
      />
      {dryMode && (
        <div className="bg-amber-900/30 border-b border-amber-700/50 px-4 py-2 text-center text-xs text-amber-300">
          Dry mode is enabled &mdash; Jira/GitHub side effects are skipped
        </div>
      )}
      <main className="flex-1 w-full px-4 sm:px-6 lg:px-8 py-6 flex flex-col gap-6">
        <SummaryStats counts={counts} activeFilter={statusFilter} onSelectFilter={setStatusFilter} />
        {hasAnyRepo === false ? (
          <div className="text-center py-16">
            <p className="text-gray-500 text-sm mb-4">
              You haven't added any repositories yet. Add a repository to get started.
            </p>
            <Link
              to="/config.html?tab=repositories"
              className="inline-block text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 transition-colors cursor-pointer"
            >
              Go to My Repositories
            </Link>
          </div>
        ) : statusFilter && visibleOrderKeys.length === 0 ? (
          <div className="text-center py-16">
            <p className="text-gray-500 text-sm mb-4">No {statusFilter} items.</p>
            <button
              onClick={() => setStatusFilter(null)}
              className="inline-block text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 transition-colors cursor-pointer"
            >
              Show all items
            </button>
          </div>
        ) : (
          <WorkflowGrid
            workflows={workflows} orderKeys={visibleOrderKeys}
            terminalStates={terminalStates} dynamicForwards={dynamicForwards}
            workflowDefs={workflowDefs} onRefresh={fetchWorkflows}
            onShowDescription={handleShowDescription} onReport={modals.openReport}
            onAddWorkflow={handleAddWorkflow}
            canAddWorkflow={hasAnyRepo === true} repoExists={repoExists}
            onSetupProject={undefined} activeRepoName={activeRepoName}
          />
        )}
      </main>
      <DashboardModals
        modal={modals.modal} close={modals.close}
        ticketingSystem={ticketingSystem} activeRepoName={activeRepoName}
        config={config} workflows={workflows}
        onTicketSelected={handleTicketSelected}
        onAddToDashboard={handleAddToDashboard}
        onPasteSubmit={handlePasteSubmit}
        onSaved={fetchWorkflows}
      />
      {noAccessRepo && (
        <ConfirmModal
          title="No access to this repository"
          message={`The connected GitHub App no longer has access to "${noAccessRepo}". You can still select it again if access is restored later.`}
          confirmLabel="OK"
          onConfirm={() => setNoAccessRepo(null)}
          onCancel={() => setNoAccessRepo(null)}
        />
      )}
      <footer className="py-3 text-center">
        <span className="text-xs text-gray-600">Takuto v{__APP_VERSION__}</span>
      </footer>
      <SystemErrorAlert errors={systemErrors} onDismiss={dismissError} />
    </div>
  );
}

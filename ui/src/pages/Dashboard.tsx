// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect, useCallback, useRef } from "react";
import { Link } from "react-router-dom";
import {
  apiJson,
  apiPost,
  fetchOnboardingStatus,
  listMyRepositories,
  type RepositoryRow,
} from "../api/client";
import type {
  ConfigResponse,
  SystemStatus,
  WorkflowDefinition,
  WorkflowEvent,
} from "../api/types";
import { useToast } from "../hooks/useToast";
import { useWebSocket } from "../hooks/useWebSocket";
import { useWorkflows } from "../hooks/useWorkflows";
import { usePolling } from "../hooks/usePolling";
import { Header } from "../components/Header";
import { PollingLabel } from "../components/PollingLabel";
import { SummaryStats } from "../components/SummaryStats";
import { WorkflowGrid } from "../components/WorkflowGrid";
import { TicketPickerModal } from "../components/modals/TicketPickerModal";
import { TicketDetailModal } from "../components/modals/TicketDetailModal";
import { PasteDescriptionModal } from "../components/modals/PasteDescriptionModal";
import { ReportModal } from "../components/modals/ReportModal";
import { NoJiraAlertModal } from "../components/modals/NoJiraAlertModal";
import { OnboardingBanner } from "../components/OnboardingBanner";
import { SystemErrorAlert } from "../components/SystemErrorAlert";
import { handleProviderChangedEvent } from "../utils/providerChanged";

interface Props {
  onLogout: () => void;
  authEnabled: boolean;
}

export function Dashboard({ onLogout, authEnabled }: Props) {
  const { showToast } = useToast();
  const [config, setConfig] = useState<ConfigResponse | null>(null);
  // Phase 0 banner: `undefined` = fetch in flight, `null` = endpoint 404'd
  // (older server, fall back to ConfigResponse.preflight_error), otherwise
  // the structured status. See 04_architecture.md §1.
  const [systemStatus, setSystemStatus] = useState<SystemStatus | null | undefined>(
    undefined,
  );

  const refreshOnboardingStatus = useCallback(() => {
    fetchOnboardingStatus()
      .then(setSystemStatus)
      .catch(() => {
        // Network or 5xx — treat as "endpoint not available" so the legacy
        // preflight_error string is rendered instead of a blank banner.
        setSystemStatus(null);
      });
  }, []);
  const { workflows, orderKeys, terminalStates, dynamicForwards, systemErrors, counts, dismissError, fetchWorkflows, fetchCounts, handleEvent, resetState: _resetState } = useWorkflows();
  const [workflowDefs, setWorkflowDefs] = useState<WorkflowDefinition[]>([]);
  const defsFetchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Plan-10: track the caller's added repositories. Drives the empty-state CTA,
  // gates the "+" picker, and feeds the header repo-switcher dropdown.
  const [myRepos, setMyRepos] = useState<RepositoryRow[] | null>(null);
  const hasAnyRepo = myRepos === null ? null : myRepos.length > 0;

  const refreshHasAnyRepo = useCallback(() => {
    listMyRepositories()
      .then(setMyRepos)
      .catch(() => setMyRepos([]));
  }, []);

  // Active repo for dashboard filtering. `null` = "All repositories".
  // Persisted in localStorage so the choice survives page reloads. Scoped to
  // the caller via the same key for simplicity; if the user changes accounts
  // on the same browser the stale name is harmless — the picker just shows
  // "All repositories" because the name won't match any of `myRepos`.
  const ACTIVE_REPO_KEY = "maestro.activeRepoName";
  const [activeRepoName, setActiveRepoNameState] = useState<string | null>(() => {
    try {
      return localStorage.getItem(ACTIVE_REPO_KEY);
    } catch {
      return null;
    }
  });
  const setActiveRepoName = useCallback((name: string | null) => {
    setActiveRepoNameState(name);
    try {
      if (name === null) localStorage.removeItem(ACTIVE_REPO_KEY);
      else localStorage.setItem(ACTIVE_REPO_KEY, name);
    } catch {
      /* ignore quota / disabled storage */
    }
  }, []);

  // Sync `activeRepoName` with the user's current `myRepos` set:
  //   - Drop the saved active repo if it's no longer in the user's list (a
  //     removal in the Config tab — or another browser tab).
  //   - If exactly ONE repo remains and nothing is selected, auto-select it.
  //     This is the dashboard's expected behaviour after a removal that leaves
  //     a single repo — the user shouldn't have to manually pick from the
  //     header picker to see their items.
  useEffect(() => {
    if (myRepos === null) return;
    if (activeRepoName !== null && !myRepos.some((r) => r.name === activeRepoName)) {
      setActiveRepoName(null);
      return;
    }
    if (myRepos.length === 1 && activeRepoName === null) {
      setActiveRepoName(myRepos[0].name);
    }
  }, [myRepos, activeRepoName, setActiveRepoName]);

  const fetchWorkflowDefs = useCallback(() => {
    apiJson<WorkflowDefinition[]>("/api/workflow-definitions")
      .then(setWorkflowDefs)
      .catch(() => {});
  }, []);

  // Fetch definitions on mount
  useEffect(() => {
    fetchWorkflowDefs();
    refreshHasAnyRepo();
  }, [fetchWorkflowDefs, refreshHasAnyRepo]);

  // Phase 0 onboarding status — fetch on mount and refetch when the window
  // regains focus. The `onboarding_changed` WS event handler in
  // handleEventWithDefs covers server-pushed updates once Phase 1 ships it.
  useEffect(() => {
    refreshOnboardingStatus();
    const onFocus = () => refreshOnboardingStatus();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refreshOnboardingStatus]);

  // Wrap handleEvent to also re-fetch definitions on relevant events
  const handleEventWithDefs = useCallback(
    (evt: WorkflowEvent) => {
      handleEvent(evt);

      // Re-fetch definitions when definitions change or workflows update (debounced)
      if (
        evt.event_type === "workflow_definitions_changed" ||
        evt.event_type === "workflow_updated" ||
        evt.event_type === "step_completed"
      ) {
        if (defsFetchTimer.current) clearTimeout(defsFetchTimer.current);
        defsFetchTimer.current = setTimeout(fetchWorkflowDefs, 500);
      }

      // Phase 0: re-fetch onboarding status on the dedicated server-pushed
      // event. The event itself ships in Phase 1; declaring the handler now
      // means we'll pick up server-side state changes the moment it does.
      if (evt.event_type === "onboarding_changed") {
        refreshOnboardingStatus();
      }

      // Phase 1: admin switched the deployment-wide AI provider. Surface a
      // toast + re-fetch /api/auth/status (degraded / provider_selected may
      // have flipped) and /api/onboarding/status (banner state). No
      // credential-storage UI work yet — that ships with Phase 2.
      if (evt.event_type === "provider_changed") {
        handleProviderChangedEvent(evt, {
          showToast,
          refreshOnboardingStatus,
        });
      }
    },
    [handleEvent, fetchWorkflowDefs, refreshOnboardingStatus, showToast]
  );

  const { connected } = useWebSocket(handleEventWithDefs);
  const prevConnected = useRef(false);
  const polling = usePolling();

  // Fetch global counts on mount.
  useEffect(() => {
    fetchCounts();
  }, [fetchCounts]);

  // Re-fetch workflows, definitions, and counts on WebSocket reconnect (connected: false → true)
  useEffect(() => {
    if (connected && !prevConnected.current) {
      fetchWorkflows();
      fetchWorkflowDefs();
      fetchCounts();
    }
    prevConnected.current = connected;
  }, [connected, fetchWorkflows, fetchWorkflowDefs, fetchCounts]);

  // Modal state
  const [showPicker, setShowPicker] = useState(false);
  const [showPaste, setShowPaste] = useState(false);
  const [showNoJira, setShowNoJira] = useState(false);
  const [detailModal, setDetailModal] = useState<{
    key: string;
    summary: string;
    description?: string;
    url?: string;
    showStart: boolean;
  } | null>(null);
  const [reportKey, setReportKey] = useState<string | null>(null);

  // Load config
  useEffect(() => {
    apiJson<ConfigResponse>("/api/config")
      .then(setConfig)
      .catch(() => {});
  }, []);

  // Show no-jira alert once
  useEffect(() => {
    if (config && config.ticketing_system === "none") {
      const dismissed = sessionStorage.getItem("noJiraAlertDismissed");
      if (!dismissed) setShowNoJira(true);
    }
  }, [config]);

  const ticketingSystem = config?.ticketing_system || "none";
  const dryMode = config?.general?.dry_mode || false;
  const githubAppConfigured = config?.github_app_configured || false;
  const githubAppInstallationId = config?.github?.app_installation_id || undefined;

  const handleAddWorkflow = useCallback(() => {
    if (ticketingSystem === "none") {
      setShowPaste(true);
    } else {
      setShowPicker(true);
    }
  }, [ticketingSystem]);

  const handleTicketSelected = useCallback(
    (key: string, summary: string, description?: string, url?: string) => {
      setShowPicker(false);
      setDetailModal({ key, summary, description, url, showStart: true });
    },
    []
  );

  const handleAddToDashboard = useCallback(async (description: string, summary: string, repositoryId: string) => {
    if (!detailModal) return;
    if (!repositoryId) {
      showToast("Pick a repository before adding a workflow.");
      return;
    }
    try {
      const res = await apiPost("/api/workflows/start-manual", {
        ticket_key: detailModal.key,
        ticket_summary: summary,
        ticket_description: description,
        repository_id: repositoryId,
        ...(detailModal.url ? { issue_url: detailModal.url } : {}),
      });
      if (!res.ok) {
        const text = await res.text();
        throw new Error(text || `HTTP ${res.status}`);
      }
      setDetailModal(null);
      fetchWorkflows();
    } catch (e) {
      showToast(e instanceof Error ? e.message : "Failed to add workflow");
    }
  }, [detailModal, fetchWorkflows, showToast]);

  const handlePasteSubmit = useCallback(
    async (name: string, description: string, repositoryId: string) => {
      if (!repositoryId) {
        showToast("Pick a repository before adding a workflow.");
        return;
      }
      try {
        const res = await apiPost("/api/workflows/start-manual", {
          ticket_key: name,
          ticket_summary: name || "Manual item",
          ticket_description: description,
          repository_id: repositoryId,
        });
        if (!res.ok) {
          const text = await res.text();
          throw new Error(text || `HTTP ${res.status}`);
        }
        setShowPaste(false);
        fetchWorkflows();
      } catch (e) {
        showToast(e instanceof Error ? e.message : "Failed to add workflow");
      }
    },
    [fetchWorkflows, showToast]
  );

  const handleShowDescription = useCallback((key: string, summary: string, description?: string) => {
    // For Jira, don't pass cached description — the modal fetches fresh from the preview API.
    // For None and GitHub, use the in-memory description (it's the source of truth or a good cache).
    const desc = ticketingSystem === "jira" ? undefined : description;
    setDetailModal({ key, summary, description: desc, showStart: false });
  }, [ticketingSystem]);

  // Plan-10: there is no "active repo" any more. The empty-state CTA links to
  // the My Repositories tab; the per-card badge tells the user which repo
  // each workflow belongs to.
  const repoExists = (hasAnyRepo ?? true);

  return (
    <div className="min-h-screen flex flex-col">
      {/* Polling label at the very top */}
      <PollingLabel
        paused={polling.paused}
        toggling={polling.toggling}
        ticketingSystem={ticketingSystem}
        onToggle={polling.toggle}
      />

      <Header
        connected={connected}
        authEnabled={authEnabled}
        githubAppConfigured={githubAppConfigured}
        githubAppInstallationId={githubAppInstallationId}
        githubAppName={config?.github_app_name}
        onLogout={onLogout}
        repos={myRepos ?? []}
        activeRepoName={activeRepoName}
        onSelectRepo={setActiveRepoName}
      />

      {/* Onboarding / preflight banner — driven by /api/onboarding/status with
          a fallback to ConfigResponse.preflight_error for older servers. */}
      <OnboardingBanner
        status={systemStatus}
        legacyPreflightError={config?.preflight_error ?? null}
      />

      {/* Dry mode banner */}
      {dryMode && (
        <div className="bg-amber-900/30 border-b border-amber-700/50 px-4 py-2 text-center text-xs text-amber-300">
          Dry mode is enabled &mdash; Jira/GitHub side effects are skipped
        </div>
      )}

      <main className="flex-1 max-w-7xl mx-auto w-full px-4 sm:px-6 lg:px-8 py-6 flex flex-col gap-6">
        <SummaryStats counts={counts} />
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
        ) : (
          <WorkflowGrid
            workflows={workflows}
            orderKeys={orderKeys}
            terminalStates={terminalStates}
            dynamicForwards={dynamicForwards}
            workflowDefs={workflowDefs}
            onRefresh={fetchWorkflows}
            onShowDescription={handleShowDescription}
            onReport={setReportKey}
            onAddWorkflow={handleAddWorkflow}
            canAddWorkflow={hasAnyRepo === true}
            repoExists={repoExists}
            onSetupProject={undefined}
            activeRepoName={activeRepoName}
          />
        )}
      </main>

      {/* Modals */}
      {showPicker && (
        <TicketPickerModal
          ticketingSystem={ticketingSystem}
          activeRepoName={activeRepoName}
          onSelect={handleTicketSelected}
          onClose={() => setShowPicker(false)}
        />
      )}
      {showPaste && (
        <PasteDescriptionModal
          onSubmit={handlePasteSubmit}
          onClose={() => setShowPaste(false)}
        />
      )}
      {detailModal && (
        <TicketDetailModal
          ticketKey={detailModal.key}
          summary={detailModal.summary}
          description={detailModal.description}
          ticketingSystem={ticketingSystem}
          showStartButton={detailModal.showStart}
          improveTimeoutSecs={config?.agent?.improve_timeout_secs}
          onStart={handleAddToDashboard}
          onClose={() => setDetailModal(null)}
          onSaved={fetchWorkflows}
        />
      )}
      {reportKey && workflows[reportKey] && workflows[reportKey].generate_report && workflows[reportKey].has_report && (
        <ReportModal workflow={workflows[reportKey]} onClose={() => setReportKey(null)} />
      )}
      {showNoJira && (
        <NoJiraAlertModal
          onClose={() => {
            setShowNoJira(false);
            sessionStorage.setItem("noJiraAlertDismissed", "1");
          }}
        />
      )}

      {/* Version footer */}
      <footer className="py-3 text-center">
        <span className="text-xs text-gray-600">Maestro v{__APP_VERSION__}</span>
      </footer>

      {/* System error alerts (bottom-right) */}
      <SystemErrorAlert errors={systemErrors} onDismiss={dismissError} />
    </div>
  );
}

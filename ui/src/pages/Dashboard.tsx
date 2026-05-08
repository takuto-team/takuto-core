// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect, useCallback, useRef } from "react";
import { apiJson, apiPost } from "../api/client";
import type { ConfigResponse, WorkflowDefinition, WorkflowEvent } from "../api/types";
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
import { RepoPickerModal } from "../components/modals/RepoPickerModal";
import { CloneProgressModal } from "../components/modals/CloneProgressModal";
import { WorkspaceSwitcherModal } from "../components/modals/WorkspaceSwitcherModal";
import { ConfirmModal } from "../components/modals/ConfirmModal";
import { NoJiraAlertModal } from "../components/modals/NoJiraAlertModal";
import { SystemErrorAlert } from "../components/SystemErrorAlert";

interface Props {
  onLogout: () => void;
  authEnabled: boolean;
}

export function Dashboard({ onLogout, authEnabled }: Props) {
  const { showToast } = useToast();
  const [config, setConfig] = useState<ConfigResponse | null>(null);
  const { workflows, orderKeys, terminalStates, dynamicForwards, systemErrors, counts, dismissError, fetchWorkflows, fetchCounts, handleEvent, resetState } = useWorkflows();
  const [workflowDefs, setWorkflowDefs] = useState<WorkflowDefinition[]>([]);
  const defsFetchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const fetchWorkflowDefs = useCallback(() => {
    apiJson<WorkflowDefinition[]>("/api/workflow-definitions")
      .then(setWorkflowDefs)
      .catch(() => {});
  }, []);

  // Fetch definitions on mount
  useEffect(() => {
    fetchWorkflowDefs();
  }, [fetchWorkflowDefs]);

  // Wrap handleEvent to also re-fetch definitions on relevant events
  const handleEventWithDefs = useCallback(
    (evt: WorkflowEvent) => {
      handleEvent(evt);

      // Handle repo clone progress events
      if (evt.event_type === "repo_clone_progress") {
        if (evt.state === "success") {
          setCloneState((prev) =>
            prev ? { ...prev, status: "success" } : null
          );
        } else if (evt.state === "error") {
          setCloneState((prev) =>
            prev
              ? { ...prev, status: "error", error: evt.error || "Clone failed" }
              : null
          );
        }
      }

      // Re-fetch definitions when definitions change or workflows update (debounced)
      if (
        evt.event_type === "workflow_definitions_changed" ||
        evt.event_type === "workflow_updated" ||
        evt.event_type === "step_completed"
      ) {
        if (defsFetchTimer.current) clearTimeout(defsFetchTimer.current);
        defsFetchTimer.current = setTimeout(fetchWorkflowDefs, 500);
      }
    },
    [handleEvent, fetchWorkflowDefs]
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
  const [showRepoPicker, setShowRepoPicker] = useState(false);
  const [showWorkspaceSwitcher, setShowWorkspaceSwitcher] = useState(false);
  const [cloneState, setCloneState] = useState<{
    repoName: string;
    status: "cloning" | "success" | "error";
    error?: string;
  } | null>(null);
  const [overwriteConfirm, setOverwriteConfirm] = useState<string | null>(null);

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

  const handleAddToDashboard = useCallback(async (description: string, summary: string) => {
    if (!detailModal) return;
    try {
      const res = await apiPost("/api/workflows/start-manual", {
        ticket_key: detailModal.key,
        ticket_summary: summary,
        ticket_description: description,
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
  }, [detailModal, fetchWorkflows]);

  const handlePasteSubmit = useCallback(
    async (name: string, description: string) => {
      try {
        const res = await apiPost("/api/workflows/start-manual", {
          ticket_key: name,
          ticket_summary: name || "Manual workflow",
          ticket_description: description,
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
    [fetchWorkflows]
  );

  const handleShowDescription = useCallback((key: string, summary: string, description?: string) => {
    // For Jira, don't pass cached description — the modal fetches fresh from the preview API.
    // For None and GitHub, use the in-memory description (it's the source of truth or a good cache).
    const desc = ticketingSystem === "jira" ? undefined : description;
    setDetailModal({ key, summary, description: desc, showStart: false });
  }, [ticketingSystem]);

  const handleSetupProject = useCallback(() => {
    setShowWorkspaceSwitcher(true);
  }, []);

  const handleRepoSelected = useCallback(async (fullName: string) => {
    setShowRepoPicker(false);
    setCloneState({ repoName: fullName, status: "cloning" });

    try {
      const res = await apiPost("/api/repos/clone", { full_name: fullName, force: false });
      if (res.status === 409) {
        const text = await res.text();
        if (text.includes("repository_exists")) {
          setCloneState(null);
          setOverwriteConfirm(fullName);
        } else {
          setCloneState({ repoName: fullName, status: "error", error: text });
        }
      } else if (!res.ok) {
        const text = await res.text();
        setCloneState({ repoName: fullName, status: "error", error: text || `HTTP ${res.status}` });
      }
      // If 202, wait for WebSocket events to update clone status
    } catch (e) {
      setCloneState({
        repoName: fullName,
        status: "error",
        error: e instanceof Error ? e.message : "Failed to start clone",
      });
    }
  }, []);

  const handleWorkspaceSwitched = useCallback(() => {
    setShowWorkspaceSwitcher(false);
    resetState();
    apiJson<ConfigResponse>("/api/config")
      .then(setConfig)
      .catch(() => {});
    fetchWorkflows();
    fetchCounts();
  }, [fetchWorkflows, fetchCounts, resetState]);

  const handleCloneDone = useCallback(() => {
    setCloneState(null);
    // Re-fetch config to update repo_exists
    apiJson<ConfigResponse>("/api/config")
      .then(setConfig)
      .catch(() => {});
    fetchWorkflows();
  }, [fetchWorkflows]);

  const handleCloneRetry = useCallback(() => {
    if (cloneState) {
      handleRepoSelected(cloneState.repoName);
    }
  }, [cloneState, handleRepoSelected]);

  const handleOverwriteConfirm = useCallback(async () => {
    const fullName = overwriteConfirm;
    if (!fullName) return;
    setOverwriteConfirm(null);
    setCloneState({ repoName: fullName, status: "cloning" });
    try {
      const res = await apiPost("/api/repos/clone", { full_name: fullName, force: true });
      if (!res.ok && res.status !== 202) {
        const errText = await res.text();
        setCloneState({ repoName: fullName, status: "error", error: errText });
      }
    } catch (e) {
      setCloneState({
        repoName: fullName,
        status: "error",
        error: e instanceof Error ? e.message : "Failed to start clone",
      });
    }
  }, [overwriteConfirm]);

  const handleOverwriteCancel = useCallback(() => {
    setOverwriteConfirm(null);
    setShowRepoPicker(true);
  }, []);

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
        repoName={config?.repo_name}
        repoHtmlUrl={config?.repo_html_url}
        onChangeRepo={() => setShowWorkspaceSwitcher(true)}
        onLogout={onLogout}
      />

      {/* Preflight error banner */}
      {config?.preflight_error && (
        <div className="bg-red-950/80 border-b border-red-700 px-4 py-3 text-red-200">
          <div className="max-w-7xl mx-auto flex items-start gap-3">
            <span className="text-red-400 text-lg leading-none mt-0.5">⚠</span>
            <div className="flex-1 min-w-0">
              <p className="font-semibold text-red-300 text-sm">Maestro is not ready — setup required</p>
              {config.preflight_error.split("\n").map((line, i) => (
                <p key={i} className="text-xs text-red-300/80 mt-1 font-mono break-all">{line}</p>
              ))}
              <p className="text-xs text-red-300/70 mt-1">
                Run <code className="bg-red-900/50 px-1 rounded">docker compose run --rm -it maestro setup</code> to complete setup, then restart.
              </p>
            </div>
          </div>
        </div>
      )}

      {/* Dry mode banner */}
      {dryMode && (
        <div className="bg-amber-900/30 border-b border-amber-700/50 px-4 py-2 text-center text-xs text-amber-300">
          Dry mode is enabled &mdash; Jira/GitHub side effects are skipped
        </div>
      )}

      <main className="flex-1 max-w-7xl mx-auto w-full px-4 sm:px-6 lg:px-8 py-6 flex flex-col gap-6">
        <SummaryStats counts={counts} />
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
          canAddWorkflow={true}
          repoExists={config?.repo_exists ?? true}
          onSetupProject={handleSetupProject}
        />
      </main>

      {/* Modals */}
      {showPicker && (
        <TicketPickerModal
          ticketingSystem={ticketingSystem}
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
      {showRepoPicker && (
        <RepoPickerModal
          onSelect={handleRepoSelected}
          onClose={() => setShowRepoPicker(false)}
        />
      )}
      {cloneState && (
        <CloneProgressModal
          repoName={cloneState.repoName}
          status={cloneState.status}
          error={cloneState.error}
          onDone={handleCloneDone}
          onRetry={handleCloneRetry}
          onCancel={undefined}
        />
      )}
      {overwriteConfirm && (
        <ConfirmModal
          title="Overwrite Repository?"
          message="A repository already exists. Overwriting will delete the current repository and clone the selected one in its place."
          onConfirm={handleOverwriteConfirm}
          onCancel={handleOverwriteCancel}
        />
      )}
      {showWorkspaceSwitcher && (
        <WorkspaceSwitcherModal
          onClose={() => setShowWorkspaceSwitcher(false)}
          onSwitched={handleWorkspaceSwitched}
          onAddRepo={() => {
            setShowWorkspaceSwitcher(false);
            setShowRepoPicker(true);
          }}
        />
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

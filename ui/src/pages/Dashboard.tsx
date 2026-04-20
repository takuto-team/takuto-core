import { useState, useEffect, useCallback, useRef } from "react";
import { apiJson, apiPost } from "../api/client";
import type { ConfigResponse } from "../api/types";
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
import { SystemErrorAlert } from "../components/SystemErrorAlert";

interface Props {
  onLogout: () => void;
  authEnabled: boolean;
}

export function Dashboard({ onLogout, authEnabled }: Props) {
  const [config, setConfig] = useState<ConfigResponse | null>(null);
  const { workflows, orderKeys, terminalStates, dynamicForwards, systemErrors, dismissError, fetchWorkflows, handleEvent } = useWorkflows();
  const { connected } = useWebSocket(handleEvent);
  const prevConnected = useRef(false);
  const polling = usePolling();

  // Re-fetch workflows on WebSocket reconnect (connected: false → true)
  useEffect(() => {
    if (connected && !prevConnected.current) {
      fetchWorkflows();
    }
    prevConnected.current = connected;
  }, [connected, fetchWorkflows]);

  // Modal state
  const [showPicker, setShowPicker] = useState(false);
  const [showPaste, setShowPaste] = useState(false);
  const [showNoJira, setShowNoJira] = useState(false);
  const [detailModal, setDetailModal] = useState<{
    key: string;
    summary: string;
    description?: string;
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
    (key: string, summary: string, description?: string) => {
      setShowPicker(false);
      setDetailModal({ key, summary, description, showStart: true });
    },
    []
  );

  const handleStartWorkflow = useCallback(async () => {
    if (!detailModal) return;
    try {
      const res = await apiPost("/api/workflows/start-manual", {
        ticket_key: detailModal.key,
        ticket_summary: detailModal.summary,
        ticket_description: detailModal.description || "",
      });
      if (!res.ok) {
        const text = await res.text();
        throw new Error(text || `HTTP ${res.status}`);
      }
      setDetailModal(null);
      fetchWorkflows();
    } catch (e) {
      alert(e instanceof Error ? e.message : "Failed to start workflow");
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
        alert(e instanceof Error ? e.message : "Failed to start workflow");
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

  const workflowList = Object.values(workflows);

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
        onLogout={onLogout}
      />

      {/* Dry mode banner */}
      {dryMode && (
        <div className="bg-amber-900/30 border-b border-amber-700/50 px-4 py-2 text-center text-xs text-amber-300">
          Dry mode is enabled &mdash; Jira/GitHub side effects are skipped
        </div>
      )}

      <main className="flex-1 max-w-7xl mx-auto w-full px-4 sm:px-6 lg:px-8 py-6 flex flex-col gap-6">
        <SummaryStats workflows={workflowList} />
        <WorkflowGrid
          workflows={workflows}
          orderKeys={orderKeys}
          terminalStates={terminalStates}
          dynamicForwards={dynamicForwards}
          onRefresh={fetchWorkflows}
          onShowDescription={handleShowDescription}
          onReport={setReportKey}
          onAddWorkflow={handleAddWorkflow}
          canAddWorkflow={true}
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
          onStart={handleStartWorkflow}
          onClose={() => setDetailModal(null)}
          onSaved={fetchWorkflows}
        />
      )}
      {reportKey && workflows[reportKey] && (
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

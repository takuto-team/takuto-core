// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `useDashboardModals` — discriminated-union state machine for the five
 * modals the Dashboard page can show: picker, paste, no-jira, detail,
 * report. Exactly one is visible at any time.
 *
 * Replaces the previous five independent `useState<boolean>` /
 * `useState<T | null>` slots in the Dashboard shell.
 *
 * Behaviour preserved verbatim:
 *   * `openDetail(ticket)` transitions picker→detail in a single
 *     `setState` call to avoid the brief "none" flicker.
 *   * `close()` for the `nojira` variant writes
 *     `sessionStorage["noJiraAlertDismissed"] = "1"` so the alert does
 *     not pop again in the same browser session.
 *   * `nojira` auto-opens on mount once per session when
 *     `config.ticketing_system === "none"` and the session-storage flag
 *     is absent. Effect waits for `config !== null`.
 */

import { useCallback, useEffect, useState } from "react";
import type { ConfigResponse } from "../api/types";

export interface DetailTicket {
  key: string;
  summary: string;
  description?: string;
  url?: string;
  showStart: boolean;
}

export type DashboardModalState =
  | { kind: "none" }
  | { kind: "picker" }
  | { kind: "paste" }
  | { kind: "nojira" }
  | { kind: "detail"; ticket: DetailTicket }
  | { kind: "report"; reportKey: string };

export interface UseDashboardModalsResult {
  modal: DashboardModalState;
  openPicker: () => void;
  openPaste: () => void;
  openNoJira: () => void;
  openDetail: (ticket: DetailTicket) => void;
  openReport: (reportKey: string) => void;
  close: () => void;
}

const NO_JIRA_DISMISSED_KEY = "noJiraAlertDismissed";

export function useDashboardModals(config: ConfigResponse | null): UseDashboardModalsResult {
  const [modal, setModal] = useState<DashboardModalState>({ kind: "none" });

  const openPicker = useCallback(() => setModal({ kind: "picker" }), []);
  const openPaste = useCallback(() => setModal({ kind: "paste" }), []);
  const openNoJira = useCallback(() => setModal({ kind: "nojira" }), []);
  const openDetail = useCallback((ticket: DetailTicket) => setModal({ kind: "detail", ticket }), []);
  const openReport = useCallback((reportKey: string) => setModal({ kind: "report", reportKey }), []);

  const close = useCallback(() => {
    setModal((prev) => {
      if (prev.kind === "nojira") {
        sessionStorage.setItem(NO_JIRA_DISMISSED_KEY, "1");
      }
      return { kind: "none" };
    });
  }, []);

  // Show no-jira alert once per session when ticketing is unconfigured.
  useEffect(() => {
    if (config === null) return;
    if (config.ticketing_system !== "none") return;
    if (sessionStorage.getItem(NO_JIRA_DISMISSED_KEY) === "1") return;
    setModal((prev) => (prev.kind === "none" ? { kind: "nojira" } : prev));
  }, [config]);

  return { modal, openPicker, openPaste, openNoJira, openDetail, openReport, close };
}

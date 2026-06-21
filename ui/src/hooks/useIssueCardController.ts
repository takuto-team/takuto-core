// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Orchestration logic for one IssueCard: owns the card's local UI state
 * (loading overlay, confirm/console/delete modals, open action menu) and
 * wires `useIssueCardActions` to the loading/toast flow. The card component
 * consumes this + the pure view-model and renders — no logic in the `.tsx`
 * body (CODING_STANDARDS §3).
 */

import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { useIssueCardActions } from "./useIssueCardActions";
import { useToast } from "./useToast";

type MenuKind = "port" | "editor" | "terminal";
type Loading = false | "generic" | string;
interface ConfirmState {
  action: string;
  label: string;
  fn: () => Promise<void>;
}

export interface IssueCardController {
  loading: Loading;
  withLoading: (fn: () => Promise<void>, message?: string) => Promise<void>;
  openMenu: MenuKind | null;
  setOpenMenu: (menu: MenuKind | null) => void;
  confirm: ConfirmState | null;
  consoleOpen: boolean;
  deleteOpen: boolean;
  onShowConsole: () => void;
  onRequestDelete: () => void;
  onRetry: () => void;
  onResumeFromError: () => void;
  onPause: () => void;
  onResume: () => void;
  onStop: () => void;
  onOpenEditor: () => void;
  onOpenTerminal: () => void;
  onCloseEditor: () => void;
  onCloseTerminal: () => void;
  onConfirm: () => void;
  onConfirmCancel: () => void;
  onConsoleClose: () => void;
  onMarkDoneAndDelete: () => void;
  onDelete: () => void;
  onDeleteCancel: () => void;
}

/**
 * @param preparingWorkspace when true, the first editor/terminal open may have
 *   to recreate the worktree on the backend (terminal workflow with no live
 *   container), so the overlay shows a "Preparing workspace…" message and stays
 *   up until the (slower) request resolves.
 */
export function useIssueCardController(
  ticketKey: string,
  onRefresh: () => void,
  preparingWorkspace: boolean,
): IssueCardController {
  const { t } = useTranslation("dashboard");
  const { showToast } = useToast();
  const { doAction, openEditor, openTerminal, closeEditor, closeTerminal } =
    useIssueCardActions(ticketKey);

  const PREPARING_MESSAGE = t("loading.preparingWorkspace");

  const [loading, setLoading] = useState<Loading>(false);
  const [confirm, setConfirm] = useState<ConfirmState | null>(null);
  const [consoleOpen, setConsoleOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [openMenu, setOpenMenu] = useState<MenuKind | null>(null);

  const withLoading = useCallback(
    async (fn: () => Promise<void>, message?: string) => {
      setLoading(message || "generic");
      try {
        await fn();
        onRefresh();
      } catch (e) {
        showToast(e instanceof Error ? e.message : t("toast.actionFailed"));
      } finally {
        setLoading(false);
      }
    },
    [onRefresh, showToast, t],
  );

  const onStop = useCallback(() => {
    setConfirm({ action: "stop", label: t("actions.stop"), fn: doAction("stop") });
  }, [doAction, t]);

  const onConfirm = useCallback(() => {
    const fn = confirm?.fn;
    setConfirm(null);
    if (fn) void withLoading(fn);
  }, [confirm, withLoading]);

  const onMarkDoneAndDelete = useCallback(() => {
    setDeleteOpen(false);
    void withLoading(async () => {
      await doAction("mark-done")();
      await doAction("delete")();
    });
  }, [doAction, withLoading]);

  return {
    loading,
    withLoading,
    openMenu,
    setOpenMenu,
    confirm,
    consoleOpen,
    deleteOpen,
    onShowConsole: useCallback(() => setConsoleOpen(true), []),
    onRequestDelete: useCallback(() => setDeleteOpen(true), []),
    onRetry: useCallback(() => void withLoading(doAction("retry")), [withLoading, doAction]),
    onResumeFromError: useCallback(() => void withLoading(doAction("resume-from-error")), [withLoading, doAction]),
    onPause: useCallback(() => void withLoading(doAction("pause")), [withLoading, doAction]),
    onResume: useCallback(() => void withLoading(doAction("resume")), [withLoading, doAction]),
    onStop,
    onOpenEditor: useCallback(
      () =>
        void withLoading(
          openEditor,
          preparingWorkspace ? PREPARING_MESSAGE : t("loading.connectingEditor"),
        ),
      [withLoading, openEditor, preparingWorkspace, PREPARING_MESSAGE, t],
    ),
    onOpenTerminal: useCallback(
      () =>
        void withLoading(
          openTerminal,
          preparingWorkspace ? PREPARING_MESSAGE : t("loading.connectingTerminal"),
        ),
      [withLoading, openTerminal, preparingWorkspace, PREPARING_MESSAGE, t],
    ),
    onCloseEditor: useCallback(() => void withLoading(closeEditor), [withLoading, closeEditor]),
    onCloseTerminal: useCallback(() => void withLoading(closeTerminal), [withLoading, closeTerminal]),
    onConfirm,
    onConfirmCancel: useCallback(() => setConfirm(null), []),
    onConsoleClose: useCallback(() => setConsoleOpen(false), []),
    onMarkDoneAndDelete,
    onDelete: useCallback(() => {
      setDeleteOpen(false);
      void withLoading(doAction("delete"));
    }, [withLoading, doAction]),
    onDeleteCancel: useCallback(() => setDeleteOpen(false), []),
  };
}

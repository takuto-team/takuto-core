// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/** The IssueCard's three overlays: stop/action confirm, console output, delete confirm. */

import { useTranslation } from "react-i18next";
import type { TerminalState } from "../../hooks/useWorkflows";
import { ConfirmModal } from "../modals/ConfirmModal";
import { ConsoleOutputModal } from "../modals/ConsoleOutputModal";
import { DeleteConfirmModal } from "../modals/DeleteConfirmModal";

interface Props {
  ticketKey: string;
  showMarkDone: boolean;
  confirm: { action: string; label: string } | null;
  consoleState: TerminalState | undefined;
  consoleOpen: boolean;
  deleteOpen: boolean;
  markDoneError: string | null;
  onConfirm: () => void;
  onConfirmCancel: () => void;
  onConsoleClose: () => void;
  onMarkDoneAndDelete: () => void;
  onMarkDoneErrorClose: () => void;
  onOpenTicketingSettings: () => void;
  onDelete: () => void;
  onDeleteCancel: () => void;
}

export function IssueCardModals({
  ticketKey,
  showMarkDone,
  confirm,
  consoleState,
  consoleOpen,
  deleteOpen,
  markDoneError,
  onConfirm,
  onConfirmCancel,
  onConsoleClose,
  onMarkDoneAndDelete,
  onMarkDoneErrorClose,
  onOpenTicketingSettings,
  onDelete,
  onDeleteCancel,
}: Props) {
  const { t } = useTranslation("dashboard");
  return (
    <>
      {confirm && (
        <ConfirmModal
          title={confirm.label}
          message={t("confirm.message", {
            action: t(`confirm.verb.${confirm.action}`),
            ticketKey,
          })}
          onConfirm={onConfirm}
          onCancel={onConfirmCancel}
        />
      )}

      {consoleOpen && consoleState && (
        <ConsoleOutputModal state={consoleState} onClose={onConsoleClose} />
      )}

      {markDoneError && (
        <ConfirmModal
          title={t("markDoneFailed.title")}
          message={markDoneError}
          confirmLabel={t("markDoneFailed.openSettings")}
          onConfirm={onOpenTicketingSettings}
          onCancel={onMarkDoneErrorClose}
        />
      )}

      {deleteOpen && (
        <DeleteConfirmModal
          ticketKey={ticketKey}
          showMarkDone={showMarkDone}
          onMarkDoneAndDelete={onMarkDoneAndDelete}
          onDelete={onDelete}
          onCancel={onDeleteCancel}
        />
      )}
    </>
  );
}

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/** The IssueCard's three overlays: stop/action confirm, console output, delete confirm. */

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
  onConfirm: () => void;
  onConfirmCancel: () => void;
  onConsoleClose: () => void;
  onMarkDoneAndDelete: () => void;
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
  onConfirm,
  onConfirmCancel,
  onConsoleClose,
  onMarkDoneAndDelete,
  onDelete,
  onDeleteCancel,
}: Props) {
  return (
    <>
      {confirm && (
        <ConfirmModal
          title={confirm.label}
          message={`Are you sure you want to ${confirm.action} work item ${ticketKey}?`}
          onConfirm={onConfirm}
          onCancel={onConfirmCancel}
        />
      )}

      {consoleOpen && consoleState && (
        <ConsoleOutputModal state={consoleState} onClose={onConsoleClose} />
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

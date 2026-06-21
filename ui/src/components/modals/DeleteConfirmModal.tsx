// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useTranslation } from "react-i18next";

interface Props {
  ticketKey: string;
  showMarkDone: boolean;
  onMarkDoneAndDelete: () => void;
  onDelete: () => void;
  onCancel: () => void;
}

export function DeleteConfirmModal({ ticketKey, showMarkDone, onMarkDoneAndDelete, onDelete, onCancel }: Props) {
  const { t } = useTranslation("modals");
  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl p-6 max-w-md w-full mx-4"
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="text-lg font-medium text-white mb-2">{t("delete.title", { ticketKey })}</h3>
        <p className="text-sm text-gray-400 mb-6">{t("delete.cannotUndo")}</p>
        <div className="flex justify-end gap-3">
          <button
            onClick={onCancel}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer transition-colors"
          >
            {t("common.cancel")}
          </button>
          {showMarkDone && (
            <button
              onClick={onMarkDoneAndDelete}
              className="text-sm px-4 py-2 rounded-lg bg-emerald-600 text-white hover:bg-emerald-500 cursor-pointer transition-colors"
            >
              {t("delete.markDoneAndDelete")}
            </button>
          )}
          <button
            onClick={onDelete}
            className="text-sm px-4 py-2 rounded-lg bg-red-600 text-white hover:bg-red-500 cursor-pointer transition-colors"
          >
            {t("delete.delete")}
          </button>
        </div>
      </div>
    </div>
  );
}

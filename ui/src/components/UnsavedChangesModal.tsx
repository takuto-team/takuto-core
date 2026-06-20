// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Confirmation modal shown when the user tries to leave a settings surface with
 * unsaved changes. Styled to match `admin/ProviderSwitchConfirm.tsx`.
 */
import { useState } from "react";

interface Props {
  /** Persist pending changes. Returns `true` on success. */
  onSave: () => Promise<boolean>;
  /** Leave the page (used both for "discard & leave" and after a successful save). */
  onProceed: () => void;
  /** Stay on the page. */
  onCancel: () => void;
}

export function UnsavedChangesModal({ onSave, onProceed, onCancel }: Props) {
  const [saving, setSaving] = useState(false);

  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        className="bg-gray-900 border border-amber-700/50 rounded-xl p-6 max-w-md w-full mx-4"
        onClick={(e) => e.stopPropagation()}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="unsaved-changes-title"
        aria-describedby="unsaved-changes-body"
      >
        <h3 id="unsaved-changes-title" className="text-lg font-medium text-amber-300 mb-2">
          Unsaved changes
        </h3>
        <p id="unsaved-changes-body" className="text-sm text-gray-300 mb-4">
          You have unsaved changes on this tab. Save them before leaving, or
          discard them?
        </p>
        <div className="flex justify-end gap-3">
          <button
            type="button"
            onClick={onCancel}
            disabled={saving}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 disabled:opacity-50 cursor-pointer"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onProceed}
            disabled={saving}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-red-300 border border-red-700/60 hover:bg-red-950/40 disabled:opacity-50 cursor-pointer"
          >
            Discard changes
          </button>
          <button
            type="button"
            onClick={async () => {
              setSaving(true);
              try {
                if (await onSave()) onProceed();
              } finally {
                setSaving(false);
              }
            }}
            disabled={saving}
            className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
          >
            {saving ? "Saving…" : "Save & leave"}
          </button>
        </div>
      </div>
    </div>
  );
}

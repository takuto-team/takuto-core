// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * The single Save footer for the settings (Config) form tabs. Rendered as the
 * last flex child of the Config page (after the scrollable <main>), so it stays
 * pinned to the viewport bottom regardless of tab content height. One Save
 * button per page: it commits every dirty section in the active form tab.
 */

import { useTranslation } from "react-i18next";

interface Props {
  /** Any section in the active tab has unsaved changes. */
  dirty: boolean;
  /** A save is in flight. */
  saving: boolean;
  /** Persist every dirty section in the active tab. */
  onSave: () => void;
}

export function SettingsFooter({ dirty, saving, onSave }: Props) {
  const { t } = useTranslation("config");
  return (
    <footer className="border-t border-gray-800 bg-gray-950/80 backdrop-blur-sm">
      <div className="w-full px-4 sm:px-6 lg:px-8 py-3 flex items-center justify-end gap-3">
        {dirty && <span className="text-xs text-amber-300">{t("actions.unsavedChanges")}</span>}
        <button
          type="button"
          disabled={!dirty || saving}
          onClick={onSave}
          className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
        >
          {saving ? t("actions.saving") : t("actions.saveChanges")}
        </button>
      </div>
    </footer>
  );
}

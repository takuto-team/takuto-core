// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Presentational modal shown when a user's Jira credential is rejected
 * (expired / revoked token). The primary CTA routes to Config → Ticketing to
 * update the token; the secondary dismisses. Stateless — the host
 * (`JiraAuthFailedModalHost`) owns open/close and navigation.
 */

import { useTranslation } from "react-i18next";

interface Props {
  onUpdateToken: () => void;
  onDismiss: () => void;
}

export function JiraAuthFailedModal({ onUpdateToken, onDismiss }: Props) {
  const { t } = useTranslation("modals");
  return (
    <div className="modal-backdrop" onClick={onDismiss}>
      <div
        className="bg-gray-900 border border-amber-700/50 rounded-xl p-6 max-w-md w-full mx-4"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="jira-auth-failed-title"
        onClick={(e) => e.stopPropagation()}
      >
        <h3 id="jira-auth-failed-title" className="text-lg font-medium text-amber-400 mb-2">
          {t("jiraAuthFailed.title")}
        </h3>
        <p className="text-sm text-gray-400 mb-5">{t("jiraAuthFailed.body")}</p>
        <div className="flex justify-end gap-3">
          <button
            type="button"
            onClick={onDismiss}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
          >
            {t("jiraAuthFailed.dismiss")}
          </button>
          <button
            type="button"
            onClick={onUpdateToken}
            className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 cursor-pointer"
          >
            {t("jiraAuthFailed.cta")}
          </button>
        </div>
      </div>
    </div>
  );
}

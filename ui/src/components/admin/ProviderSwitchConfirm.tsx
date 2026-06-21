// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Provider-switch confirmation modal (05_ux_design.md §2.6). Forces the
 * admin to type `SWITCH` before persisting a provider change because
 * switching marks every per-user credential for the previous provider as
 * `inactive=1`.
 *
 * Extracted from `AiProviderSettingsSection.tsx` so the section shell stays
 * focused on the save flow (CODING_STANDARDS §3 one component per file).
 */

import { useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import type { AgentProviderId } from "../../api/types";
import { PROVIDER_LABEL } from "./ProviderForm";

interface SwitchProps {
  from: AgentProviderId;
  to: AgentProviderId;
  onCancel: () => void;
  onConfirm: () => void;
}

export function ProviderSwitchConfirm({ from, to, onCancel, onConfirm }: SwitchProps) {
  const { t } = useTranslation("config");
  const [typed, setTyped] = useState("");
  const canConfirm = typed.trim().toUpperCase() === "SWITCH";
  const fromLabel = PROVIDER_LABEL[from] ?? from;
  const toLabel = PROVIDER_LABEL[to] ?? to;
  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        className="bg-gray-900 border border-amber-700/50 rounded-xl p-6 max-w-md w-full mx-4"
        onClick={(e) => e.stopPropagation()}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="provider-switch-title"
        aria-describedby="provider-switch-body"
      >
        <h3
          id="provider-switch-title"
          className="text-lg font-medium text-amber-300 mb-2"
        >
          {t("ai.switch.title")}
        </h3>
        <div id="provider-switch-body" className="text-sm text-gray-300 mb-4">
          <p>
            <Trans
              i18nKey="ai.switch.intro"
              ns="config"
              values={{ from: fromLabel, to: toLabel }}
              components={{ strong: <strong /> }}
            />
          </p>
          <p className="mt-2 text-gray-400">
            {t("ai.switch.body", { from: fromLabel, to: toLabel })}
          </p>
          <p className="mt-2 text-xs text-gray-500">
            {t("ai.switch.migrateNote")}
          </p>
        </div>
        <label
          htmlFor="provider-switch-confirm"
          className="block text-xs text-gray-400 mb-1"
        >
          <Trans
            i18nKey="ai.switch.typeToConfirm"
            ns="config"
            components={{ code: <code className="text-amber-300" /> }}
          />
        </label>
        <input
          id="provider-switch-confirm"
          type="text"
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          autoFocus
          className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono mb-4"
        />
        <div className="flex justify-end gap-3">
          <button
            onClick={onCancel}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
          >
            {t("actions.cancel")}
          </button>
          <button
            onClick={onConfirm}
            disabled={!canConfirm}
            className="text-sm px-4 py-2 rounded-lg bg-amber-600 text-white hover:bg-amber-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
          >
            {t("ai.switch.confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}

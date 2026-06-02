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
import type { AgentProviderId } from "../../api/types";
import { PROVIDER_LABEL } from "./ProviderForm";

interface SwitchProps {
  from: AgentProviderId;
  to: AgentProviderId;
  onCancel: () => void;
  onConfirm: () => void;
}

export function ProviderSwitchConfirm({ from, to, onCancel, onConfirm }: SwitchProps) {
  const [typed, setTyped] = useState("");
  const canConfirm = typed.trim().toUpperCase() === "SWITCH";
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
          Switch AI provider?
        </h3>
        <div id="provider-switch-body" className="text-sm text-gray-300 mb-4">
          <p>
            You&rsquo;re switching from{" "}
            <strong>{PROVIDER_LABEL[from] ?? from}</strong> to{" "}
            <strong>{PROVIDER_LABEL[to] ?? to}</strong>.
          </p>
          <p className="mt-2 text-gray-400">
            Per-user credentials for {PROVIDER_LABEL[from] ?? from} will be
            deactivated. Each user must connect their{" "}
            {PROVIDER_LABEL[to] ?? to} account before they can run new
            workflows. Workflows already running will finish on{" "}
            {PROVIDER_LABEL[from] ?? from}.
          </p>
          <p className="mt-2 text-xs text-gray-500">
            Per-user credentials will be migrated in a later phase.
          </p>
        </div>
        <label
          htmlFor="provider-switch-confirm"
          className="block text-xs text-gray-400 mb-1"
        >
          Type <code className="text-amber-300">SWITCH</code> to confirm
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
            Cancel
          </button>
          <button
            onClick={onConfirm}
            disabled={!canConfirm}
            className="text-sm px-4 py-2 rounded-lg bg-amber-600 text-white hover:bg-amber-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
          >
            Switch provider
          </button>
        </div>
      </div>
    </div>
  );
}

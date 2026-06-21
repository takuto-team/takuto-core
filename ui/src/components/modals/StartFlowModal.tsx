// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Overflow fallback for the work-item card's flow buttons. Lists every flow as
 * a full-width row carrying the same five visual states as the inline buttons
 * (enabled / disabled-with-lock / running / completed / error). Running a flow
 * here calls the same endpoint as the inline button via `useRunWorkflowDef`.
 */

import { useTranslation } from "react-i18next";
import type { WorkflowDefinition } from "../../api/types";
import { useRunWorkflowDef } from "../../hooks/useRunWorkflowDef";
import { topoSort, runStateOf, depsAreMet, unmetDeps } from "../workflowDefState";
import { SpinnerIcon, CheckIcon, XIcon, LockIcon } from "../icons";

interface StartFlowModalProps {
  definitions: WorkflowDefinition[];
  runStates: Record<string, string>;
  ticketKey: string;
  onRefresh: () => void;
  onClose: () => void;
  mainRunning?: boolean;
}

export function StartFlowModal({
  definitions,
  runStates,
  ticketKey,
  onRefresh,
  onClose,
  mainRunning,
}: StartFlowModalProps) {
  const { t } = useTranslation("modals");
  const { run, loadingDef } = useRunWorkflowDef(ticketKey, onRefresh);

  const validDefs = definitions.filter((d) => d.valid);
  const sorted = topoSort(validDefs);

  const firstEnabledFile = sorted.find(
    (d) => runStateOf(d, runStates) === "idle" && depsAreMet(d, runStates) && !mainRunning,
  )?.filename;

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") onClose();
  };

  const depChips = (def: WorkflowDefinition) =>
    def.depends_on.length > 0 ? (
      <span className="flex items-center gap-1 text-xs text-gray-500 min-w-0">
        <span className="whitespace-nowrap">{t("startFlow.dependsOn")}</span>
        {def.depends_on.map((dep) => (
          <span
            key={dep}
            title={dep}
            className="bg-gray-800 text-gray-400 px-1.5 py-0.5 rounded truncate max-w-[8rem]"
          >
            {dep}
          </span>
        ))}
      </span>
    ) : null;

  const renderAction = (def: WorkflowDefinition) => {
    const state = runStateOf(def, runStates);
    const isLoading = loadingDef === def.filename;
    const met = depsAreMet(def, runStates);

    if (state === "running") {
      return (
        <span className="action-btn wf-btn-primary opacity-75 cursor-default inline-flex items-center gap-1">
          <SpinnerIcon /> {t("startFlow.running")}
        </span>
      );
    }
    if (state === "completed") {
      return (
        <span className="action-btn wf-btn-success cursor-default inline-flex items-center gap-1">
          <CheckIcon /> {t("startFlow.done")}
        </span>
      );
    }
    if (state === "error") {
      if (mainRunning) {
        return (
          <span className="action-btn wf-btn-danger opacity-50 cursor-not-allowed inline-flex items-center gap-1">
            <XIcon /> {t("startFlow.failed")}
          </span>
        );
      }
      return (
        <button
          className="action-btn wf-btn-danger inline-flex items-center gap-1"
          onClick={() => run(def, state)}
          disabled={isLoading}
          title={t("startFlow.clickToRetry")}
        >
          {isLoading ? <SpinnerIcon /> : <XIcon />} {t("startFlow.retry")}
        </button>
      );
    }
    if (!met || mainRunning) {
      return (
        <span className="action-btn wf-btn-secondary opacity-50 cursor-not-allowed inline-flex items-center gap-1">
          {!met && <LockIcon />} --
        </span>
      );
    }
    return (
      <button
        className="action-btn wf-btn-primary inline-flex items-center gap-1"
        onClick={() => run(def, state)}
        disabled={isLoading}
        autoFocus={def.filename === firstEnabledFile}
      >
        {isLoading ? <SpinnerIcon /> : null} {t("startFlow.start")}
      </button>
    );
  };

  return (
    <div className="modal-backdrop" onClick={onClose} onKeyDown={handleKeyDown}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl max-w-lg w-full mx-4 max-h-[80vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between p-4 border-b border-gray-800">
          <h3 className="text-lg font-medium text-white">{t("startFlow.title", { ticketKey })}</h3>
          <button
            type="button"
            onClick={onClose}
            className="text-gray-500 hover:text-gray-300 cursor-pointer"
            aria-label={t("startFlow.close")}
          >
            &times;
          </button>
        </div>

        <div className="overflow-y-auto flex-1 p-4 space-y-2">
          {sorted.length === 0 ? (
            <p className="text-sm text-gray-500">{t("startFlow.noFlows")}</p>
          ) : (
            sorted.map((def) => {
              const state = runStateOf(def, runStates);
              const met = depsAreMet(def, runStates);
              const locked = state === "idle" && !met;
              const waiting = locked ? unmetDeps(def, definitions, runStates) : [];
              return (
                <div
                  key={def.filename}
                  className="flex items-center gap-3 border border-gray-800 rounded-lg bg-gray-950 px-3 py-2.5"
                >
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2 min-w-0">
                      <span
                        className={`text-sm font-medium truncate ${locked ? "text-gray-500" : "text-gray-200"}`}
                      >
                        {def.name}
                      </span>
                      {depChips(def)}
                    </div>
                    {locked && (
                      <div
                        className="text-xs text-gray-500 mt-0.5"
                        title={t("startFlow.waitingFor", { deps: waiting.join(", ") })}
                      >
                        {t("startFlow.waitingFor", { deps: waiting.join(", ") })}
                      </div>
                    )}
                  </div>
                  <div className="flex-shrink-0">{renderAction(def)}</div>
                </div>
              );
            })
          )}
        </div>

        <div className="p-4 border-t border-gray-800 flex justify-end">
          <button
            type="button"
            onClick={onClose}
            autoFocus={!firstEnabledFile}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
          >
            {t("startFlow.close")}
          </button>
        </div>
      </div>
    </div>
  );
}

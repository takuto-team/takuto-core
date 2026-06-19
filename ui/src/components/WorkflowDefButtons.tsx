// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import type { WorkflowDefinition } from "../api/types";
import { useRunWorkflowDef } from "../hooks/useRunWorkflowDef";
import { topoSort, runStateOf, depsAreMet, unmetDeps } from "./workflowDefState";
import { StartFlowModal } from "./modals/StartFlowModal";
import { SpinnerIcon, CheckIcon, XIcon, LockIcon } from "./icons";

interface WorkflowDefButtonsProps {
  definitions: WorkflowDefinition[];
  runStates: Record<string, string>;
  ticketKey: string;
  onRefresh: () => void;
  /** When true, all buttons are disabled (main pipeline is actively running). */
  mainRunning?: boolean;
  /** When true, all buttons are disabled for another reason (e.g. the item's
   *  worktree is still being prepared). Combined with `mainRunning`. */
  disabled?: boolean;
}

/**
 * Collapse the inline button row to a single "Start flow" button when the
 * buttons would not fit the available width. An always-rendered, off-screen
 * measurer holds the full inline set so the measurement is stable regardless
 * of whether we are currently collapsed (no flip-flop). A one-frame flicker on
 * first paint is acceptable.
 */
function useOverflowCollapse(signature: string) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const measurerRef = useRef<HTMLDivElement>(null);
  const [collapsed, setCollapsed] = useState(false);

  useEffect(() => {
    const wrap = wrapRef.current;
    const measurer = measurerRef.current;
    if (!wrap || !measurer || typeof ResizeObserver === "undefined") return;
    const measure = () => setCollapsed(measurer.scrollWidth > wrap.clientWidth);
    const ro = new ResizeObserver(measure);
    ro.observe(wrap);
    return () => ro.disconnect();
  }, [signature]);

  return { wrapRef, measurerRef, collapsed };
}

export function WorkflowDefButtons({
  definitions,
  runStates,
  ticketKey,
  onRefresh,
  mainRunning,
  disabled,
}: WorkflowDefButtonsProps) {
  const { run, loadingDef } = useRunWorkflowDef(ticketKey, onRefresh);
  const [modalOpen, setModalOpen] = useState(false);
  // Any reason that blocks starting a flow disables every button.
  const blocked = mainRunning || disabled;

  const validDefs = definitions.filter((d) => d.valid);
  const sorted = topoSort(validDefs);
  const signature = sorted.map((d) => d.name).join("|");
  const { wrapRef, measurerRef, collapsed } = useOverflowCollapse(signature);

  function renderButton(def: WorkflowDefinition) {
    const state = runStateOf(def, runStates);
    const met = depsAreMet(def, runStates);
    const isLoading = loadingDef === def.filename;

    if (state === "running") {
      return (
        <span
          key={def.filename}
          className="action-btn wf-btn-primary opacity-75 cursor-default inline-flex items-center justify-between gap-2"
        >
          {def.name} <SpinnerIcon />
        </span>
      );
    }

    if (state === "completed") {
      return (
        <span
          key={def.filename}
          className="action-btn wf-btn-success cursor-default inline-flex items-center gap-1"
        >
          <CheckIcon /> {def.name}
        </span>
      );
    }

    if (state === "error") {
      if (blocked) {
        return (
          <span
            key={def.filename}
            className="action-btn wf-btn-danger opacity-50 cursor-not-allowed inline-flex items-center gap-1"
          >
            <XIcon /> {def.name}
          </span>
        );
      }
      return (
        <button
          key={def.filename}
          className="action-btn wf-btn-danger inline-flex items-center gap-1"
          onClick={() => run(def, state)}
          disabled={isLoading}
          title="Click to retry"
        >
          {isLoading ? <SpinnerIcon /> : <XIcon />} {def.name}
        </button>
      );
    }

    // idle state
    if (!met || blocked) {
      const waiting = !met ? unmetDeps(def, definitions, runStates) : [];
      return (
        <span
          key={def.filename}
          className="action-btn wf-btn-secondary opacity-50 cursor-not-allowed inline-flex items-center gap-1"
          title={!met ? `Waiting for: ${waiting.join(", ")}` : undefined}
        >
          {!met && <LockIcon />} {def.name}
        </span>
      );
    }

    return (
      <button
        key={def.filename}
        className="action-btn wf-btn-primary inline-flex items-center gap-1"
        onClick={() => run(def, state)}
        disabled={isLoading}
      >
        {isLoading ? <SpinnerIcon /> : null} {def.name}
      </button>
    );
  }

  const section = (body: React.ReactNode) => (
    <>
      <div className="border-t border-gray-800/60" />
      <div>
        <div className="text-xs text-gray-500 mb-1.5">Workflows</div>
        {body}
      </div>
    </>
  );

  // Empty resolved flow list: the user cleared every flow for this workspace.
  if (definitions.length === 0) {
    return section(
      <p className="text-sm text-gray-500">
        No flows configured.{" "}
        <Link to="/config?tab=Flows" className="text-blue-400 hover:text-blue-300">
          Configure flows &rarr;
        </Link>
      </p>,
    );
  }

  const canCollapse = collapsed && sorted.length >= 2;

  return (
    <>
      {section(
        <div ref={wrapRef} className="relative">
          <div
            ref={measurerRef}
            aria-hidden="true"
            inert
            className="absolute left-0 top-0 invisible pointer-events-none flex gap-2 w-max"
          >
            {sorted.map(renderButton)}
          </div>

          {canCollapse ? (
            <button
              type="button"
              className="action-btn wf-btn-primary inline-flex items-center gap-1 disabled:opacity-50 disabled:cursor-not-allowed"
              onClick={() => setModalOpen(true)}
              disabled={blocked}
            >
              Start flow
            </button>
          ) : (
            <div className="flex flex-wrap gap-2">{sorted.map(renderButton)}</div>
          )}
        </div>,
      )}

      {modalOpen && (
        <StartFlowModal
          definitions={definitions}
          runStates={runStates}
          ticketKey={ticketKey}
          onRefresh={onRefresh}
          mainRunning={blocked}
          onClose={() => setModalOpen(false)}
        />
      )}
    </>
  );
}

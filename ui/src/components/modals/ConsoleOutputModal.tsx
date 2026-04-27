// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useRef } from "react";
import type { TerminalState } from "../../hooks/useWorkflows";

interface Props {
  state: TerminalState;
  onClose: () => void;
}

export function ConsoleOutputModal({ state, onClose }: Props) {
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (bodyRef.current) {
      bodyRef.current.scrollTop = bodyRef.current.scrollHeight;
    }
  }, [state.lines.length]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const stepDisplay = state.completed
    ? `${state.stepName} — completed`
    : `$ ${state.stepName || "Waiting..."}`;

  return (
    <div
      className="fixed inset-0 bg-black/60 backdrop-blur-sm z-50 flex items-stretch justify-center"
      onClick={onClose}
    >
      <div
        className="flex flex-col w-full max-w-2xl bg-gray-900 border-x border-gray-700"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-gray-700 flex-shrink-0">
          <span className="text-sm font-medium text-gray-200">Console output</span>
          <span
            className={`text-xs font-mono px-2 py-0.5 rounded ${
              state.completed
                ? "bg-emerald-900/30 text-emerald-400"
                : "bg-gray-800 text-gray-400"
            }`}
          >
            {stepDisplay}
          </span>
        </div>

        {/* Body */}
        <div
          ref={bodyRef}
          className="flex-1 overflow-y-auto bg-gray-950 px-4 py-3 font-mono text-xs leading-relaxed whitespace-pre-wrap break-all"
        >
          {state.lines.length === 0 ? (
            <span className="text-gray-600">No output yet.</span>
          ) : (
            state.lines.map((line, i) => {
              const isWarn = /\bwarn(ing)?\b/i.test(line.text);
              const cls = isWarn
                ? "text-yellow-400/80"
                : line.stream === "stderr"
                ? "text-red-400/70"
                : "text-gray-400";
              return (
                <div key={i} className={cls}>
                  {line.text}
                </div>
              );
            })
          )}
        </div>

        {/* Footer */}
        <div className="flex justify-end px-4 py-3 border-t border-gray-700 flex-shrink-0">
          <button
            onClick={onClose}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer transition-colors"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

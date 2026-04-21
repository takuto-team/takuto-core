// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useRef } from "react";
import type { TerminalState } from "../hooks/useWorkflows";

interface Props {
  state: TerminalState | undefined;
}

export function TerminalOutput({ state }: Props) {
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (bodyRef.current) {
      bodyRef.current.scrollTop = bodyRef.current.scrollHeight;
    }
  }, [state?.lines.length]);

  if (!state) return null;

  const stepDisplay = state.completed
    ? `${state.stepName} -- completed`
    : `$ ${state.stepName || "Waiting..."}`;

  return (
    <div className="mt-auto">
      <div
        className={`text-xs font-mono px-3 py-1.5 rounded-t-lg border border-b-0 border-gray-700 ${
          state.completed
            ? "bg-emerald-900/20 text-emerald-400"
            : "bg-gray-800/60 text-gray-400"
        }`}
      >
        {stepDisplay}
      </div>
      <div
        ref={bodyRef}
        className="terminal-output bg-gray-950 border border-gray-700 rounded-b-lg px-3 py-2"
      >
        {state.lines.map((line, i) => {
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
        })}
      </div>
    </div>
  );
}

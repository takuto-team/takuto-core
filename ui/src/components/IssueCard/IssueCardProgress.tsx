// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/** Presentational progress frame of an IssueCard: step label, state line, progress bar, action buttons. */

import { ProgressBar } from "../ProgressBar";
import { PauseIconButton } from "../PauseIconButton";
import { RestartIconButton } from "../RestartIconButton";
import { ResumeIconButton } from "../ResumeIconButton";
import { StopIconButton } from "../StopIconButton";
import { ClockIcon } from "../icons";
import type { StatusInfo } from "../StatusBadge";

interface Props {
  status: StatusInfo;
  stepLabel: string;
  stateDisplay: string;
  pct: number;
  total: number;
  filled: number;
  duration: string | null;
  isActive: boolean;
  hasReport: boolean;
  canResumeFromError: boolean;
  onRetry: () => void;
  onResumeFromError: () => void;
  onPause: () => void;
  onResume: () => void;
  onStop: () => void;
  onReport: () => void;
}

export function IssueCardProgress({
  status,
  stepLabel,
  stateDisplay,
  pct,
  total,
  filled,
  duration,
  isActive,
  hasReport,
  canResumeFromError,
  onRetry,
  onResumeFromError,
  onPause,
  onResume,
  onStop,
  onReport,
}: Props) {
  const isTerminalish =
    status.label === "Error" || status.label === "Completed" || status.label === "Stopped";
  return (
    <div className="bg-gray-800/50 rounded-lg px-3 pt-2.5 pb-2.5 relative h-[80px] flex flex-col justify-center">
      <div className="flex items-center justify-between">
        <div className="text-xs text-gray-500">{stepLabel}</div>
        <div className="flex items-center gap-2">
          <span
            className={`flex items-center leading-none gap-1 text-xs text-gray-400 ${
              !duration ? "invisible" : ""
            }`}
          >
            <ClockIcon />
            <span className="font-mono">{duration ?? "0s"}</span>
          </span>
          {hasReport && (
            <button
              onClick={onReport}
              className="text-xs text-gray-500 hover:text-gray-300 cursor-pointer transition-colors"
              title="View work item report"
            >
              Show Report
            </button>
          )}
          {isTerminalish && <RestartIconButton onClick={onRetry} />}
          {(status.label === "Error" || status.label === "Stopped") && canResumeFromError && (
            <ResumeIconButton onClick={onResumeFromError} title="Retry from last failure" />
          )}
          {isActive && status.label === "Running" && <PauseIconButton onClick={onPause} />}
          {isActive && status.label === "Paused" && <ResumeIconButton onClick={onResume} />}
          {isActive && <StopIconButton onClick={onStop} />}
        </div>
      </div>
      <div className="text-sm font-mono text-gray-300 mt-0.5">{stateDisplay}</div>
      <div className="mt-2">
        <ProgressBar pct={pct} total={total} filled={filled} color={status.color} />
      </div>
    </div>
  );
}

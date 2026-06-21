// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useTranslation } from "react-i18next";
import type { SystemError } from "../hooks/useWorkflows";

interface Props {
  errors: SystemError[];
  onDismiss: (id: number) => void;
}

export function SystemErrorAlert({ errors, onDismiss }: Props) {
  const { t } = useTranslation("errors");
  if (errors.length === 0) return null;

  return (
    <div className="fixed bottom-4 right-4 z-50 flex flex-col gap-2 max-w-lg">
      {errors.map((err) => (
        <div
          key={err.id}
          className="bg-red-950/90 border border-red-700/50 rounded-xl p-4 shadow-lg backdrop-blur-sm"
        >
          <div className="flex items-start justify-between gap-3">
            <div className="flex items-start gap-2 min-w-0">
              <svg
                className="w-5 h-5 text-red-400 flex-shrink-0 mt-0.5"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={2}
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z"
                />
              </svg>
              <div className="min-w-0">
                <p className="text-sm font-medium text-red-300">
                  {t("system.commandFailed", { ticketKey: err.ticketKey })}
                </p>
                <pre className="mt-1 text-xs text-red-200/70 whitespace-pre-wrap break-all font-mono max-h-40 overflow-y-auto">
                  {err.message}
                </pre>
              </div>
            </div>
            <button
              onClick={() => onDismiss(err.id)}
              className="text-red-400/60 hover:text-red-300 flex-shrink-0 cursor-pointer"
            >
              &times;
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}

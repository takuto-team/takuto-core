// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Full-screen "Installing dependencies" overlay shown while the server installs
 * the agent + Atlassian CLIs at startup (they are no longer baked into the
 * image). Polls `GET /api/system/dependencies`; renders only while installing
 * (or on error). The current step updates as each CLI installs.
 */

import { useDependencyStatus } from "../hooks/useDependencyStatus";

export function DependencyInstallOverlay() {
  const status = useDependencyStatus();
  if (!status || (status.phase !== "installing" && status.phase !== "error")) {
    return null;
  }

  const isError = status.phase === "error";

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-gray-950/85 backdrop-blur-sm">
      <div className="max-w-md w-full mx-4 rounded-xl border border-gray-800 bg-gray-900 p-8 text-center space-y-4">
        {isError ? (
          <h2 className="text-lg font-semibold text-red-400">
            Could not install dependencies
          </h2>
        ) : (
          <>
            <div
              className="mx-auto h-8 w-8 rounded-full border-2 border-gray-700 border-t-blue-500 animate-spin"
              aria-hidden="true"
            />
            <h2 className="text-lg font-semibold text-gray-100">Installing dependencies</h2>
          </>
        )}

        {isError ? (
          <p className="text-sm text-red-300 break-words">{status.error}</p>
        ) : (
          <>
            <p className="text-sm text-gray-400">{status.current_step || "Preparing…"}</p>
            {status.total > 0 && (
              <p className="text-xs font-mono text-gray-500">
                {Math.min(status.done + 1, status.total)} / {status.total}
              </p>
            )}
          </>
        )}

        <p className="text-xs text-gray-500">
          {isError
            ? "Agent CLIs are installed at startup. Check the server logs, then restart."
            : "The agent CLIs are being installed on first start. This only takes a moment."}
        </p>
      </div>
    </div>
  );
}

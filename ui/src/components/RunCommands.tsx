// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { apiPost } from "../api/client";
import type { RunCommandStatus } from "../api/types";
import { StopSquareIcon, CopyIcon, ExternalLinkIcon, PlayIcon } from "./icons";

export function RunCommands({
  ticketKey,
  commands,
  withLoading,
}: {
  ticketKey: string;
  commands: RunCommandStatus[];
  withLoading: (fn: () => Promise<void>, message?: string) => Promise<void>;
}) {
  const startCmd = (index: number) => async () => {
    const res = await apiPost(`/api/work-items/${encodeURIComponent(ticketKey)}/run-commands/${index}/start`);
    if (!res.ok) {
      const t = await res.text();
      throw new Error(t || "Failed to start run command");
    }
  };

  const stopCmd = (index: number) => async () => {
    const res = await apiPost(`/api/work-items/${encodeURIComponent(ticketKey)}/run-commands/${index}/stop`);
    if (!res.ok) {
      const t = await res.text();
      throw new Error(t || "Failed to stop run command");
    }
  };

  const copyUrl = (proxyUrl: string) => {
    const url = window.location.origin + proxyUrl;
    navigator.clipboard.writeText(url).catch(() => {
      // Fallback for insecure contexts
      const ta = document.createElement("textarea");
      ta.value = url;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
    });
  };

  return (
    <>
      <div className="border-t border-gray-800/60" />
      <div>
        <div className="text-xs text-gray-500 mb-1.5">Commands</div>
      <div className="flex flex-col gap-1.5">
        {commands.map((cmd) => (
          <div key={cmd.index} className="flex items-center gap-2 flex-wrap">
            {cmd.running ? (
              <>
                <button
                  onClick={() => withLoading(stopCmd(cmd.index))}
                  className="action-btn wf-btn-danger inline-flex items-center gap-1"
                >
                  <StopSquareIcon /> Stop {cmd.name}
                </button>
                {cmd.forwarded_port ? (
                  <>
                    <button
                      onClick={() => copyUrl(cmd.forwarded_port![1])}
                      className="action-btn wf-btn-secondary inline-flex items-center gap-1"
                      title={`Copy ${window.location.origin}${cmd.forwarded_port[1]}`}
                    >
                      <CopyIcon /> Copy
                    </button>
                    <a
                      href={cmd.forwarded_port[1]}
                      target="_blank"
                      rel="noopener"
                      className="action-btn wf-btn-secondary inline-flex items-center gap-1"
                    >
                      <ExternalLinkIcon /> Open
                    </a>
                  </>
                ) : (
                  <>
                    <span
                      className="action-btn wf-btn-secondary opacity-50 cursor-not-allowed inline-flex items-center gap-1"
                      title="No listening port detected"
                    >
                      <CopyIcon /> Copy
                    </span>
                    <span
                      className="action-btn wf-btn-secondary opacity-50 cursor-not-allowed inline-flex items-center gap-1"
                      title="No listening port detected"
                    >
                      <ExternalLinkIcon /> Open
                    </span>
                  </>
                )}
              </>
            ) : (
              <button
                onClick={() => withLoading(startCmd(cmd.index), `Starting ${cmd.name}`)}
                className="action-btn wf-btn-primary inline-flex items-center gap-1"
              >
                <PlayIcon /> Run {cmd.name}
              </button>
            )}
          </div>
        ))}
      </div>
      </div>
    </>
  );
}

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { apiPost } from "../api/client";
import type { RunCommandStatus } from "../api/types";
import { StopSquareIcon, CopyIcon, ExternalLinkIcon, PlayIcon } from "./icons";

/** Copy/Open shown while the command is running but no listening port has been
 *  detected yet — the URL isn't ready, so the action is covered by a spinner. */
function PendingActionButton({ icon, label }: { icon: ReactNode; label: string }) {
  const { t } = useTranslation("dashboard");
  return (
    <span
      className="action-btn wf-btn-secondary relative inline-flex items-center gap-1 cursor-progress"
      title={t("runCommands.waitingToListen")}
      aria-busy="true"
    >
      <span className="inline-flex items-center gap-1 opacity-30">
        {icon} {label}
      </span>
      <span className="absolute inset-0 flex items-center justify-center rounded-[inherit] bg-gray-900/30">
        <span className="w-3.5 h-3.5 border-2 border-gray-500 border-t-blue-400 rounded-full animate-spin" />
      </span>
    </span>
  );
}

export function RunCommands({
  ticketKey,
  commands,
  withLoading,
  disabled,
}: {
  ticketKey: string;
  commands: RunCommandStatus[];
  withLoading: (fn: () => Promise<void>, message?: string) => Promise<void>;
  /** When true (e.g. the item's worktree is still preparing), the Run/Stop
   *  controls are disabled. */
  disabled?: boolean;
}) {
  const { t } = useTranslation("dashboard");
  const startCmd = (index: number) => async () => {
    const res = await apiPost(`/api/work-items/${encodeURIComponent(ticketKey)}/run-commands/${index}/start`);
    if (!res.ok) {
      const body = await res.text();
      throw new Error(body || t("runCommands.startFailed"));
    }
  };

  const stopCmd = (index: number) => async () => {
    const res = await apiPost(`/api/work-items/${encodeURIComponent(ticketKey)}/run-commands/${index}/stop`);
    if (!res.ok) {
      const body = await res.text();
      throw new Error(body || t("runCommands.stopFailed"));
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
        <div className="text-xs text-gray-500 mb-1.5">{t("runCommands.heading")}</div>
      <div className="flex flex-col gap-1.5">
        {commands.map((cmd) => (
          <div key={cmd.index} className="flex items-center gap-2 flex-wrap">
            {cmd.running ? (
              <>
                <button
                  onClick={() => withLoading(stopCmd(cmd.index))}
                  disabled={disabled}
                  className="action-btn wf-btn-danger inline-flex items-center gap-1 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  <StopSquareIcon /> {t("runCommands.stop", { name: cmd.name })}
                </button>
                {cmd.forwarded_port ? (
                  <>
                    <button
                      onClick={() => copyUrl(cmd.forwarded_port![1])}
                      className="action-btn wf-btn-secondary inline-flex items-center gap-1"
                      title={t("runCommands.copyTitle", { url: `${window.location.origin}${cmd.forwarded_port[1]}` })}
                    >
                      <CopyIcon /> {t("runCommands.copy")}
                    </button>
                    <a
                      href={cmd.forwarded_port[1]}
                      target="_blank"
                      rel="noopener"
                      className="action-btn wf-btn-secondary inline-flex items-center gap-1"
                    >
                      <ExternalLinkIcon /> {t("runCommands.open")}
                    </a>
                  </>
                ) : (
                  <>
                    <PendingActionButton icon={<CopyIcon />} label={t("runCommands.copy")} />
                    <PendingActionButton icon={<ExternalLinkIcon />} label={t("runCommands.open")} />
                  </>
                )}
              </>
            ) : (
              <button
                onClick={() => withLoading(startCmd(cmd.index), t("runCommands.starting", { name: cmd.name }))}
                disabled={disabled}
                title={disabled ? t("runCommands.preparingWorktree") : undefined}
                className="action-btn wf-btn-primary inline-flex items-center gap-1 disabled:opacity-50 disabled:cursor-not-allowed"
              >
                <PlayIcon /> {t("runCommands.run", { name: cmd.name })}
              </button>
            )}
          </div>
        ))}
      </div>
      </div>
    </>
  );
}

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useRef, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import type { ImproveResponse } from "../api/types";

const PROMPT_TIMEOUT_SECS = 300;

function formatCountdown(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

interface AiPromptPanelProps {
  ticketKey: string;
  ticketTitle: string;
  ticketDescription: string;
  disabled?: boolean;
  /** Notify parent when loading state changes (for mutual exclusion with other AI features). */
  onLoadingChange?: (loading: boolean) => void;
  /** Called with the AI-improved description so the parent can show a diff for review. */
  onImprovement: (
    originalDescription: string,
    improvedDescription: string,
    improvedSummary?: string
  ) => void;
}

export function AiPromptPanel({
  ticketKey,
  ticketTitle,
  ticketDescription,
  disabled,
  onLoadingChange,
  onImprovement,
}: AiPromptPanelProps) {
  const { t } = useTranslation("modals");
  const [prompt, setPrompt] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState(true);
  const [countdown, setCountdown] = useState(PROMPT_TIMEOUT_SECS);

  const abortRef = useRef<AbortController | null>(null);
  const countdownRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    onLoadingChange?.(loading);
  }, [loading, onLoadingChange]);

  useEffect(() => {
    return () => {
      abortRef.current?.abort();
      if (countdownRef.current) clearInterval(countdownRef.current);
    };
  }, []);

  const handleSend = useCallback(async () => {
    if (prompt.trim() === "" || loading) return;

    setLoading(true);
    setError(null);
    setCountdown(PROMPT_TIMEOUT_SECS);

    if (countdownRef.current) clearInterval(countdownRef.current);
    countdownRef.current = setInterval(() => {
      setCountdown((prev) => Math.max(0, prev - 1));
    }, 1000);

    abortRef.current = new AbortController();
    const snapshotDescription = ticketDescription;

    try {
      const res = await fetch(
        `/api/tickets/${encodeURIComponent(ticketKey)}/improve`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          credentials: "same-origin",
          body: JSON.stringify({
            description: snapshotDescription,
            summary: ticketTitle,
            prompt,
          }),
          signal: abortRef.current.signal,
        }
      );
      abortRef.current = null;

      if (!res.ok) {
        const text = await res.text();
        setError(text || t("aiPrompt.requestFailedHttp", { status: res.status }));
        return;
      }

      const data: ImproveResponse = await res.json();
      setPrompt("");
      setCollapsed(true);
      onImprovement(
        snapshotDescription,
        data.improved_description,
        data.improved_summary
      );
    } catch (e) {
      abortRef.current = null;
      if (e instanceof Error && e.name !== "AbortError") {
        setError(e.message || t("aiPrompt.requestFailed"));
      }
    } finally {
      setLoading(false);
      if (countdownRef.current) {
        clearInterval(countdownRef.current);
        countdownRef.current = null;
      }
    }
  }, [prompt, loading, ticketKey, ticketTitle, ticketDescription, onImprovement, t]);

  const handleCancel = useCallback(() => {
    abortRef.current?.abort();
    abortRef.current = null;
    setLoading(false);
    if (countdownRef.current) {
      clearInterval(countdownRef.current);
      countdownRef.current = null;
    }
  }, []);

  const inputDisabled = disabled || loading;

  return (
    <div className="border-t border-gray-800">
      {/* Collapsible header */}
      <button
        onClick={() => setCollapsed(!collapsed)}
        className="w-full flex items-center gap-2 px-4 py-2 text-xs text-purple-300 hover:bg-purple-600/10 cursor-pointer select-none"
      >
        <span className="text-[10px]">{collapsed ? "▶" : "▼"}</span>
        <span>{t("aiPrompt.askAi")}</span>
        {loading && (
          <span className="ml-auto text-gray-500">
            {t("aiPrompt.remaining", { time: formatCountdown(countdown) })}
          </span>
        )}
      </button>

      {!collapsed && (
        <div className="px-4 pb-3 space-y-3">
          <div className="flex flex-col gap-2">
            <textarea
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              placeholder={t("aiPrompt.placeholder")}
              disabled={inputDisabled}
              rows={3}
              className="w-full bg-gray-950 border border-gray-700 rounded-lg p-3 text-sm text-gray-200 resize-none placeholder-gray-600 disabled:opacity-50"
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                  e.preventDefault();
                  handleSend();
                }
              }}
            />
            <div className="flex items-center justify-between">
              <span className="text-[10px] text-gray-600">
                {loading
                  ? t("aiPrompt.applyingChanges", { time: formatCountdown(countdown) })
                  : t("aiPrompt.ctrlEnter")}
              </span>
              <div className="flex gap-2">
                {loading ? (
                  <>
                    <div className="flex items-center gap-2">
                      <div className="w-3 h-3 border border-gray-600 border-t-purple-400 rounded-full animate-spin" />
                    </div>
                    <button
                      onClick={handleCancel}
                      className="text-xs px-3 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
                    >
                      {t("aiPrompt.cancel")}
                    </button>
                  </>
                ) : (
                  <button
                    onClick={handleSend}
                    disabled={inputDisabled || prompt.trim() === ""}
                    className="text-xs px-3 py-1.5 rounded-lg bg-purple-600/20 text-purple-300 border border-purple-500/30 hover:bg-purple-600/30 disabled:opacity-50 cursor-pointer"
                  >
                    {t("aiPrompt.apply")}
                  </button>
                )}
              </div>
            </div>
          </div>

          {error && (
            <div className="flex items-start gap-2 p-3 bg-red-900/20 border border-red-500/30 rounded-lg">
              <p className="text-xs text-red-300 flex-1">{error}</p>
              <button
                onClick={handleSend}
                disabled={loading}
                className="text-xs px-2 py-1 rounded bg-red-600/20 text-red-300 border border-red-500/30 hover:bg-red-600/30 cursor-pointer flex-shrink-0"
              >
                {t("aiPrompt.retry")}
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Claude `~/.claude.json` paste field + the tab button used to switch
 * between API-key and session tabs on the Claude card. Extracted from
 * `MyCredentialsSection.tsx` so the AI panel can stay under ~150 LOC.
 */

import { useState } from "react";
import { Trans, useTranslation } from "react-i18next";

/**
 * Tab button for the Claude auth-method selector. Renders a small dot
 * indicator when that kind is already connected so the user can see at a
 * glance which mode(s) they've already saved.
 */
export function ClaudeAuthTabButton({
  isActive,
  connected,
  onClick,
  label,
}: {
  isActive: boolean;
  connected: boolean;
  onClick: () => void;
  label: string;
}) {
  const { t } = useTranslation("credentials");
  return (
    <button
      type="button"
      role="tab"
      aria-selected={isActive}
      onClick={onClick}
      className={`px-3 py-1.5 text-xs rounded-md cursor-pointer transition-colors flex items-center gap-1.5 ${
        isActive ? "bg-gray-800 text-white" : "text-gray-400 hover:text-gray-200"
      }`}
    >
      {connected && (
        <span
          aria-label={t("my.ai.connectedDot")}
          className="inline-block w-1.5 h-1.5 rounded-full bg-green-400"
        />
      )}
      {label}
    </button>
  );
}

/**
 * `~/.claude.json` paste field — large textarea with inline help and a
 * client-side validation message slot. The Save handler runs the structural
 * check (`parseClaudeSessionBlob`) before the POST so users see obvious
 * shape problems without a round-trip.
 */
export function ClaudeSessionField({
  value,
  onChange,
  onSubmit,
  saving,
  error,
  connected,
  helper,
  hideSave = false,
}: {
  value: string;
  onChange: (v: string) => void;
  onSubmit: () => void;
  saving: boolean;
  error: string | null;
  connected: boolean;
  helper: string;
  /** Hide the field's own Save button — persisted by a page-level Save. */
  hideSave?: boolean;
}) {
  const { t } = useTranslation("credentials");
  const [showHelp, setShowHelp] = useState(false);
  const canSubmit = !saving && value.trim().length > 0;
  return (
    <div className="flex flex-col gap-2">
      <label
        htmlFor="claude-session-textarea"
        className="text-xs text-gray-400"
      >
        <Trans
          i18nKey="my.claude.sessionLabel"
          ns="credentials"
          components={{ code: <code className="text-gray-300" /> }}
        />
      </label>
      <textarea
        id="claude-session-textarea"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={t("my.claude.sessionPlaceholder")}
        rows={12}
        spellCheck={false}
        autoComplete="off"
        disabled={saving}
        className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-xs text-gray-200 font-mono whitespace-pre"
        aria-invalid={error !== null}
        aria-describedby={error ? "claude-session-error" : undefined}
      />
      {error && (
        <p
          id="claude-session-error"
          role="alert"
          className="text-xs text-red-300 bg-red-950/40 border border-red-700/50 rounded px-2 py-1.5"
        >
          {error}
        </p>
      )}
      <p className="text-xs text-gray-500">{helper}</p>
      <p className="text-xs text-gray-500">
        <button
          type="button"
          onClick={() => setShowHelp((v) => !v)}
          className="text-blue-400 hover:text-blue-300 cursor-pointer"
          aria-expanded={showHelp}
        >
          {showHelp ? t("my.claude.hideHelp") : t("my.claude.whereToFind")}
        </button>
      </p>
      {showHelp && (
        <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-3 text-xs text-gray-400 space-y-2">
          <p>
            <Trans
              i18nKey="my.claude.help.shell"
              ns="credentials"
              components={{ code: <code className="text-gray-300" /> }}
            />
          </p>
          <p>
            <Trans
              i18nKey="my.claude.help.fields"
              ns="credentials"
              components={{
                code0: <code className="text-gray-300" />,
                code1: <code className="text-gray-300" />,
                code2: <code className="text-gray-300" />,
                code3: <code className="text-gray-300" />,
              }}
            />
          </p>
          <p>
            <Trans
              i18nKey="my.claude.help.bearer"
              ns="credentials"
              components={{ strong: <strong /> }}
            />
          </p>
        </div>
      )}
      {!hideSave && (
        <div className="flex justify-end">
          <button
            type="button"
            disabled={!canSubmit}
            onClick={onSubmit}
            className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
          >
            {saving
              ? t("actions.saving")
              : connected
                ? t("my.claude.replaceSession")
                : t("my.claude.saveSession")}
          </button>
        </div>
      )}
    </div>
  );
}

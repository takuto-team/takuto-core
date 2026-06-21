// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Ordered run-command list editor for the per-user-per-workspace Worktree
 * Settings tab.
 *
 * Each row holds a `{ name, command }` pair:
 *   - `name`: short label rendered on the dashboard run-command button (e.g.
 *     "Dashboard UI", "Storybook"). Single-line text input.
 *   - `command`: shell command executed when the button is clicked. Multi-line
 *     `<textarea>` — newlines inside a single command are allowed.
 *
 * Up/down reorder buttons, delete, and "Add run command" mirror the shape of
 * `WorktreeCommandList` so the two editors feel identical. Drag-and-drop is
 * deliberately avoided to stay accessible and library-free.
 */

import { useTranslation } from "react-i18next";
import type { RunCommand } from "../api/client";

interface Props {
  commands: RunCommand[];
  onChange: (next: RunCommand[]) => void;
  disabled?: boolean;
}

export function WorktreeRunCommandList({ commands, onChange, disabled }: Props) {
  const { t } = useTranslation("config");
  const updateAt = (index: number, patch: Partial<RunCommand>) => {
    const next = commands.slice();
    next[index] = { ...next[index], ...patch };
    onChange(next);
  };

  const removeAt = (index: number) => {
    const next = commands.slice();
    next.splice(index, 1);
    onChange(next);
  };

  const moveUp = (index: number) => {
    if (index === 0) return;
    const next = commands.slice();
    [next[index - 1], next[index]] = [next[index], next[index - 1]];
    onChange(next);
  };

  const moveDown = (index: number) => {
    if (index >= commands.length - 1) return;
    const next = commands.slice();
    [next[index], next[index + 1]] = [next[index + 1], next[index]];
    onChange(next);
  };

  const addCommand = () => {
    onChange([...commands, { name: "", command: "" }]);
  };

  return (
    <div className="space-y-3">
      {commands.length === 0 && (
        <p className="text-sm text-gray-500 italic">{t("worktreeSettings.runList.empty")}</p>
      )}
      <ol className="space-y-3 list-none">
        {commands.map((rc, i) => (
          <li
            key={i}
            className="flex items-start gap-2 bg-gray-950 border border-gray-800 rounded-lg p-3"
          >
            <span className="text-xs text-gray-500 font-mono pt-2 select-none w-6 text-right">
              {i + 1}.
            </span>
            <div className="flex-1 space-y-2">
              <div className="flex items-center gap-2">
                <label className="text-xs text-gray-500 w-20 shrink-0 font-mono">{t("worktreeSettings.runList.name")}</label>
                <input
                  type="text"
                  value={rc.name}
                  onChange={(e) => updateAt(i, { name: e.target.value })}
                  disabled={disabled}
                  spellCheck={false}
                  placeholder={t("worktreeSettings.runList.namePlaceholder")}
                  maxLength={100}
                  className="flex-1 bg-gray-900 border border-gray-700 rounded px-2 py-1.5 text-sm font-mono text-gray-200 disabled:opacity-60"
                />
              </div>
              <div className="flex items-start gap-2">
                <label className="text-xs text-gray-500 w-20 shrink-0 font-mono pt-2">
                  {t("worktreeSettings.runList.command")}
                </label>
                <textarea
                  value={rc.command}
                  onChange={(e) => updateAt(i, { command: e.target.value })}
                  disabled={disabled}
                  spellCheck={false}
                  rows={Math.max(1, Math.min(8, rc.command.split("\n").length))}
                  placeholder={t("worktreeSettings.runList.commandPlaceholder")}
                  className="flex-1 bg-gray-900 border border-gray-700 rounded px-2 py-1.5 text-sm font-mono text-gray-200 resize-y disabled:opacity-60"
                />
              </div>
            </div>
            <div className="flex flex-col gap-1">
              <button
                type="button"
                onClick={() => moveUp(i)}
                disabled={disabled || i === 0}
                aria-label={t("worktreeSettings.runList.moveUp")}
                title={t("worktreeSettings.runList.moveUp")}
                className="px-2 py-0.5 rounded bg-gray-800 text-gray-300 text-xs hover:bg-gray-700 disabled:opacity-30 disabled:cursor-not-allowed cursor-pointer"
              >
                ↑
              </button>
              <button
                type="button"
                onClick={() => moveDown(i)}
                disabled={disabled || i >= commands.length - 1}
                aria-label={t("worktreeSettings.runList.moveDown")}
                title={t("worktreeSettings.runList.moveDown")}
                className="px-2 py-0.5 rounded bg-gray-800 text-gray-300 text-xs hover:bg-gray-700 disabled:opacity-30 disabled:cursor-not-allowed cursor-pointer"
              >
                ↓
              </button>
              <button
                type="button"
                onClick={() => removeAt(i)}
                disabled={disabled}
                aria-label={t("worktreeSettings.runList.deleteRunCommand")}
                title={t("worktreeSettings.runList.delete")}
                className="px-2 py-0.5 rounded bg-red-900/40 text-red-300 text-xs hover:bg-red-900/70 disabled:opacity-30 disabled:cursor-not-allowed cursor-pointer"
              >
                ✕
              </button>
            </div>
          </li>
        ))}
      </ol>
      <button
        type="button"
        onClick={addCommand}
        disabled={disabled}
        className="px-3 py-1.5 rounded-lg bg-gray-800 text-gray-300 text-sm font-medium border border-gray-700 hover:bg-gray-700 disabled:opacity-50 cursor-pointer"
      >
        {t("worktreeSettings.runList.addRunCommand")}
      </button>
    </div>
  );
}

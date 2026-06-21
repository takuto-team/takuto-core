// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Ordered command-list editor.
 *
 * Each command is rendered as a `<textarea>` (newlines allowed — each entry
 * runs as a single `bash -lc` invocation in the worktree). Rows can be
 * reordered with up/down buttons, added, or deleted. Drag-and-drop is
 * deliberately avoided to stay accessible and library-free; command lists
 * are typically <10 entries.
 */

import { useTranslation } from "react-i18next";

interface Props {
  commands: string[];
  onChange: (next: string[]) => void;
  disabled?: boolean;
}

export function WorktreeCommandList({ commands, onChange, disabled }: Props) {
  const { t } = useTranslation("config");
  const updateAt = (index: number, value: string) => {
    const next = commands.slice();
    next[index] = value;
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
    onChange([...commands, ""]);
  };

  return (
    <div className="space-y-3">
      {commands.length === 0 && (
        <p className="text-sm text-gray-500 italic">
          {t("repositories.commands.empty")}
        </p>
      )}
      <ol className="space-y-3 list-none">
        {commands.map((cmd, i) => (
          <li
            key={i}
            className="flex items-start gap-2 bg-gray-950 border border-gray-800 rounded-lg p-3"
          >
            <span className="text-xs text-gray-500 font-mono pt-2 select-none w-6 text-right">
              {i + 1}.
            </span>
            <textarea
              value={cmd}
              onChange={(e) => updateAt(i, e.target.value)}
              disabled={disabled}
              spellCheck={false}
              rows={Math.max(1, Math.min(8, cmd.split("\n").length))}
              placeholder={t("repositories.commands.placeholder")}
              className="flex-1 bg-gray-900 border border-gray-700 rounded px-2 py-1.5 text-sm font-mono text-gray-200 resize-y disabled:opacity-60"
            />
            <div className="flex flex-col gap-1">
              <button
                type="button"
                onClick={() => moveUp(i)}
                disabled={disabled || i === 0}
                aria-label={t("repositories.commands.moveUp")}
                title={t("repositories.commands.moveUp")}
                className="px-2 py-0.5 rounded bg-gray-800 text-gray-300 text-xs hover:bg-gray-700 disabled:opacity-30 disabled:cursor-not-allowed cursor-pointer"
              >
                ↑
              </button>
              <button
                type="button"
                onClick={() => moveDown(i)}
                disabled={disabled || i >= commands.length - 1}
                aria-label={t("repositories.commands.moveDown")}
                title={t("repositories.commands.moveDown")}
                className="px-2 py-0.5 rounded bg-gray-800 text-gray-300 text-xs hover:bg-gray-700 disabled:opacity-30 disabled:cursor-not-allowed cursor-pointer"
              >
                ↓
              </button>
              <button
                type="button"
                onClick={() => removeAt(i)}
                disabled={disabled}
                aria-label={t("repositories.commands.deleteCommand")}
                title={t("repositories.commands.delete")}
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
        {t("repositories.commands.addCommand")}
      </button>
    </div>
  );
}

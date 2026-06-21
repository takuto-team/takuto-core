// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Left-pane repository picker, shared by Repository Settings and Workflows.
 *
 * Each row shows the repo name; the optional green-dot "set" / gray-dot "none"
 * badge is rendered only when an item supplies `hasCommands` (Repository
 * Settings uses it to flag which repos have init/run commands; Workflows omits
 * it). Presentational — the parent owns the repo list, selection, and loading.
 */

import { useTranslation } from "react-i18next";

export interface RepoSidebarItem {
  name: string;
  /** When defined, render a "set"/"none" badge; when undefined, no badge. */
  hasCommands?: boolean;
  /** `false` ⇒ the GitHub App can no longer reach it: disabled + sorted last. */
  accessible?: boolean;
}

interface Props {
  repos: RepoSidebarItem[];
  loading: boolean;
  selected: string | null;
  onSelect: (name: string) => void;
}

function CommandsBadge({ hasCommands }: { hasCommands: boolean }) {
  const { t } = useTranslation("config");
  return hasCommands ? (
    <span
      className="inline-flex items-center gap-1 text-[11px] text-emerald-300"
      title={t("repositories.sidebar.commandsSetTitle")}
    >
      <span className="w-2 h-2 rounded-full bg-emerald-400" />
      {t("repositories.sidebar.commandsSet")}
    </span>
  ) : (
    <span
      className="inline-flex items-center gap-1 text-[11px] text-gray-500"
      title={t("repositories.sidebar.commandsNoneTitle")}
    >
      <span className="w-2 h-2 rounded-full bg-gray-600" />
      {t("repositories.sidebar.commandsNone")}
    </span>
  );
}

export function RepoSidebar({ repos, loading, selected, onSelect }: Props) {
  const { t } = useTranslation("config");
  // Accessible repos keep their order; inaccessible ones drop to the end.
  const ordered = [...repos].sort(
    (a, b) => Number(a.accessible === false) - Number(b.accessible === false),
  );

  return (
    <aside className="md:w-1/3 md:max-w-xs border border-gray-800 rounded-lg bg-gray-950 overflow-hidden">
      <div className="px-3 py-2 border-b border-gray-800 text-xs uppercase tracking-wide text-gray-500">
        {t("repositories.sidebar.header")}
      </div>
      {loading ? (
        <p className="text-sm text-gray-500 p-3">{t("actions.loading")}</p>
      ) : ordered.length === 0 ? (
        <p className="text-sm text-gray-500 p-3">{t("repositories.sidebar.none")}</p>
      ) : (
        <ul className="divide-y divide-gray-800">
          {ordered.map((r) =>
            r.accessible === false ? (
              <li key={r.name}>
                <div
                  aria-disabled="true"
                  title={t("repositories.sidebar.noAccessTitle")}
                  className="w-full flex items-center justify-between gap-2 px-3 py-2 text-sm text-gray-500 opacity-60 cursor-not-allowed"
                >
                  <span className="truncate font-medium">{r.name}</span>
                  <span className="shrink-0 text-[11px] font-medium text-red-400">{t("repositories.sidebar.noAccess")}</span>
                </div>
              </li>
            ) : (
              <li key={r.name}>
                <button
                  type="button"
                  onClick={() => onSelect(r.name)}
                  className={`w-full flex items-center justify-between gap-2 px-3 py-2 text-left text-sm cursor-pointer transition-colors ${
                    selected === r.name
                      ? "bg-blue-950/40 text-blue-200"
                      : "text-gray-300 hover:bg-gray-900"
                  }`}
                >
                  <span className="truncate font-medium">{r.name}</span>
                  {r.hasCommands !== undefined && (
                    <span className="flex items-center gap-1.5 shrink-0">
                      <CommandsBadge hasCommands={r.hasCommands} />
                    </span>
                  )}
                </button>
              </li>
            ),
          )}
        </ul>
      )}
    </aside>
  );
}

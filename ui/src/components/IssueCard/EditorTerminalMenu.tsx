// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useTranslation } from "react-i18next";
import { ExternalLinkIcon, StopSquareIcon, EditorIcon, TerminalIcon } from "../icons";

type Kind = "editor" | "terminal";

interface Props {
  kind: Kind;
  /** Live URL when the editor / terminal is already running. `null` / `undefined`
   *  means the icon click should trigger the open-action instead of opening
   *  the menu. The two empty shapes mirror the API types where the server can
   *  return either. */
  url?: string | null;
  /** Open menu state lifted to the parent so the editor / terminal / port menus
   * can be mutually exclusive (one open at a time). */
  isMenuOpen: boolean;
  onToggleMenu: (open: boolean) => void;
  /** Invoked when the icon is clicked while `url` is unset — kicks off the
   * "Setting up secure connection…" flow in the parent. */
  onStart: () => void;
  /** Invoked from the "Stop" menu item. */
  onStop: () => void;
}

export function EditorTerminalMenu({ kind, url, isMenuOpen, onToggleMenu, onStart, onStop }: Props) {
  const { t } = useTranslation("dashboard");
  const copy = {
    title: t(`editorMenu.${kind}.title`),
    running: t(`editorMenu.${kind}.running`),
    idle: t(`editorMenu.${kind}.idle`),
    stop: t(`editorMenu.${kind}.stop`),
  };
  const Icon = kind === "editor" ? EditorIcon : TerminalIcon;
  const handleClick = () => {
    if (url) {
      onToggleMenu(!isMenuOpen);
    } else {
      onStart();
    }
  };

  return (
    <div className="relative">
      {isMenuOpen && url && (
        <>
          <div className="fixed inset-0" onClick={() => onToggleMenu(false)} />
          <div className="absolute bottom-full mb-2 right-0 bg-gray-800 border border-gray-700 rounded-lg py-1.5 shadow-xl z-20 min-w-[160px]">
            <div className="px-3 py-1 text-xs text-gray-500 font-medium border-b border-gray-700/60 mb-1">
              {copy.title}
            </div>
            <a
              href={url}
              target="_blank"
              rel="noopener"
              className="flex items-center leading-none gap-2 px-3 py-1.5 text-xs text-gray-300 hover:bg-gray-700 hover:text-white transition-colors"
              onClick={() => onToggleMenu(false)}
            >
              <ExternalLinkIcon />
              {t("editorMenu.openInBrowser")}
            </a>
            <button
              onClick={() => {
                onToggleMenu(false);
                onStop();
              }}
              className="flex w-full items-center leading-none gap-2 px-3 py-1.5 text-xs text-red-400 hover:bg-gray-700 hover:text-red-300 transition-colors"
            >
              <StopSquareIcon />
              {copy.stop}
            </button>
          </div>
        </>
      )}
      <button
        onClick={handleClick}
        title={url ? copy.running : copy.idle}
        className={`cursor-pointer transition-colors ${url ? "text-green-400" : "text-gray-500 hover:text-gray-300"}`}
      >
        <Icon className={kind === "terminal" ? "w-4 h-4" : undefined} />
      </button>
    </div>
  );
}

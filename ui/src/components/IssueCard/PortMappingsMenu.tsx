// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useTranslation } from "react-i18next";
import { MonitorIcon, PortIcon } from "../icons";

interface Props {
  /** `[containerPort, proxyUrl]` pairs, already merged from the API mapping
   *  list + the live dynamic-forwards stream. Empty list = render nothing. */
  ports: [number, string][];
  isMenuOpen: boolean;
  onToggleMenu: (open: boolean) => void;
}

export function PortMappingsMenu({ ports, isMenuOpen, onToggleMenu }: Props) {
  const { t } = useTranslation("dashboard");
  if (ports.length === 0) return null;

  return (
    <div className="relative">
      {isMenuOpen && (
        <>
          <div className="fixed inset-0" onClick={() => onToggleMenu(false)} />
          <div className="absolute bottom-full mb-2 right-0 bg-gray-800 border border-gray-700 rounded-lg py-1.5 shadow-xl z-20 min-w-[180px]">
            <div className="px-3 py-1 text-xs text-gray-500 font-medium border-b border-gray-700/60 mb-1">
              {t("ports.title")}
            </div>
            {ports.map(([cp, proxyUrl]) => (
              <a
                key={`${cp}-${proxyUrl}`}
                href={proxyUrl}
                target="_blank"
                rel="noopener"
                className="flex items-center leading-none gap-2 px-3 py-1.5 text-xs text-gray-300 hover:bg-gray-700 hover:text-white transition-colors"
              >
                <PortIcon />
                {cp} &rarr; {proxyUrl}
              </a>
            ))}
          </div>
        </>
      )}
      <button
        onClick={() => onToggleMenu(!isMenuOpen)}
        title={t("ports.title")}
        className="text-green-400 cursor-pointer"
      >
        <MonitorIcon />
      </button>
    </div>
  );
}

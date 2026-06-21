// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { LANGUAGES, SUPPORTED_LANGUAGES } from "../i18n/config";

/**
 * Header language picker: a clickable current-language code that opens a small
 * dropdown of all supported languages. Mirrors the header's repo-picker /
 * config-menu pattern (open state + ref + click-outside). Selecting a language
 * calls `i18n.changeLanguage`, which the detector persists to localStorage.
 */
export function LanguageSwitcher() {
  const { i18n, t } = useTranslation("common");
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    function handleClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, [open]);

  // Reflect the user's CHOSEN language (i18n.language), not resolvedLanguage —
  // the latter falls back to English for any language whose resources aren't
  // populated yet, which would wrongly show "EN" after picking that language.
  // Normalize a region code (fr-FR) to its base (fr).
  const base = (i18n.language || "en").split("-")[0];
  const current = SUPPORTED_LANGUAGES.includes(base as never) ? base : "en";
  const currentLabel = LANGUAGES.find((l) => l.code === current)?.label ?? "EN";

  return (
    <div className="relative" ref={ref}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="p-1.5 rounded text-xs font-medium text-gray-400 hover:text-gray-200 hover:bg-gray-800 transition-colors cursor-pointer min-w-[2rem]"
        aria-label={t("language.switch")}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        {currentLabel}
      </button>
      {open && (
        <div
          role="menu"
          className="absolute right-0 mt-1 w-28 bg-gray-900 border border-gray-700 rounded-lg shadow-lg py-1 z-50"
        >
          {LANGUAGES.map((l) => (
            <button
              key={l.code}
              type="button"
              role="menuitem"
              lang={l.code}
              aria-current={l.code === current}
              onClick={() => {
                void i18n.changeLanguage(l.code);
                setOpen(false);
              }}
              className={`w-full text-left px-3 py-1.5 text-sm transition-colors cursor-pointer ${
                l.code === current
                  ? "bg-blue-950/60 text-blue-300"
                  : "text-gray-300 hover:bg-gray-800 hover:text-white"
              }`}
            >
              {l.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

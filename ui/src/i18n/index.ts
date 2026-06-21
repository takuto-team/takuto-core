// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * i18next bootstrap. Imported for its side effect (`import "./i18n"`) once,
 * before the app renders. Resources are bundled statically (eager glob) so
 * initialization is synchronous — `useTranslation()` resolves on first render
 * with no Suspense, which also keeps unit tests simple.
 *
 * A brand-new visitor gets their browser language when it is one of the
 * supported set (`fr-FR` → `fr`); otherwise English. A previously chosen
 * language (persisted under `takuto.lang`) always wins.
 */

import i18n from "i18next";
import type { Resource } from "i18next";
import { initReactI18next } from "react-i18next";
import LanguageDetector from "i18next-browser-languagedetector";

import { LANGUAGE_STORAGE_KEY, NAMESPACES, SUPPORTED_LANGUAGES } from "./config";

// Every locale JSON, bundled at build time. New languages/namespaces are picked
// up automatically by adding files under src/locales/<lang>/<ns>.json.
const modules = import.meta.glob("../locales/*/*.json", {
  eager: true,
  import: "default",
}) as Record<string, Record<string, unknown>>;

const resources: Resource = {};
for (const [path, content] of Object.entries(modules)) {
  const m = path.match(/\/locales\/([^/]+)\/([^/]+)\.json$/);
  if (!m) continue;
  const [, lng, ns] = m;
  (resources[lng] ??= {})[ns] = content;
}

void i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources,
    fallbackLng: "en",
    supportedLngs: SUPPORTED_LANGUAGES,
    nonExplicitSupportedLngs: true, // map fr-FR → fr
    ns: NAMESPACES as unknown as string[],
    defaultNS: "common",
    detection: {
      order: ["localStorage", "navigator"],
      lookupLocalStorage: LANGUAGE_STORAGE_KEY,
      caches: ["localStorage"],
    },
    interpolation: { escapeValue: false }, // React already escapes
    react: { useSuspense: false },
    returnNull: false,
  });

// Keep <html lang> in sync so screen readers and the browser pick the right
// language for the whole document.
const applyDocumentLang = (lng: string) => {
  if (typeof document !== "undefined") {
    document.documentElement.lang = lng;
  }
};
applyDocumentLang(i18n.resolvedLanguage ?? "en");
i18n.on("languageChanged", applyDocumentLang);

export default i18n;

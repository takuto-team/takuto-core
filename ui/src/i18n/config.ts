// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/** Single source of truth for the supported languages and the switcher labels. */
export const LANGUAGES = [
  { code: "en", label: "EN" },
  { code: "fr", label: "FR" },
  { code: "es", label: "ES" },
  { code: "zh", label: "中文" },
  { code: "ja", label: "日本" },
] as const;

export type LanguageCode = (typeof LANGUAGES)[number]["code"];

export const SUPPORTED_LANGUAGES: LanguageCode[] = LANGUAGES.map((l) => l.code);

/** The translation namespaces, in load order. `common` is the default. */
export const NAMESPACES = [
  "common",
  "status",
  "dashboard",
  "config",
  "onboarding",
  "auth",
  "credentials",
  "modals",
  "errors",
] as const;

/** localStorage key holding the user's chosen language (mirrors takuto.activeRepoName). */
export const LANGUAGE_STORAGE_KEY = "takuto.lang";

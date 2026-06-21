// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Pure helpers shared by the per-user credentials panels. Extracted from the
 * monolithic `MyCredentialsSection.tsx` so each panel can import only what it
 * needs. No React, no fetch — these are display-string formatters.
 */

import i18n from "../../i18n";
import type { GithubAuthMode } from "../../api/types";

export const PROVIDER_LABEL: Record<string, string> = {
  claude: "Claude",
  cursor: "Cursor",
  codex: "Codex",
  opencode: "OpenCode",
  gemini: "Gemini",
};

export function providerHelper(
  provider: string,
  kind: "api_key" | "cli_state",
): string {
  if (kind === "cli_state") {
    // Only Claude renders this branch (task #39 amendment).
    return i18n.t("credentials:provider.helper.claudeSession");
  }
  switch (provider) {
    case "cursor":
      // A1 regression guard: no ttyd / browser-flow vocabulary here.
      return i18n.t("credentials:provider.helper.cursor");
    case "claude":
      return i18n.t("credentials:provider.helper.claude");
    case "codex":
      return i18n.t("credentials:provider.helper.codex");
    case "opencode":
      // Self-hosted spec (lore/audits/2026-05-27-opencode-self-hosted-spec.md
      // §2.5): OpenCode is the self-hosted adapter. The key field is an
      // optional bearer for the endpoint, not an Anthropic / OpenAI key.
      // Leave blank for LM Studio / Ollama; required for authenticated
      // private gateways. Takuto materialises `opencode.json` per
      // workflow with this value as `options.apiKey`.
      return i18n.t("credentials:provider.helper.opencode");
    default:
      return i18n.t("credentials:provider.helper.default");
  }
}

export function describeMode(mode: GithubAuthMode): string {
  switch (mode) {
    case "app":
      return i18n.t("credentials:github.mode.app");
    case "app_plus_pat":
      return i18n.t("credentials:github.mode.appPlusPat");
    case "pat_only":
      return i18n.t("credentials:github.mode.patOnly");
    case "pat_required":
      return i18n.t("credentials:github.mode.patRequired");
    case "missing":
      return i18n.t("credentials:github.mode.missing");
  }
}

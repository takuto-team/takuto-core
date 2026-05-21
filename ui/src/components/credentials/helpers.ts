// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Pure helpers shared by the per-user credentials panels. Extracted from the
 * monolithic `MyCredentialsSection.tsx` so each panel can import only what it
 * needs. No React, no fetch — these are display-string formatters.
 */

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
    return "Required for Pro/Team accounts whose local `claude` uses `/login`. Maestro reads `oauthAccount` from this blob and writes it to the worker's `.claude.json` at workflow start. The bearer token is still set separately on the API key tab.";
  }
  switch (provider) {
    case "cursor":
      // A1 regression guard: no ttyd / browser-flow vocabulary here.
      return "Cursor accepts only an API key. Generate one at cursor.com/dashboard and paste it above.";
    case "claude":
      return "For direct Anthropic API or proxies that accept the same API key format. If you're on Pro/Team and your local `claude` uses `/login`, use 'Claude Code session' instead.";
    case "codex":
      return "OpenAI API key (sk-…). The Codex CLI reads OPENAI_API_KEY from the worker environment — Maestro bridges this from the value you paste here.";
    case "opencode":
      return "OpenCode credential (anthropic-style key or any provider key — depends on which provider you've configured in [agent.providers.opencode]). Note: opencode does NOT auto-read env vars; admin must configure a provider in opencode.json.";
    default:
      return "Paste the API key issued by your provider.";
  }
}

export function describeMode(mode: GithubAuthMode): string {
  switch (mode) {
    case "app":
      return "App only";
    case "app_plus_pat":
      return "App + your PAT";
    case "pat_only":
      return "PAT only";
    case "pat_required":
      return "PAT required";
    case "missing":
      return "Not connected";
  }
}

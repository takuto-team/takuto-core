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
    return "Required for Pro/Team accounts whose local `claude` uses `/login`. Takuto reads `oauthAccount` from this blob and writes it to the worker's `.claude.json` at workflow start. The bearer token is still set separately on the API key tab.";
  }
  switch (provider) {
    case "cursor":
      // A1 regression guard: no ttyd / browser-flow vocabulary here.
      return "Cursor accepts only an API key. Generate one at cursor.com/dashboard and paste it above.";
    case "claude":
      return "For direct Anthropic API or proxies that accept the same API key format. If you're on Pro/Team and your local `claude` uses `/login`, use 'Claude Code session' instead.";
    case "codex":
      return "OpenAI API key (sk-…). The Codex CLI reads OPENAI_API_KEY from the worker environment — Takuto bridges this from the value you paste here.";
    case "opencode":
      // Self-hosted spec (lore/audits/2026-05-27-opencode-self-hosted-spec.md
      // §2.5): OpenCode is the self-hosted adapter. The key field is an
      // optional bearer for the endpoint, not an Anthropic / OpenAI key.
      // Leave blank for LM Studio / Ollama; required for authenticated
      // private gateways. Takuto materialises `opencode.json` per
      // workflow with this value as `options.apiKey`.
      return "Optional bearer token for your self-hosted OpenAI-compatible endpoint. Leave blank for LM Studio / Ollama or any unauthenticated server. For private gateways requiring auth, paste the bearer the server expects.";
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

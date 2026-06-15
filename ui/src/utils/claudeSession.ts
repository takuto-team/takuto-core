// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Client-side validator for the Claude `~/.claude.json` blob (#40).
 *
 * The **server is the source of truth** — see
 * `crates/takuto-web/src/routes/credentials.rs::validate_claude_session_blob`.
 * This module's purpose is only to surface obvious shape problems to the
 * user BEFORE the POST round-trip (#40 T-CLAUDE-UI-006), not to gate the
 * save itself.
 *
 * Required fields (mirror of the Rust validator):
 *   - The blob must parse as JSON.
 *   - Top-level `oauthAccount` must exist and be an object.
 *   - `oauthAccount.accountUuid`, `oauthAccount.emailAddress`, and
 *     `oauthAccount.organizationUuid` must all be present as non-empty
 *     strings.
 *
 * The validator returns a tagged result so the caller can render either
 * a generic toast or a structured error. We do NOT throw — that would
 * coupling the validator to React's error boundaries.
 */

/** Stable error codes the UI maps to copy. */
export type ClaudeSessionError =
  | "empty"
  | "invalid_json"
  | "missing_oauth_account"
  | "missing_required_fields";

/** Outcome of `parseClaudeSessionBlob`. */
export type ClaudeSessionParseResult =
  | { ok: true }
  | { ok: false; code: ClaudeSessionError; message: string };

/** Required keys inside `oauthAccount`. Order matters only for the error
 *  message; the test asserts the set, not the order. */
const REQUIRED_OAUTH_KEYS = ["accountUuid", "emailAddress", "organizationUuid"] as const;

/**
 * Validate a pasted `~/.claude.json` blob. See module doc for the rules.
 * Returns `{ ok: true }` on success, or `{ ok: false, code, message }`.
 */
export function parseClaudeSessionBlob(blob: string): ClaudeSessionParseResult {
  const trimmed = blob.trim();
  if (trimmed.length === 0) {
    return {
      ok: false,
      code: "empty",
      message: "Paste the contents of your local ~/.claude.json file.",
    };
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch (e: unknown) {
    return {
      ok: false,
      code: "invalid_json",
      message: `That doesn't look like valid JSON: ${
        e instanceof Error ? e.message : String(e)
      }`,
    };
  }

  if (!isObject(parsed)) {
    return {
      ok: false,
      code: "invalid_json",
      message: "Expected a JSON object at the top level of ~/.claude.json.",
    };
  }

  const oauth = parsed["oauthAccount"];
  if (!isObject(oauth)) {
    return {
      ok: false,
      code: "missing_oauth_account",
      message:
        "Missing `oauthAccount` — Takuto needs the OAuth block Claude Code wrote at login.",
    };
  }

  const missing: string[] = [];
  for (const key of REQUIRED_OAUTH_KEYS) {
    const v = oauth[key];
    if (typeof v !== "string" || v.trim().length === 0) {
      missing.push(key);
    }
  }
  if (missing.length > 0) {
    return {
      ok: false,
      code: "missing_required_fields",
      message: `oauthAccount is missing required fields: ${missing.join(", ")}. Re-run \`claude /login\` and copy the fresh ~/.claude.json.`,
    };
  }

  return { ok: true };
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

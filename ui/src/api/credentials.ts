// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Per-user credentials.
 *
 * Source of truth: tmp/multi-agents/04_architecture.md §3 + §4.4 +
 * 05_ux_design.md §2.2 / §2.3. Every entry point honours the in-memory mock
 * layer at `./mocks.ts` when `isMocksEnabled()` is true; otherwise it makes a
 * real fetch against the documented endpoints.
 */

import { api } from "./http";
import {
  isMocksEnabled,
  mockDeleteGithubPat,
  mockDeleteProviderCredential,
  mockGetCredentials,
  mockPatchGithubSettings,
  mockSetGithubPat,
  mockSetProviderCredential,
} from "./mocks";
import type {
  PatchGithubSettingsRequest,
  ProviderCredentialKind,
  SetGithubPatRequest,
  SetProviderCredentialRequest,
  UserCredentialsStatus,
} from "./types";

/**
 * Structured error from any per-user credential endpoint. Carries the
 * server's `error` code (or a synthetic `http_<status>` fallback) plus an
 * optional `org_sso_url` when the server reported
 * `sso_authorization_required` (04_architecture.md §4.4 A4).
 */
export class UserCredentialsError extends Error {
  public readonly code: string;
  public readonly status: number;
  public readonly orgSsoUrl: string | null;
  constructor(
    message: string,
    code: string,
    status: number,
    orgSsoUrl: string | null = null,
  ) {
    super(message);
    this.name = "UserCredentialsError";
    this.code = code;
    this.status = status;
    this.orgSsoUrl = orgSsoUrl;
  }
}

/**
 * Shared helper: parse `{ error, message, org_sso_url? }` JSON; fall back to
 * raw text. Same shape as `AgentConfigError` (see `./agentConfig.ts`) but
 * typed differently so each surface can present its own copy.
 */
async function rejectWithCredentialsError(res: Response): Promise<never> {
  let code = `http_${res.status}`;
  let message = `HTTP ${res.status}`;
  let orgSsoUrl: string | null = null;
  const text = await res.text();
  if (text) {
    try {
      const body = JSON.parse(text) as {
        error?: string;
        message?: string;
        org_sso_url?: string;
      };
      if (typeof body.error === "string") code = body.error;
      if (typeof body.message === "string" && body.message.length > 0) {
        message = body.message;
      } else if (typeof body.error === "string") {
        message = body.error;
      } else {
        message = text;
      }
      if (typeof body.org_sso_url === "string") orgSsoUrl = body.org_sso_url;
    } catch {
      message = text;
    }
  }
  throw new UserCredentialsError(message, code, res.status, orgSsoUrl);
}

/** GET /api/users/me/credentials — current per-user readiness flags. */
export async function fetchUserCredentials(): Promise<UserCredentialsStatus> {
  if (isMocksEnabled()) {
    return await mockGetCredentials().json();
  }
  const res = await api("/api/users/me/credentials");
  if (!res.ok) await rejectWithCredentialsError(res);
  return res.json();
}

/**
 * POST /api/users/me/credentials/{provider} — paste-and-save a credential.
 *
 * Body is discriminated by `kind` (task #39):
 *   - omitted / `"api_key"` → `api_key` field carries the bearer string.
 *   - `"cli_state"` → `claude_session_json` field carries the full
 *     `~/.claude.json` blob (Claude only).
 *
 * The server is the source of truth for validation; see
 * `routes/credentials.rs::ApiKeyBody`.
 */
export async function setProviderCredential(
  provider: string,
  body: SetProviderCredentialRequest,
): Promise<void> {
  if (isMocksEnabled()) {
    const r = mockSetProviderCredential(provider, body);
    if (!r.ok) await rejectWithCredentialsError(r);
    return;
  }
  const res = await api(`/api/users/me/credentials/${encodeURIComponent(provider)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) await rejectWithCredentialsError(res);
}

/**
 * Convenience wrapper for the Claude `kind = cli_state` path. Posts the
 * raw `~/.claude.json` blob to the Claude credential endpoint.
 *
 * The blob is NOT validated client-side here — UI code should call
 * `parseClaudeSessionBlob` first to surface obvious shape errors before
 * the round-trip. The server runs the authoritative validation.
 */
export async function setClaudeSession(claudeSessionJson: string): Promise<void> {
  return setProviderCredential("claude", {
    kind: "cli_state",
    claude_session_json: claudeSessionJson,
  });
}

/**
 * DELETE /api/users/me/credentials/{provider} — hard delete.
 *
 * An optional `kind` query parameter scopes the delete to a single slot
 * (api_key or cli_state). Omitting `kind` deletes every row for
 * `(user, provider)` — matches the backend's back-compat behaviour.
 */
export async function deleteProviderCredential(
  provider: string,
  kind?: ProviderCredentialKind,
): Promise<void> {
  if (isMocksEnabled()) {
    const r = mockDeleteProviderCredential(provider, kind);
    if (!r.ok) await rejectWithCredentialsError(r);
    return;
  }
  const qs = kind ? `?kind=${encodeURIComponent(kind)}` : "";
  const res = await api(
    `/api/users/me/credentials/${encodeURIComponent(provider)}${qs}`,
    { method: "DELETE" },
  );
  if (!res.ok && res.status !== 204) await rejectWithCredentialsError(res);
}

/** POST /api/users/me/github-pat — validate scopes + SSO, then seal. */
export async function setGithubPat(
  body: SetGithubPatRequest,
): Promise<UserCredentialsStatus> {
  if (isMocksEnabled()) {
    const r = mockSetGithubPat(body);
    if (!r.ok) await rejectWithCredentialsError(r);
    return r.json();
  }
  const res = await api("/api/users/me/github-pat", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) await rejectWithCredentialsError(res);
  return res.json();
}

/** DELETE /api/users/me/github-pat — hard delete. */
export async function deleteGithubPat(): Promise<UserCredentialsStatus> {
  if (isMocksEnabled()) {
    const r = mockDeleteGithubPat();
    if (!r.ok) await rejectWithCredentialsError(r);
    return r.json();
  }
  const res = await api("/api/users/me/github-pat", { method: "DELETE" });
  if (!res.ok) await rejectWithCredentialsError(res);
  return res.json();
}

/** PATCH /api/users/me/github — toggle attribute_commits (A3 rename). */
export async function patchGithubSettings(
  body: PatchGithubSettingsRequest,
): Promise<UserCredentialsStatus> {
  if (isMocksEnabled()) {
    const r = mockPatchGithubSettings(body);
    if (!r.ok) await rejectWithCredentialsError(r);
    return r.json();
  }
  const res = await api("/api/users/me/github", {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) await rejectWithCredentialsError(res);
  return res.json();
}

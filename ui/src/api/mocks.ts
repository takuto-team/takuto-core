// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Phase 2 mock layer — in-memory backend for the per-user credential
 * endpoints listed in 04_architecture.md §3 + §4. Used by Storybook (and
 * optionally by `npm run dev`) while Phase 2b is still landing the Rust
 * handlers. **Not** shipped in the production bundle: every entry point is
 * gated by `isMocksEnabled()`, which reads
 * `import.meta.env.VITE_USE_MOCKS === 'true'` or a runtime override.
 *
 * Stories drive the runtime override (`setMocksEnabled(true)`) so the env
 * var is not required in CI / `npm run build`. The override also lets each
 * story start from a known fixture by calling `resetMocks(fixture)` in a
 * decorator.
 */

import type {
  PatchGithubSettingsRequest,
  SetGithubPatRequest,
  SetProviderCredentialRequest,
  UserCredentialsStatus,
} from "./types";

/**
 * Build-time toggle: when `VITE_USE_MOCKS` is `"true"` at `vite build` /
 * `vite dev` time, the mock layer is active by default for the whole app.
 * Vite replaces the expression with the literal string at build time, so
 * production builds with the var unset get `'false' === 'true'` → dead-code
 * elimination keeps the mock code out of the hot path.
 */
const ENV_FLAG = import.meta.env.VITE_USE_MOCKS === "true";

let runtimeOverride: boolean | null = null;

/** Returns true when the client should route requests through the mock layer. */
export function isMocksEnabled(): boolean {
  return runtimeOverride ?? ENV_FLAG;
}

/** Override the env-var setting at runtime (used by Storybook decorators). */
export function setMocksEnabled(on: boolean): void {
  runtimeOverride = on;
}

/** Clear the runtime override (revert to whatever the env var says). */
export function clearMocksOverride(): void {
  runtimeOverride = null;
}

// ---------------------------------------------------------------------------
// In-memory state.
// ---------------------------------------------------------------------------

/**
 * Starting fixture. Each call to `resetMocks()` deep-clones this so stories
 * can mutate state freely without leaking into the next render.
 */
/**
 * Starting fixture. `github: null` mirrors the backend's `Option<>` shape —
 * a missing PAT is represented as a null sub-object, NOT an object with
 * `has_pat: false`. See routes/credentials.rs::UserCredentialsStatus.
 */
const DEFAULT_STATE: UserCredentialsStatus = {
  provider: null,
  github: null,
};

let state: UserCredentialsStatus = clone(DEFAULT_STATE);

/** Reset (or replace) the mock state. Pass a fixture to seed a story. */
export function resetMocks(fixture: UserCredentialsStatus = DEFAULT_STATE): void {
  state = clone(fixture);
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

// ---------------------------------------------------------------------------
// Forced-error toggles for stories.
// ---------------------------------------------------------------------------

type ForcedFailure =
  | { kind: "sso_required"; orgUrl: string }
  | { kind: "invalid_token"; message: string }
  | { kind: "http_403"; message: string };

let forcedFailure: ForcedFailure | null = null;

/** Make the next single write (POST/PATCH/DELETE) fail with a typed error. */
export function setNextFailure(f: ForcedFailure | null): void {
  forcedFailure = f;
}

// ---------------------------------------------------------------------------
// Handlers — pure functions that mimic the documented endpoints.
// ---------------------------------------------------------------------------

export function mockGetCredentials(): Response {
  return jsonResponse(200, state);
}

export function mockSetProviderCredential(
  provider: string,
  body: SetProviderCredentialRequest,
): Response {
  const fail = takeFailure();
  if (fail) return failureResponse(fail);
  if (!body.api_key || body.api_key.trim().length === 0) {
    return jsonResponse(400, {
      error: "invalid_token",
      message: "API key cannot be empty.",
    });
  }
  state.provider = {
    // Wire-format note: mirrors routes/credentials.rs::ProviderCredentialStatus.
    provider,
    kind: "api_key",
    active: true,
    last_validated_at: new Date().toISOString(),
    last_used_at: null,
  };
  return new Response(null, { status: 204 });
}

export function mockDeleteProviderCredential(_provider: string): Response {
  state.provider = null;
  return new Response(null, { status: 204 });
}

export function mockSetGithubPat(body: SetGithubPatRequest): Response {
  const fail = takeFailure();
  if (fail) return failureResponse(fail);
  if (!body.pat || body.pat.trim().length === 0) {
    return jsonResponse(400, {
      error: "invalid_token",
      message: "PAT cannot be empty.",
    });
  }
  // Wire-format note: mirrors routes/credentials.rs::GithubCredentialStatus.
  // `mode` is NOT here — it lives on /api/auth/status::github_mode.
  state.github = {
    login: "mock-user",
    scopes: ["repo", "read:org"],
    attribute_commits: body.attribute_commits ?? true,
    last_validated_at: new Date().toISOString(),
  };
  return jsonResponse(200, state);
}

export function mockDeleteGithubPat(): Response {
  // Deleting a PAT collapses the github sub-object to null (matches the
  // backend's Option<...> wire shape).
  state.github = null;
  return jsonResponse(200, state);
}

export function mockPatchGithubSettings(
  body: PatchGithubSettingsRequest,
): Response {
  if (!state.github) {
    // PATCHing the toggle without a stored PAT is a 404 in the real
    // backend (the row doesn't exist yet) — surface that here too.
    return jsonResponse(404, {
      error: "not_found",
      message: "No GitHub PAT to update.",
    });
  }
  state.github.attribute_commits = body.attribute_commits;
  return jsonResponse(200, state);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function takeFailure(): ForcedFailure | null {
  const f = forcedFailure;
  forcedFailure = null;
  return f;
}

function failureResponse(f: ForcedFailure): Response {
  switch (f.kind) {
    case "sso_required":
      return jsonResponse(403, {
        error: "sso_authorization_required",
        message: `Authorize SSO for this org: ${f.orgUrl}`,
        org_sso_url: f.orgUrl,
      });
    case "invalid_token":
      return jsonResponse(400, {
        error: "invalid_token",
        message: f.message,
      });
    case "http_403":
      return jsonResponse(403, {
        error: "forbidden",
        message: f.message,
      });
  }
}

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

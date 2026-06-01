// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  api,
  apiJson,
  apiPost,
  apiPostJson,
  addRepository,
  AgentConfigError,
  deleteGithubPat,
  deleteProviderCredential,
  fetchOnboardingStatus,
  fetchUserCredentials,
  listMyRepositories,
  listAvailableRepositories,
  patchGithubSettings,
  putAgentConfig,
  removeRepository,
  setGithubPat,
  setProviderCredential,
  UserCredentialsError,
} from "./client";
import { clearMocksOverride, setMocksEnabled } from "./mocks";

// Stub window.location for the 401 redirect logic
const originalLocation = window.location;

beforeEach(() => {
  Object.defineProperty(window, "location", {
    writable: true,
    value: { ...originalLocation, pathname: "/", search: "", href: "" },
  });
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.defineProperty(window, "location", { writable: true, value: originalLocation });
});

function mockFetch(status: number, body?: unknown, ok?: boolean) {
  // 204/205/304 must have a null body per the Fetch spec.
  const hasBody = body !== undefined && status !== 204 && status !== 205 && status !== 304;
  const res = new Response(hasBody ? JSON.stringify(body) : null, {
    status,
    headers: { "Content-Type": "application/json" },
  });
  Object.defineProperty(res, "ok", { value: ok ?? (status >= 200 && status < 300) });
  (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);
  return res;
}

describe("api()", () => {
  it("calls fetch with credentials: same-origin", async () => {
    mockFetch(200, { ok: true });
    await api("/api/health");
    expect(fetch).toHaveBeenCalledWith("/api/health", { credentials: "same-origin" });
  });

  it("passes through RequestInit options", async () => {
    mockFetch(200);
    await api("/api/foo", { method: "DELETE" });
    expect(fetch).toHaveBeenCalledWith("/api/foo", {
      method: "DELETE",
      credentials: "same-origin",
    });
  });

  it("redirects to login on 401", async () => {
    mockFetch(401);
    await api("/api/work-items");
    expect(window.location.href).toContain("/login.html?return=");
  });
});

describe("apiJson()", () => {
  it("parses JSON on success", async () => {
    mockFetch(200, { paused: true });
    const data = await apiJson<{ paused: boolean }>("/api/polling");
    expect(data).toEqual({ paused: true });
  });

  it("throws on non-2xx", async () => {
    const res = new Response("bad request", { status: 400 });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);
    await expect(apiJson("/api/bad")).rejects.toThrow("bad request");
  });

  it("throws generic message when body is empty", async () => {
    const res = new Response("", { status: 500 });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);
    await expect(apiJson("/api/fail")).rejects.toThrow("HTTP 500");
  });
});

describe("apiPost()", () => {
  it("sends POST with JSON body", async () => {
    mockFetch(200);
    await apiPost("/api/work-items/start-manual", { ticket_key: "TEST-1" });
    expect(fetch).toHaveBeenCalledWith("/api/work-items/start-manual", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ ticket_key: "TEST-1" }),
      credentials: "same-origin",
    });
  });

  it("sends POST without body when none given", async () => {
    mockFetch(200);
    await apiPost("/api/polling/pause");
    expect(fetch).toHaveBeenCalledWith("/api/polling/pause", {
      method: "POST",
      headers: undefined,
      body: undefined,
      credentials: "same-origin",
    });
  });
});

describe("apiPostJson()", () => {
  it("parses JSON response", async () => {
    mockFetch(200, { workflow_id: "abc" });
    const data = await apiPostJson<{ workflow_id: string }>("/api/start", { key: "T-1" });
    expect(data).toEqual({ workflow_id: "abc" });
  });

  it("throws on error response", async () => {
    const res = new Response("conflict", { status: 409 });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);
    await expect(apiPostJson("/api/start", {})).rejects.toThrow("conflict");
  });
});

// ---------------------------------------------------------------------------
// Per-user credentials (Phase 2 — auth-overhaul).
// All tests below force `clearMocksOverride()` first so a previous test that
// flipped the runtime override doesn't bleed into the next.
// ---------------------------------------------------------------------------

describe("per-user credentials client", () => {
  beforeEach(() => {
    clearMocksOverride();
  });

  it("fetchUserCredentials parses a 200 body", async () => {
    // Wire shape mirrors routes/credentials.rs::ProviderCredentialStatus.
    const status = {
      provider: {
        provider: "claude",
        kind: "api_key",
        active: true,
        last_validated_at: "2026-01-01T00:00:00Z",
        last_used_at: null,
      },
      // Wire shape: no `has_pat` / `mode` on the per-user response; a
      // missing PAT is represented as `github: null` (Option<>).
      github: null,
    };
    mockFetch(200, status);
    const got = await fetchUserCredentials();
    expect(fetch).toHaveBeenCalledWith("/api/users/me/credentials", {
      credentials: "same-origin",
    });
    expect(got).toEqual(status);
  });

  it("fetchUserCredentials throws UserCredentialsError on 401", async () => {
    const res = new Response(JSON.stringify({ error: "unauthorized" }), {
      status: 401,
      headers: { "Content-Type": "application/json" },
    });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);
    let caught: unknown;
    try {
      await fetchUserCredentials();
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(UserCredentialsError);
    expect((caught as UserCredentialsError).code).toBe("unauthorized");
    expect((caught as UserCredentialsError).status).toBe(401);
  });

  it("setProviderCredential POSTs the api_key body to the encoded provider URL", async () => {
    mockFetch(204);
    await setProviderCredential("cursor", { api_key: "key_xyz" });
    expect(fetch).toHaveBeenCalledWith("/api/users/me/credentials/cursor", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ api_key: "key_xyz" }),
      credentials: "same-origin",
    });
  });

  it("setProviderCredential surfaces the structured `invalid_token` error on 400", async () => {
    const res = new Response(
      JSON.stringify({ error: "invalid_token", message: "rejected by Claude" }),
      { status: 400, headers: { "Content-Type": "application/json" } },
    );
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);
    let caught: unknown;
    try {
      await setProviderCredential("claude", { api_key: "bad" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(UserCredentialsError);
    const err = caught as UserCredentialsError;
    expect(err.code).toBe("invalid_token");
    expect(err.message).toBe("rejected by Claude");
  });

  it("deleteProviderCredential DELETEs the encoded provider URL", async () => {
    mockFetch(204);
    await deleteProviderCredential("claude");
    expect(fetch).toHaveBeenCalledWith("/api/users/me/credentials/claude", {
      method: "DELETE",
      credentials: "same-origin",
    });
  });

  // ── Task #40 — Claude `kind=cli_state` path. ──

  it("setProviderCredential threads kind + claude_session_json into the POST body", async () => {
    mockFetch(204);
    const blob = JSON.stringify({
      oauthAccount: {
        accountUuid: "a",
        emailAddress: "b@c.d",
        organizationUuid: "o",
      },
    });
    await setProviderCredential("claude", {
      kind: "cli_state",
      claude_session_json: blob,
    });
    expect(fetch).toHaveBeenCalledWith("/api/users/me/credentials/claude", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ kind: "cli_state", claude_session_json: blob }),
      credentials: "same-origin",
    });
  });

  it("setClaudeSession is a thin wrapper that always posts kind=cli_state to /claude", async () => {
    const { setClaudeSession } = await import("./client");
    mockFetch(204);
    const blob = "{}";
    await setClaudeSession(blob);
    expect(fetch).toHaveBeenCalledWith("/api/users/me/credentials/claude", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ kind: "cli_state", claude_session_json: blob }),
      credentials: "same-origin",
    });
  });

  it("deleteProviderCredential appends ?kind= when a kind is supplied", async () => {
    mockFetch(204);
    await deleteProviderCredential("claude", "cli_state");
    expect(fetch).toHaveBeenCalledWith(
      "/api/users/me/credentials/claude?kind=cli_state",
      { method: "DELETE", credentials: "same-origin" },
    );
  });

  it("deleteProviderCredential URL-encodes the kind value", async () => {
    mockFetch(204);
    await deleteProviderCredential("claude", "api_key");
    // `api_key` happens to have no encodable characters, but the wrapper
    // still runs it through encodeURIComponent. Pin the literal so a
    // future refactor that swaps to template-literal concatenation still
    // does the right thing.
    expect(fetch).toHaveBeenCalledWith(
      "/api/users/me/credentials/claude?kind=api_key",
      { method: "DELETE", credentials: "same-origin" },
    );
  });

  it("setGithubPat threads attribute_commits through the body", async () => {
    // Wire shape: github sub-object has only { login, scopes,
    // attribute_commits, last_validated_at } — no `has_pat`, no `mode`.
    const status = {
      provider: null,
      github: {
        login: "alice",
        scopes: ["repo"],
        attribute_commits: false,
        last_validated_at: "2026-05-19T08:00:00Z",
      },
    };
    mockFetch(200, status);
    const got = await setGithubPat({ pat: "ghp_xyz", attribute_commits: false });
    expect(fetch).toHaveBeenCalledWith("/api/users/me/github-pat", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ pat: "ghp_xyz", attribute_commits: false }),
      credentials: "same-origin",
    });
    expect(got).toEqual(status);
  });

  it("setGithubPat surfaces sso_authorization_required with orgSsoUrl", async () => {
    const body = {
      error: "sso_authorization_required",
      message: "Authorize SSO",
      org_sso_url: "https://github.com/orgs/acme/sso",
    };
    const res = new Response(JSON.stringify(body), {
      status: 403,
      headers: { "Content-Type": "application/json" },
    });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);
    let caught: unknown;
    try {
      await setGithubPat({ pat: "ghp_xyz" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(UserCredentialsError);
    const err = caught as UserCredentialsError;
    expect(err.code).toBe("sso_authorization_required");
    expect(err.status).toBe(403);
    expect(err.orgSsoUrl).toBe("https://github.com/orgs/acme/sso");
  });

  it("deleteGithubPat DELETEs /api/users/me/github-pat and returns the fresh status", async () => {
    // After delete, the github row is gone — wire shape uses null, not an
    // object with `has_pat: false` (#29 alignment).
    const status = {
      provider: null,
      github: null,
    };
    mockFetch(200, status);
    const got = await deleteGithubPat();
    expect(fetch).toHaveBeenCalledWith("/api/users/me/github-pat", {
      method: "DELETE",
      credentials: "same-origin",
    });
    expect(got).toEqual(status);
  });

  it("patchGithubSettings PATCHes with attribute_commits (A3 rename guard)", async () => {
    const status = {
      provider: null,
      github: {
        login: "alice",
        scopes: ["repo"],
        attribute_commits: false,
        last_validated_at: "2026-05-19T08:00:00Z",
      },
    };
    mockFetch(200, status);
    await patchGithubSettings({ attribute_commits: false });
    expect(fetch).toHaveBeenCalledWith("/api/users/me/github", {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ attribute_commits: false }),
      credentials: "same-origin",
    });
  });

  it("routes through the mock layer when isMocksEnabled() returns true", async () => {
    setMocksEnabled(true);
    const got = await fetchUserCredentials();
    // Real fetch must NOT have been called — mock layer answered.
    expect(fetch).not.toHaveBeenCalled();
    // The fresh mock state has no PAT — `github` is null per the new
    // wire shape (#29).
    expect(got.github).toBeNull();
    clearMocksOverride();
  });
});

// ---------------------------------------------------------------------------
// Agent config patch (Phase 1 — auth-overhaul).
// ---------------------------------------------------------------------------

describe("putAgentConfig()", () => {
  const patch = {
    provider: "claude" as const,
    providers: {
      claude: { model: "claude-3-5-sonnet-latest", base_url: "", extra_args: [], allow_shared_default: false },
    },
  };

  it("PUTs the patch and returns the parsed ConfigResponse on 200", async () => {
    const updated = {
      general: { ticketing_system: "none" },
      agent: { provider: "claude" },
    };
    mockFetch(200, updated);
    const got = await putAgentConfig(patch);
    expect(fetch).toHaveBeenCalledWith("/api/config/agent", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
      credentials: "same-origin",
    });
    expect(got).toEqual(updated);
  });

  it("throws AgentConfigError with structured code on 400", async () => {
    const body = { error: "denied_extra_arg", message: "--resume is Maestro-owned" };
    const res = new Response(JSON.stringify(body), { status: 400 });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);
    let caught: unknown;
    try {
      await putAgentConfig(patch);
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(AgentConfigError);
    const err = caught as AgentConfigError;
    expect(err.code).toBe("denied_extra_arg");
    expect(err.status).toBe(400);
    expect(err.message).toBe("--resume is Maestro-owned");
  });

  it("throws AgentConfigError on 403 with no JSON body (falls back to http_403 code)", async () => {
    const res = new Response("forbidden", { status: 403 });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);
    let caught: unknown;
    try {
      await putAgentConfig(patch);
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(AgentConfigError);
    const err = caught as AgentConfigError;
    expect(err.code).toBe("http_403");
    expect(err.status).toBe(403);
    expect(err.message).toBe("forbidden");
  });
});

// ---------------------------------------------------------------------------
// Onboarding status (Phase 0 — auth-overhaul).
// ---------------------------------------------------------------------------

describe("fetchOnboardingStatus()", () => {
  it("returns the parsed SystemStatus on 200", async () => {
    const status = {
      config_toml_ok: true,
      github: { mode: "app", app_configured: true, app_id: 1, app_name: "x" },
      provider: {
        selected: "claude",
        deployment_default_credential_present: true,
        headless_capable: true,
        custom_base_url: null,
      },
      ticketing: { system: "jira", acli_ok: true },
      per_user_required: true,
      warnings: [],
    };
    mockFetch(200, status);
    const got = await fetchOnboardingStatus();
    expect(fetch).toHaveBeenCalledWith("/api/onboarding/status", {
      credentials: "same-origin",
    });
    expect(got).toEqual(status);
  });

  it("returns null when the server has no such endpoint (404)", async () => {
    mockFetch(404, "not found");
    const got = await fetchOnboardingStatus();
    expect(got).toBeNull();
  });

  it("throws on other non-2xx responses", async () => {
    const res = new Response("boom", { status: 500 });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);
    await expect(fetchOnboardingStatus()).rejects.toThrow("boom");
  });
});

// ---------------------------------------------------------------------------
// Plan-10 repository wrappers.
// ---------------------------------------------------------------------------

describe("repository API wrappers", () => {
  it("listMyRepositories hits GET /api/repositories", async () => {
    mockFetch(200, [
      { id: "r1", name: "maestro-core", repo_url: "https://github.com/x/y", local_path: "/workspaces/maestro-core", default_branch: "main", added_at: 1 },
    ]);
    const rows = await listMyRepositories();
    expect(fetch).toHaveBeenCalledWith("/api/repositories", { credentials: "same-origin" });
    expect(rows).toHaveLength(1);
    expect(rows[0].name).toBe("maestro-core");
  });

  it("listAvailableRepositories hits GET /api/repositories/_available", async () => {
    mockFetch(200, []);
    await listAvailableRepositories();
    expect(fetch).toHaveBeenCalledWith("/api/repositories/_available", { credentials: "same-origin" });
  });

  it("addRepository POSTs body and returns row", async () => {
    mockFetch(200, { id: "r2", name: "foo", repo_url: null, local_path: "/workspaces/foo", default_branch: "main" });
    const row = await addRepository({ repository_id: "r2" });
    expect(fetch).toHaveBeenCalledWith("/api/repositories", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ repository_id: "r2" }),
      credentials: "same-origin",
    });
    expect(row.id).toBe("r2");
  });

  it("addRepository throws with body text on error", async () => {
    const res = new Response("clone already in progress", { status: 409 });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);
    await expect(addRepository({ repo_url: "https://github.com/x/y" })).rejects.toThrow(
      "clone already in progress",
    );
  });

  it("removeRepository sends DELETE without body by default", async () => {
    mockFetch(204);
    await removeRepository("r3");
    expect(fetch).toHaveBeenCalledWith("/api/repositories/r3", {
      method: "DELETE",
      headers: undefined,
      body: undefined,
      credentials: "same-origin",
    });
  });

  it("removeRepository sends force_purge body when admin asks for it", async () => {
    mockFetch(204);
    await removeRepository("r3", { force_purge: true });
    expect(fetch).toHaveBeenCalledWith("/api/repositories/r3", {
      method: "DELETE",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ force_purge: true }),
      credentials: "same-origin",
    });
  });

  it("removeRepository url-encodes the id", async () => {
    mockFetch(204);
    await removeRepository("abc/def");
    const call = (fetch as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(call).toBe("/api/repositories/abc%2Fdef");
  });

  it("removeRepository surfaces 409 conflict text", async () => {
    const res = new Response("active workflow blocks removal", { status: 409 });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);
    await expect(removeRepository("r3")).rejects.toThrow("active workflow blocks removal");
  });
});

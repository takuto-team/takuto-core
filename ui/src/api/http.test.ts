// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * The shared fetch wrapper intercepts the per-user Jira credential-invalid 401
 * (JSON `{ code: "jira_credential_invalid" }`): it fires the global
 * `onJiraAuthFailure` event and does NOT redirect to login. Every other 401 is
 * a genuine auth failure → login redirect. No other status (400 no-keys, 403
 * not-in-project, 5xx, 2xx) triggers the Jira modal.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { api } from "./http";
import { onJiraAuthFailure } from "./jiraAuthFailure";

function jsonRes(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

let calls: number;
let unsubscribe: () => void;
let originalLocation: Location;

beforeEach(() => {
  calls = 0;
  unsubscribe = onJiraAuthFailure(() => {
    calls += 1;
  });
  originalLocation = window.location;
  // Redefine location so the genuine-401 redirect just records href instead of
  // attempting a (jsdom-unimplemented) navigation.
  Object.defineProperty(window, "location", {
    configurable: true,
    writable: true,
    value: { pathname: "/", search: "", href: "" },
  });
});

afterEach(() => {
  unsubscribe();
  Object.defineProperty(window, "location", {
    configurable: true,
    writable: true,
    value: originalLocation,
  });
  vi.restoreAllMocks();
});

describe("api() — Jira credential-invalid interception", () => {
  it("emits a Jira auth-failure and does NOT redirect on 401 + jira_credential_invalid", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => jsonRes({ code: "jira_credential_invalid", message: "bad token" }, 401)),
    );
    const res = await api("/api/jira/todo-tickets-manual?repository=acme");
    expect(calls).toBe(1);
    expect(res.status).toBe(401);
    expect(window.location.href).toBe(""); // no login redirect
  });

  it("leaves the response body readable by the caller (clone peek)", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => jsonRes({ code: "jira_credential_invalid", message: "bad token" }, 401)),
    );
    const res = await api("/api/jira/tickets/PROJ-1/preview?repository=acme");
    const body = (await res.json()) as { code: string };
    expect(body.code).toBe("jira_credential_invalid");
  });

  it("does NOT emit on a genuine 401 — it redirects to login instead", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => jsonRes({ error: "unauthorized" }, 401)));
    await api("/api/workflows");
    expect(calls).toBe(0);
    expect(window.location.href).toContain("/login.html");
  });

  it("does NOT emit on the 400 no-project-keys error", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => jsonRes({ error: "No Jira project keys configured for this repository" }, 400)),
    );
    const res = await api("/api/jira/todo-tickets-manual?repository=acme");
    expect(calls).toBe(0);
    expect(res.status).toBe(400);
    expect(window.location.href).toBe("");
  });

  it("does NOT emit on the 403 ticket-not-in-project error", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => jsonRes({ error: "ticket's project prefix not in configured" }, 403)),
    );
    await api("/api/jira/tickets/OTHER-9/preview?repository=acme");
    expect(calls).toBe(0);
  });

  it("does NOT emit on a successful response", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => jsonRes({ ok: true }, 200)));
    await api("/api/config");
    expect(calls).toBe(0);
  });
});

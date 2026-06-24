// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createElement, type ReactNode } from "react";
import { renderHook, act, waitFor } from "@testing-library/react";
import { useTicketingForm } from "./useTicketingForm";
import { ToastProvider } from "./useToast";

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  vi.restoreAllMocks();
});

/**
 * Default fetch stub: the mount-time `GET /api/users/me/credentials` returns
 * `jira: <jiraStatus>`; `PUT /api/config` and the Jira POST return 200. Tests
 * inspect the recorded calls.
 */
function stubFetch(jiraStatus: unknown = null) {
  (fetch as ReturnType<typeof vi.fn>).mockImplementation(async (url: string) => {
    if (url === "/api/users/me/credentials") {
      return new Response(
        JSON.stringify({ provider: null, github: null, jira: jiraStatus }),
        { status: 200 },
      );
    }
    if (url === "/api/users/me/jira-credential") {
      return new Response(
        JSON.stringify({
          site: "https://acme.atlassian.net",
          email: "dev@acme.com",
          account: { account_id: "a1", display_name: "Dev" },
        }),
        { status: 200 },
      );
    }
    // PUT /api/config and anything else
    return new Response("{}", { status: 200 });
  });
}

const wrapper = ({ children }: { children: ReactNode }) =>
  createElement(ToastProvider, null, children);

function callsTo(url: string) {
  return (fetch as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === url);
}

describe("useTicketingForm", () => {
  it("seeds the selector from the persisted system once config is ready", async () => {
    stubFetch();
    const { result, rerender } = renderHook(
      ({ ready }: { ready: boolean }) =>
        useTicketingForm({ initialSystem: "github", ready }),
      { wrapper, initialProps: { ready: false } },
    );
    expect(result.current.system).toBe("none");
    rerender({ ready: true });
    expect(result.current.system).toBe("github");
  });

  it("save() writes ticketing_system via PUT /api/config", async () => {
    stubFetch();
    const { result } = renderHook(
      () => useTicketingForm({ initialSystem: "none", ready: true }),
      { wrapper },
    );
    act(() => result.current.setSystem("github"));

    let ok = false;
    await act(async () => {
      ok = await result.current.save();
    });

    expect(ok).toBe(true);
    const cfg = callsTo("/api/config");
    expect(cfg).toHaveLength(1);
    expect(cfg[0][1].method).toBe("PUT");
    expect(JSON.parse(cfg[0][1].body)).toEqual({
      general: { ticketing_system: "github" },
    });
    expect(callsTo("/api/users/me/jira-credential")).toHaveLength(0);
  });

  it("save() posts the Jira credential when Jira is selected and fields are complete", async () => {
    stubFetch();
    const { result } = renderHook(
      () => useTicketingForm({ initialSystem: "none", ready: true }),
      { wrapper },
    );
    act(() => {
      result.current.setSystem("jira");
      result.current.setSite("https://acme.atlassian.net");
      result.current.setEmail("dev@acme.com");
      result.current.setToken("tok-123");
    });

    let ok = false;
    await act(async () => {
      ok = await result.current.save();
    });

    expect(ok).toBe(true);
    expect(callsTo("/api/config")).toHaveLength(1);
    const jira = callsTo("/api/users/me/jira-credential");
    expect(jira).toHaveLength(1);
    expect(JSON.parse(jira[0][1].body)).toEqual({
      site: "https://acme.atlassian.net",
      email: "dev@acme.com",
      token: "tok-123",
    });
  });

  it("save() blocks on a half-filled Jira form without writing config", async () => {
    stubFetch();
    const { result } = renderHook(
      () => useTicketingForm({ initialSystem: "none", ready: true }),
      { wrapper },
    );
    act(() => {
      result.current.setSystem("jira");
      result.current.setSite("https://acme.atlassian.net");
      // email + token left blank
    });

    let ok = true;
    await act(async () => {
      ok = await result.current.save();
    });

    expect(ok).toBe(false);
    expect(callsTo("/api/config")).toHaveLength(0);
    expect(callsTo("/api/users/me/jira-credential")).toHaveLength(0);
  });

  it("an already-connected user can save with a blank form without any writes", async () => {
    stubFetch({
      site: "https://acme.atlassian.net",
      email: "dev@acme.com",
      account_id: "a1",
      account_name: "Dev",
      last_validated_at: null,
    });
    const { result } = renderHook(
      () => useTicketingForm({ initialSystem: "jira", ready: true }),
      { wrapper },
    );
    // Wait for the mount-time credentials fetch to land.
    await waitFor(() => expect(result.current.connected).not.toBeNull());

    let ok = false;
    await act(async () => {
      ok = await result.current.save();
    });

    expect(ok).toBe(true);
    // System unchanged and no token entered → no config PUT, no credential POST.
    expect(callsTo("/api/config")).toHaveLength(0);
    expect(callsTo("/api/users/me/jira-credential")).toHaveLength(0);
  });

  it("rotates the Jira credential without writing config when the system is unchanged", async () => {
    stubFetch();
    const { result } = renderHook(
      () => useTicketingForm({ initialSystem: "jira", ready: true }),
      { wrapper },
    );
    // System stays "jira" (unchanged) — only a new token is entered. This is
    // the non-admin "manage my own credential" path: it must NOT hit the
    // admin-gated PUT /api/config.
    act(() => {
      result.current.setSite("https://acme.atlassian.net");
      result.current.setEmail("dev@acme.com");
      result.current.setToken("rotated-token");
    });

    let ok = false;
    await act(async () => {
      ok = await result.current.save();
    });

    expect(ok).toBe(true);
    expect(callsTo("/api/config")).toHaveLength(0);
    expect(callsTo("/api/users/me/jira-credential")).toHaveLength(1);
  });

  it("pre-fills site/email and, on a non-secret change, saves with the token OMITTED (KEEP)", async () => {
    stubFetch({
      site: "https://acme.atlassian.net",
      email: "dev@acme.com",
      account_id: "a1",
      account_name: "Dev",
      last_validated_at: null,
    });
    const { result } = renderHook(
      () => useTicketingForm({ initialSystem: "jira", ready: true }),
      { wrapper },
    );
    // site/email pre-fill from the stored credential once it loads.
    await waitFor(() => expect(result.current.site).toBe("https://acme.atlassian.net"));
    expect(result.current.email).toBe("dev@acme.com");

    // Change a non-secret (email), leave the token masked/blank.
    act(() => result.current.setEmail("dev2@acme.com"));

    let ok = false;
    await act(async () => {
      ok = await result.current.save();
    });

    expect(ok).toBe(true);
    const jira = callsTo("/api/users/me/jira-credential");
    expect(jira).toHaveLength(1);
    const body = JSON.parse(jira[0][1].body);
    // KEEP: the token is omitted entirely (never an empty string, never a sentinel).
    expect(body).toEqual({ site: "https://acme.atlassian.net", email: "dev2@acme.com" });
    expect("token" in body).toBe(false);
  });

  it("sends the token when a connected user replaces it (REPLACE)", async () => {
    stubFetch({
      site: "https://acme.atlassian.net",
      email: "dev@acme.com",
      account_id: "a1",
      account_name: "Dev",
      last_validated_at: null,
    });
    const { result } = renderHook(
      () => useTicketingForm({ initialSystem: "jira", ready: true }),
      { wrapper },
    );
    await waitFor(() => expect(result.current.site).toBe("https://acme.atlassian.net"));

    act(() => result.current.setToken("rotated-token"));

    let ok = false;
    await act(async () => {
      ok = await result.current.save();
    });

    expect(ok).toBe(true);
    const jira = callsTo("/api/users/me/jira-credential");
    expect(jira).toHaveLength(1);
    expect(JSON.parse(jira[0][1].body)).toEqual({
      site: "https://acme.atlassian.net",
      email: "dev@acme.com",
      token: "rotated-token",
    });
  });

  it("disconnect() deletes the per-user Jira credential", async () => {
    stubFetch({
      site: "https://acme.atlassian.net",
      email: "dev@acme.com",
      account_id: "a1",
      account_name: "Dev",
      last_validated_at: null,
    });
    const { result } = renderHook(
      () => useTicketingForm({ initialSystem: "jira", ready: true }),
      { wrapper },
    );
    await waitFor(() => expect(result.current.connected).not.toBeNull());

    let ok = false;
    await act(async () => {
      ok = await result.current.disconnect();
    });

    expect(ok).toBe(true);
    const del = (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(
      (c) => c[0] === "/api/users/me/jira-credential" && (c[1]?.method ?? "GET") === "DELETE",
    );
    expect(del).toHaveLength(1);
  });
});

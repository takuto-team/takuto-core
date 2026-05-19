// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useAuth } from "./useAuth";

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  vi.restoreAllMocks();
});

function mockFetchImpl(impl: (url: string, init?: RequestInit) => Promise<Response>) {
  (fetch as ReturnType<typeof vi.fn>).mockImplementation(impl);
}

describe("useAuth", () => {
  it("sets authEnabled=true and checks session when auth is enabled", async () => {
    mockFetchImpl(async (url: string) => {
      if (url === "/api/auth/status") {
        return new Response(JSON.stringify({ dashboard_auth_enabled: true }), { status: 200 });
      }
      if (url === "/api/config") {
        return new Response("{}", { status: 200 });
      }
      return new Response("", { status: 404 });
    });

    const { result } = renderHook(() => useAuth());

    await vi.waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.authEnabled).toBe(true);
    expect(result.current.loggedIn).toBe(true);
  });

  it("sets loggedIn=false when session cookie is invalid", async () => {
    mockFetchImpl(async (url: string) => {
      if (url === "/api/auth/status") {
        return new Response(JSON.stringify({ dashboard_auth_enabled: true }), { status: 200 });
      }
      if (url === "/api/config") {
        const res = new Response("", { status: 401 });
        Object.defineProperty(res, "ok", { value: false });
        return res;
      }
      return new Response("", { status: 404 });
    });

    const { result } = renderHook(() => useAuth());

    await vi.waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.authEnabled).toBe(true);
    expect(result.current.loggedIn).toBe(false);
  });

  it("sets loggedIn=true when auth is disabled", async () => {
    mockFetchImpl(async (url: string) => {
      if (url === "/api/auth/status") {
        return new Response(JSON.stringify({ dashboard_auth_enabled: false }), { status: 200 });
      }
      return new Response("", { status: 404 });
    });

    const { result } = renderHook(() => useAuth());

    await vi.waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.authEnabled).toBe(false);
    expect(result.current.loggedIn).toBe(true);
  });

  it("login() calls POST /api/auth/login and sets loggedIn on success", async () => {
    mockFetchImpl(async (url: string, init?: RequestInit) => {
      if (url === "/api/auth/status") {
        return new Response(JSON.stringify({ dashboard_auth_enabled: true }), { status: 200 });
      }
      if (url === "/api/config") {
        const res = new Response("", { status: 401 });
        Object.defineProperty(res, "ok", { value: false });
        return res;
      }
      if (url === "/api/auth/login" && init?.method === "POST") {
        return new Response(null, { status: 204 });
      }
      return new Response("", { status: 404 });
    });

    const { result } = renderHook(() => useAuth());

    await vi.waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.loggedIn).toBe(false);

    let success: boolean | undefined;
    await act(async () => {
      success = await result.current.login("admin", "password");
    });

    expect(success).toBe(true);
    expect(result.current.loggedIn).toBe(true);
    // Verify fetch was called with the right args
    expect(fetch).toHaveBeenCalledWith("/api/auth/login", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ username: "admin", password: "password" }),
    }));
  });

  it("loading stays true until /api/auth/me has resolved (#34 race fix)", async () => {
    // Regression for the race that bounced admins to "/" on direct
    // /admin/ai load. Before the fix, checkAuth's `.finally(setLoading
    // false)` ran while fetchMe was still in flight, so the App rendered
    // Routes with currentUser=null. By the time the route element checked
    // `currentUser?.role === "admin"`, the value was null → Navigate to "/".
    //
    // This test simulates a slow /api/auth/me and verifies that
    // `loading=false` and `currentUser` flip in the same tick (or that
    // loading stays true until currentUser arrives).
    let resolveMe: ((res: Response) => void) | null = null;
    mockFetchImpl(async (url: string) => {
      if (url === "/api/auth/status") {
        return new Response(
          JSON.stringify({ dashboard_auth_enabled: true }),
          { status: 200 },
        );
      }
      if (url === "/api/config") {
        return new Response("{}", { status: 200 });
      }
      if (url === "/api/auth/me") {
        // Block /api/auth/me until we call resolveMe() below.
        return new Promise<Response>((r) => {
          resolveMe = r;
        });
      }
      return new Response("", { status: 404 });
    });

    const { result } = renderHook(() => useAuth());

    // Give checkAuth time to fire the /api/auth/status + /api/config
    // fetches and reach the fetchMe() call. We loop until fetchMe has
    // registered its pending promise.
    await vi.waitFor(() => {
      expect(resolveMe).not.toBeNull();
    });

    // At this point /api/auth/status + /api/config have resolved. The
    // pre-fix code would have flipped `loading` to false RIGHT HERE,
    // before fetchMe completes. With the fix, loading must still be true.
    expect(result.current.loading).toBe(true);
    expect(result.current.currentUser).toBeNull();

    // Now resolve fetchMe with an admin user.
    act(() => {
      resolveMe!(
        new Response(
          JSON.stringify({ username: "alice", role: "admin" }),
          { status: 200 },
        ),
      );
    });

    // After fetchMe resolves, loading flips false AND currentUser is
    // populated. A consumer reading both in the same render gets a
    // consistent snapshot — no more `loading=false + currentUser=null` gap.
    await vi.waitFor(() => {
      expect(result.current.loading).toBe(false);
    });
    expect(result.current.currentUser).toEqual({
      username: "alice",
      role: "admin",
    });
  });

  it("logout() calls POST /api/auth/logout and sets loggedIn=false", async () => {
    mockFetchImpl(async (url: string) => {
      if (url === "/api/auth/status") {
        return new Response(JSON.stringify({ dashboard_auth_enabled: false }), { status: 200 });
      }
      if (url === "/api/auth/logout") {
        return new Response(null, { status: 204 });
      }
      return new Response("", { status: 404 });
    });

    const { result } = renderHook(() => useAuth());

    await vi.waitFor(() => {
      expect(result.current.loggedIn).toBe(true);
    });

    await act(async () => {
      await result.current.logout();
    });

    expect(result.current.loggedIn).toBe(false);
    expect(fetch).toHaveBeenCalledWith("/api/auth/logout", expect.objectContaining({
      method: "POST",
    }));
  });
});

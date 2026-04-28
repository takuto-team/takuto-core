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

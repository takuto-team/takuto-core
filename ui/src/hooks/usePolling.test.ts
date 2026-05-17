// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { usePolling } from "./usePolling";

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  vi.restoreAllMocks();
});

function mockFetchResponses(...responses: Array<{ status: number; body?: unknown; ok?: boolean }>) {
  const fn = fetch as ReturnType<typeof vi.fn>;
  for (const r of responses) {
    const res = new Response(r.body !== undefined ? JSON.stringify(r.body) : "", {
      status: r.status,
    });
    Object.defineProperty(res, "ok", { value: r.ok ?? (r.status >= 200 && r.status < 300) });
    fn.mockResolvedValueOnce(res);
  }
}

describe("usePolling", () => {
  it("fetches initial polling state", async () => {
    mockFetchResponses({ status: 200, body: { paused: true } });

    const { result } = renderHook(() => usePolling());

    // Wait for the useEffect to settle
    await vi.waitFor(() => {
      expect(result.current.paused).toBe(true);
    });
    expect(fetch).toHaveBeenCalledWith("/api/polling", { credentials: "same-origin" });
  });

  it("toggle() calls /api/polling/resume when paused", async () => {
    mockFetchResponses(
      { status: 200, body: { paused: true } },  // initial fetch
      { status: 200, ok: true }                   // toggle POST
    );

    const { result } = renderHook(() => usePolling());

    await vi.waitFor(() => {
      expect(result.current.paused).toBe(true);
    });

    await act(async () => {
      await result.current.toggle();
    });

    expect(fetch).toHaveBeenCalledWith("/api/polling/resume", {
      method: "POST",
      credentials: "same-origin",
    });
    expect(result.current.paused).toBe(false);
  });

  it("toggle() calls /api/polling/pause when not paused", async () => {
    (fetch as ReturnType<typeof vi.fn>).mockImplementation(async (url: string) => {
      if (url === "/api/polling") {
        return new Response(JSON.stringify({ paused: false }), { status: 200 });
      }
      return new Response(null, { status: 200 });
    });

    const { result } = renderHook(() => usePolling());

    // Flush the initial fetch promise chain
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });

    expect(result.current.paused).toBe(false);

    await act(async () => {
      await result.current.toggle();
    });

    expect(fetch).toHaveBeenCalledWith("/api/polling/pause", {
      method: "POST",
      credentials: "same-origin",
    });
    expect(result.current.paused).toBe(true);
  });

  it("sets toggling=true during toggle and false after", async () => {
    mockFetchResponses(
      { status: 200, body: { paused: false } },
      { status: 200, ok: true }
    );

    const { result } = renderHook(() => usePolling());

    await vi.waitFor(() => {
      expect(result.current.toggling).toBe(false);
    });

    await act(async () => {
      await result.current.toggle();
    });

    expect(result.current.toggling).toBe(false);
  });
});

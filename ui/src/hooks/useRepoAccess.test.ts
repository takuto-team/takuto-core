// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { useRepoAccess, isRepoAccessible } from "./useRepoAccess";

afterEach(() => vi.restoreAllMocks());

describe("isRepoAccessible", () => {
  it("treats null and absent/true entries as accessible, only false as not", () => {
    expect(isRepoAccessible({}, null)).toBe(true);
    expect(isRepoAccessible({}, "x")).toBe(true);
    expect(isRepoAccessible({ x: true }, "x")).toBe(true);
    expect(isRepoAccessible({ x: false }, "x")).toBe(false);
  });
});

describe("useRepoAccess", () => {
  beforeEach(() => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(
          JSON.stringify([
            { name: "ok", accessible: true },
            { name: "gone", accessible: false },
          ]),
          { status: 200, headers: { "Content-Type": "application/json" } },
        ),
      ),
    );
  });

  it("fetches on mount and maps name → accessible", async () => {
    const { result } = renderHook(() => useRepoAccess());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.access).toEqual({ ok: true, gone: false });
  });

  it("leaves the map empty (all accessible) on fetch error", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("boom", { status: 500 })));
    const { result } = renderHook(() => useRepoAccess());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.access).toEqual({});
    expect(isRepoAccessible(result.current.access, "anything")).toBe(true);
  });
});

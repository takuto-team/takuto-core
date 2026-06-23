// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * useTicketDetail — the Jira ticket-preview fetch must carry the repository as a
 * `?repository=` query param (the server resolves the caller's per-repo project
 * keys from it). The hook short-circuits for github / none ticketing systems and
 * when an initial description is already supplied.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { useTicketDetail } from "./useTicketDetail";

let fetchMock: ReturnType<typeof vi.fn>;

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

beforeEach(() => {
  fetchMock = vi.fn(async () => json({ description_markdown: "# Hello" }));
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("useTicketDetail — ?repository= on the Jira preview", () => {
  it("appends ?repository= when fetching a Jira ticket preview", async () => {
    const { result } = renderHook(() =>
      useTicketDetail("PROJ-1", undefined, "jira", "takuto-core"),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const url = String(fetchMock.mock.calls[0][0]);
    expect(url).toBe("/api/jira/tickets/PROJ-1/preview?repository=takuto-core");
    expect(result.current.markdown).toBe("# Hello");
  });

  it("URL-encodes the repository name", async () => {
    renderHook(() => useTicketDetail("PROJ-2", undefined, "jira", "my repo/x"));
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    const url = String(fetchMock.mock.calls[0][0]);
    expect(url).toContain("?repository=my%20repo%2Fx");
  });

  it("omits the query when no repository is known", async () => {
    renderHook(() => useTicketDetail("PROJ-3", undefined, "jira", null));
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    expect(String(fetchMock.mock.calls[0][0])).toBe("/api/jira/tickets/PROJ-3/preview");
  });

  it("does not hit the Jira preview endpoint in github mode (different endpoint family)", () => {
    renderHook(() => useTicketDetail("GH-1", undefined, "github", "takuto-core"));
    // The hook short-circuits github tickets; the caller fetches them elsewhere.
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("does not fetch when an initial description is already supplied", () => {
    const { result } = renderHook(() =>
      useTicketDetail("PROJ-9", "preloaded body", "jira", "takuto-core"),
    );
    expect(fetchMock).not.toHaveBeenCalled();
    expect(result.current.markdown).toBe("preloaded body");
    expect(result.current.loading).toBe(false);
  });
});

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { api, apiJson, apiPost, apiPostJson } from "./client";

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
  const res = new Response(body !== undefined ? JSON.stringify(body) : "", {
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
    await api("/api/workflows");
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
    await apiPost("/api/workflows/start-manual", { ticket_key: "TEST-1" });
    expect(fetch).toHaveBeenCalledWith("/api/workflows/start-manual", {
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

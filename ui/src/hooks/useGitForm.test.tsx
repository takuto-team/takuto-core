// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createElement, type ReactNode } from "react";
import { renderHook, act } from "@testing-library/react";
import { useGitForm } from "./useGitForm";
import { ToastProvider } from "./useToast";

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  vi.restoreAllMocks();
});

function stubFetch(status = 200) {
  (fetch as ReturnType<typeof vi.fn>).mockImplementation(async () => {
    const ok = status >= 200 && status < 300;
    const res = new Response(JSON.stringify({ git: {}, persisted: true }), { status });
    Object.defineProperty(res, "ok", { value: ok });
    return res;
  });
}

const wrapper = ({ children }: { children: ReactNode }) =>
  createElement(ToastProvider, null, children);

function callsTo(url: string) {
  return (fetch as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === url);
}

describe("useGitForm", () => {
  it("seeds base branch + remote from config once ready", () => {
    stubFetch();
    const { result, rerender } = renderHook(
      ({ ready }: { ready: boolean }) =>
        useGitForm({ initialBaseBranch: "develop", initialRemote: "upstream", ready }),
      { wrapper, initialProps: { ready: false } },
    );
    // Defaults before the config fetch resolves.
    expect(result.current.baseBranch).toBe("main");
    expect(result.current.remote).toBe("origin");
    rerender({ ready: true });
    expect(result.current.baseBranch).toBe("develop");
    expect(result.current.remote).toBe("upstream");
  });

  it("falls back to main/origin when config returns empty strings", () => {
    stubFetch();
    const { result } = renderHook(
      () => useGitForm({ initialBaseBranch: "", initialRemote: "", ready: true }),
      { wrapper },
    );
    expect(result.current.baseBranch).toBe("main");
    expect(result.current.remote).toBe("origin");
  });

  it("save() PUTs to /api/config/git and returns true", async () => {
    stubFetch();
    const { result } = renderHook(
      () => useGitForm({ initialBaseBranch: "main", initialRemote: "origin", ready: true }),
      { wrapper },
    );
    act(() => result.current.setBaseBranch("develop"));
    act(() => result.current.setRemote("upstream"));
    let ok: boolean | undefined;
    await act(async () => {
      ok = await result.current.save();
    });
    expect(ok).toBe(true);
    const calls = callsTo("/api/config/git");
    expect(calls).toHaveLength(1);
    expect(calls[0][1]).toMatchObject({
      method: "PUT",
      body: JSON.stringify({ base_branch: "develop", remote: "upstream" }),
    });
  });

  it("blocks save and makes no API call when base branch is blank", async () => {
    stubFetch();
    const { result } = renderHook(
      () => useGitForm({ initialBaseBranch: "main", initialRemote: "origin", ready: true }),
      { wrapper },
    );
    act(() => result.current.setBaseBranch("   "));
    expect(result.current.baseBranchInvalid).toBe(true);
    let ok: boolean | undefined;
    await act(async () => {
      ok = await result.current.save();
    });
    expect(ok).toBe(false);
    expect(callsTo("/api/config/git")).toHaveLength(0);
  });

  it("is a no-op (returns true, no API call) when canSave is false", async () => {
    stubFetch();
    const { result } = renderHook(
      () =>
        useGitForm({
          initialBaseBranch: "main",
          initialRemote: "origin",
          ready: true,
          canSave: false,
        }),
      { wrapper },
    );
    let ok: boolean | undefined;
    await act(async () => {
      ok = await result.current.save();
    });
    expect(ok).toBe(true);
    expect(callsTo("/api/config/git")).toHaveLength(0);
  });

  it("returns false on the admin-gated 403 (no throw)", async () => {
    stubFetch(403);
    const { result } = renderHook(
      () => useGitForm({ initialBaseBranch: "main", initialRemote: "origin", ready: true }),
      { wrapper },
    );
    let ok: boolean | undefined;
    await act(async () => {
      ok = await result.current.save();
    });
    expect(ok).toBe(false);
  });
});

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createElement, type ReactNode } from "react";
import { renderHook, act } from "@testing-library/react";
import { useStepTimeoutForm } from "./useStepTimeoutForm";
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
    const res = new Response(JSON.stringify({ agent: {}, persisted: true }), { status });
    Object.defineProperty(res, "ok", { value: ok });
    return res;
  });
}

const wrapper = ({ children }: { children: ReactNode }) =>
  createElement(ToastProvider, null, children);

function callsTo(url: string) {
  return (fetch as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === url);
}

describe("useStepTimeoutForm", () => {
  it("seeds from config once ready, defaulting to 1800", () => {
    stubFetch();
    const { result, rerender } = renderHook(
      ({ ready }: { ready: boolean }) =>
        useStepTimeoutForm({ initialSecs: 3600, ready }),
      { wrapper, initialProps: { ready: false } },
    );
    expect(result.current.value).toBe("1800");
    rerender({ ready: true });
    expect(result.current.value).toBe("3600");
  });

  it("save() PUTs step_timeout_secs to /api/config/agent", async () => {
    stubFetch();
    const { result } = renderHook(
      () => useStepTimeoutForm({ initialSecs: 1800, ready: true }),
      { wrapper },
    );
    act(() => result.current.setValue("3600"));
    let ok: boolean | undefined;
    await act(async () => {
      ok = await result.current.save();
    });
    expect(ok).toBe(true);
    const calls = callsTo("/api/config/agent");
    expect(calls).toHaveLength(1);
    expect(calls[0][1]).toMatchObject({
      method: "PUT",
      body: JSON.stringify({ step_timeout_secs: 3600 }),
    });
  });

  it("blocks save with no API call when the value is non-positive or blank", async () => {
    stubFetch();
    const { result } = renderHook(
      () => useStepTimeoutForm({ initialSecs: 1800, ready: true }),
      { wrapper },
    );
    act(() => result.current.setValue("0"));
    expect(result.current.invalid).toBe(true);
    let ok: boolean | undefined;
    await act(async () => {
      ok = await result.current.save();
    });
    expect(ok).toBe(false);

    act(() => result.current.setValue(""));
    expect(result.current.invalid).toBe(true);
    await act(async () => {
      ok = await result.current.save();
    });
    expect(ok).toBe(false);
    expect(callsTo("/api/config/agent")).toHaveLength(0);
  });
});

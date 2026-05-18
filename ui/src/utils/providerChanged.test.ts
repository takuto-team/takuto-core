// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi } from "vitest";
import { handleProviderChangedEvent } from "./providerChanged";
import type { WorkflowEvent } from "../api/types";

function makeEvent(over: Partial<WorkflowEvent> = {}): WorkflowEvent {
  return {
    event_type: "provider_changed",
    workflow_id: "",
    ticket_key: "",
    state: "",
    from: "claude",
    to: "cursor",
    ...over,
  };
}

describe("handleProviderChangedEvent", () => {
  it("shows an info toast with the from/to labels", () => {
    const showToast = vi.fn();
    const refreshOnboardingStatus = vi.fn();
    const fetchImpl = vi.fn(() =>
      Promise.resolve(new Response(null, { status: 200 })),
    );

    handleProviderChangedEvent(makeEvent(), {
      showToast,
      refreshOnboardingStatus,
      fetchImpl,
    });

    expect(showToast).toHaveBeenCalledTimes(1);
    const [message, type] = showToast.mock.calls[0];
    expect(message).toContain("claude");
    expect(message).toContain("cursor");
    expect(type).toBe("info");
  });

  it("refreshes /api/auth/status with same-origin credentials", () => {
    const fetchImpl = vi.fn(() =>
      Promise.resolve(new Response(null, { status: 200 })),
    );
    handleProviderChangedEvent(makeEvent(), {
      showToast: vi.fn(),
      refreshOnboardingStatus: vi.fn(),
      fetchImpl,
    });
    expect(fetchImpl).toHaveBeenCalledWith("/api/auth/status", {
      credentials: "same-origin",
    });
  });

  it("triggers refreshOnboardingStatus exactly once", () => {
    const refreshOnboardingStatus = vi.fn();
    handleProviderChangedEvent(makeEvent(), {
      showToast: vi.fn(),
      refreshOnboardingStatus,
      fetchImpl: vi.fn(() =>
        Promise.resolve(new Response(null, { status: 200 })),
      ),
    });
    expect(refreshOnboardingStatus).toHaveBeenCalledTimes(1);
  });

  it("falls back to placeholder labels when from/to are absent", () => {
    const showToast = vi.fn();
    handleProviderChangedEvent(
      makeEvent({ from: undefined, to: undefined }),
      {
        showToast,
        refreshOnboardingStatus: vi.fn(),
        fetchImpl: vi.fn(() =>
          Promise.resolve(new Response(null, { status: 200 })),
        ),
      },
    );
    const [message] = showToast.mock.calls[0];
    expect(message).toContain("previous provider");
    expect(message).toContain("new provider");
  });

  it("swallows fetch rejections so the WS handler stays sync", () => {
    const fetchImpl = vi.fn(() => Promise.reject(new Error("network down")));
    expect(() =>
      handleProviderChangedEvent(makeEvent(), {
        showToast: vi.fn(),
        refreshOnboardingStatus: vi.fn(),
        fetchImpl,
      }),
    ).not.toThrow();
  });
});

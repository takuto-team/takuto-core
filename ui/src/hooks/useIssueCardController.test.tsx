// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createElement, type ReactNode } from "react";
import { renderHook, act, waitFor } from "@testing-library/react";
import { useIssueCardController } from "./useIssueCardController";
import { ToastProvider } from "./useToast";

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn());
});
afterEach(() => {
  vi.restoreAllMocks();
});

const wrapper = ({ children }: { children: ReactNode }) =>
  createElement(ToastProvider, null, children);

/** Record POSTed paths; mark-done returns the given outcome, everything else 200. */
function stubFetch(markDoneOutcome: unknown) {
  const calls: string[] = [];
  (fetch as ReturnType<typeof vi.fn>).mockImplementation(async (url: string, init?: RequestInit) => {
    if (init?.method === "POST") calls.push(url);
    if (url.endsWith("/mark-done")) {
      return new Response(JSON.stringify(markDoneOutcome), { status: 200 });
    }
    return new Response("{}", { status: 200 });
  });
  return calls;
}

const didDelete = (calls: string[]) => calls.some((u) => u.endsWith("/delete"));

describe("useIssueCardController — mark-done-and-delete", () => {
  it("on a Jira transition failure: surfaces the reason and does NOT delete", async () => {
    const calls = stubFetch({
      jira_ok: false,
      worktree_ok: true,
      jira_error: 'no transition leads to status "Done". Available statuses: À faire, Terminé(e)',
    });
    const { result } = renderHook(() => useIssueCardController("KAN-1", () => {}, false), { wrapper });

    act(() => result.current.onMarkDoneAndDelete());

    await waitFor(() =>
      expect(result.current.markDoneError).toMatch(/Available statuses: À faire, Terminé\(e\)/),
    );
    expect(didDelete(calls)).toBe(false);
  });

  it("on success: mark-done removed the workflow, so it does NOT issue a redundant delete", async () => {
    // mark-done removes the worktree + dashboard item on full success; a
    // separate delete would 404 "Workflow not found".
    const calls = stubFetch({ jira_ok: true, worktree_ok: true, workflow_removed: true });
    const { result } = renderHook(() => useIssueCardController("KAN-1", () => {}, false), { wrapper });

    act(() => result.current.onMarkDoneAndDelete());

    await waitFor(() => expect(calls.some((u) => u.endsWith("/mark-done"))).toBe(true));
    expect(didDelete(calls)).toBe(false);
    expect(result.current.markDoneError).toBeNull();
  });

  it("succeeds but was not auto-removed (e.g. worktree hiccup): force-deletes", async () => {
    const calls = stubFetch({ jira_ok: true, worktree_ok: false, workflow_removed: false });
    const { result } = renderHook(() => useIssueCardController("KAN-1", () => {}, false), { wrapper });

    act(() => result.current.onMarkDoneAndDelete());

    await waitFor(() => expect(didDelete(calls)).toBe(true));
    expect(result.current.markDoneError).toBeNull();
  });

  it("dismissing the failure modal clears the error without deleting", async () => {
    const calls = stubFetch({ jira_ok: false, worktree_ok: true, jira_error: "bad" });
    const { result } = renderHook(() => useIssueCardController("KAN-1", () => {}, false), { wrapper });
    act(() => result.current.onMarkDoneAndDelete());
    await waitFor(() => expect(result.current.markDoneError).toBe("bad"));

    act(() => result.current.onMarkDoneErrorClose());
    expect(result.current.markDoneError).toBeNull();
    expect(didDelete(calls)).toBe(false);
  });
});

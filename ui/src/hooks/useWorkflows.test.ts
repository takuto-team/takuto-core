// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useWorkflows } from "./useWorkflows";
import { createQueryWrapper } from "../test/queryWrapper";
import type { WorkflowSummary } from "../api/types";

function makeWorkflow(overrides: Partial<WorkflowSummary> = {}): WorkflowSummary {
  return {
    id: "uuid-1",
    ticket_key: "TEST-1",
    ticket_summary: "Test ticket",
    ticket_description: "",
    ticket_type: "Story",
    state: "AddressingTicket",
    started_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:01:00Z",
    branch_name: "feat/test-1",
    pr_url: null,
    pr_merged: false,
    steps_log: [],
    error: null,
    terminal_lines: [],
    can_address_pr_comments: false,
    can_merge_base: false,
    can_mark_done: false,
    can_delete: false,
    can_start: false,
    progress_percent: 50,
    progress_steps_total: 4,
    started_manually: false,
    counts_toward_manual_cap: false,
    jira_browse_url: "",
    issue_url: null,
    can_open_editor: false,
    editor_url: null,
    editor_port_mappings: [],
    jira_available: true,
    ticketing_system: "jira",
    can_resume_from_error: false,
    terminal_url: null,
    run_commands: [],
    generate_report: false,
    has_report: false,
    definition_runs: {},
    workspace_name: "test-repo",
    ...overrides,
  };
}

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  vi.restoreAllMocks();
});

function mockFetchWorkflows(workflows: WorkflowSummary[]) {
  (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
    new Response(JSON.stringify(workflows), { status: 200 })
  );
}

describe("useWorkflows", () => {
  it("fetches and populates workflows on mount", async () => {
    const wf1 = makeWorkflow({ ticket_key: "TEST-1" });
    const wf2 = makeWorkflow({ ticket_key: "TEST-2", id: "uuid-2" });
    mockFetchWorkflows([wf1, wf2]);

    const { result } = renderHook(() => useWorkflows(), { wrapper: createQueryWrapper().wrapper });

    await vi.waitFor(() => {
      expect(Object.keys(result.current.workflows)).toHaveLength(2);
    });

    expect(result.current.workflows["TEST-1"].ticket_key).toBe("TEST-1");
    expect(result.current.workflows["TEST-2"].ticket_key).toBe("TEST-2");
    expect(result.current.orderKeys).toEqual(["TEST-1", "TEST-2"]);
  });

  it("derives counts from the workflow list (matches the grid)", async () => {
    mockFetchWorkflows([
      makeWorkflow({ ticket_key: "DONE-1", state: "Done" }),
      makeWorkflow({ ticket_key: "DONE-2", state: "Completed" }),
      makeWorkflow({ ticket_key: "RUN-1", state: "AddressingTicket" }),
      makeWorkflow({ ticket_key: "PAUSE-1", state: "Paused" }),
      makeWorkflow({ ticket_key: "ERR-1", state: "Error: boom" }),
      makeWorkflow({ ticket_key: "STOP-1", state: "Stopped" }),
      makeWorkflow({ ticket_key: "PEND-1", state: "Pending", can_start: true }),
    ]);

    const { result } = renderHook(() => useWorkflows(), { wrapper: createQueryWrapper().wrapper });

    await vi.waitFor(() => {
      expect(Object.keys(result.current.workflows)).toHaveLength(7);
    });

    // Stopped buckets with errors (matches the backend); Pending has its own bucket.
    expect(result.current.counts).toEqual({ running: 1, completed: 2, errors: 2, paused: 1, pending: 1 });
  });

  it("work_item_updated event triggers re-fetch (refreshes server-computed prep_state)", async () => {
    // Item starts "preparing" (worktree pre-creation in flight).
    mockFetchWorkflows([
      makeWorkflow({ ticket_key: "TEST-1", state: "Pending", prep_state: "preparing" }),
    ]);

    const { result } = renderHook(() => useWorkflows(), { wrapper: createQueryWrapper().wrapper });

    await vi.waitFor(() => {
      expect(result.current.workflows["TEST-1"]?.prep_state).toBe("preparing");
    });

    // Prep finished server-side → next fetch reports "ready". prep_state is NOT
    // carried on the event, so the handler must re-fetch (a patch would leave it stale).
    mockFetchWorkflows([
      makeWorkflow({ ticket_key: "TEST-1", state: "Pending", prep_state: "ready" }),
    ]);

    act(() => {
      result.current.handleEvent({
        event_type: "work_item_updated",
        workflow_id: "uuid-1",
        ticket_key: "TEST-1",
        state: "Pending",
        progress_percent: 0,
      });
    });

    await vi.waitFor(() => {
      expect(result.current.workflows["TEST-1"].prep_state).toBe("ready");
    });
  });

  it("workflow_removed event removes the entry", async () => {
    const wf1 = makeWorkflow({ ticket_key: "TEST-1" });
    mockFetchWorkflows([wf1]);

    const { result } = renderHook(() => useWorkflows(), { wrapper: createQueryWrapper().wrapper });

    await vi.waitFor(() => {
      expect(result.current.workflows["TEST-1"]).toBeDefined();
    });

    act(() => {
      result.current.handleEvent({
        event_type: "work_item_removed",
        workflow_id: "uuid-1",
        ticket_key: "TEST-1",
        state: "",
      });
    });

    expect(result.current.workflows["TEST-1"]).toBeUndefined();
    expect(result.current.orderKeys).not.toContain("TEST-1");
  });

  it("port_forwarded event triggers re-fetch", async () => {
    mockFetchWorkflows([makeWorkflow({ ticket_key: "TEST-1", editor_port_mappings: [] })]);

    const { result } = renderHook(() => useWorkflows(), { wrapper: createQueryWrapper().wrapper });

    await vi.waitFor(() => {
      expect(result.current.workflows["TEST-1"]).toBeDefined();
    });

    // Update mock to return port mappings on next fetch
    mockFetchWorkflows([makeWorkflow({ ticket_key: "TEST-1", editor_port_mappings: [[3000, "/s/abc123/"]] })]);

    act(() => {
      result.current.handleEvent({
        event_type: "port_forwarded",
        workflow_id: "uuid-1",
        ticket_key: "TEST-1",
        state: "",
        forwarded_port: [3000, 9100],
      });
    });

    // Re-fetch should populate dynamic forwards from API
    await vi.waitFor(() => {
      expect(result.current.dynamicForwards["TEST-1"]).toEqual([[3000, "/s/abc123/"]]);
    });
  });

  it("port_unforwarded event triggers re-fetch", async () => {
    mockFetchWorkflows([makeWorkflow({ ticket_key: "TEST-1", editor_port_mappings: [[3000, "/s/abc123/"]] })]);

    const { result } = renderHook(() => useWorkflows(), { wrapper: createQueryWrapper().wrapper });

    await vi.waitFor(() => {
      expect(result.current.dynamicForwards["TEST-1"]).toEqual([[3000, "/s/abc123/"]]);
    });

    // Update mock to return empty mappings on next fetch
    mockFetchWorkflows([makeWorkflow({ ticket_key: "TEST-1", editor_port_mappings: [] })]);

    act(() => {
      result.current.handleEvent({
        event_type: "port_unforwarded",
        workflow_id: "uuid-1",
        ticket_key: "TEST-1",
        state: "",
        forwarded_port: [3000, 9100],
      });
    });

    // Re-fetch should clear dynamic forwards
    await vi.waitFor(() => {
      expect(result.current.dynamicForwards["TEST-1"] ?? []).toHaveLength(0);
    });
  });

  it("new keys from refetch are appended, not re-sorted", async () => {
    const wf1 = makeWorkflow({ ticket_key: "TEST-1" });
    const wf2 = makeWorkflow({ ticket_key: "TEST-2", id: "uuid-2" });
    // URL-aware mock: the list query and the counts query both fire on mount,
    // so a positional `mockResolvedValueOnce` would race. `listBody` is the
    // controllable work-items response; counts is constant.
    let listBody: WorkflowSummary[] = [wf1];
    (fetch as ReturnType<typeof vi.fn>).mockImplementation(async (url: string) => {
      if (url === "/api/work-items") {
        return new Response(JSON.stringify(listBody), { status: 200 });
      }
      if (url === "/api/work-items/counts") {
        return new Response(
          JSON.stringify({ running: 0, completed: 0, errors: 0, paused: 0 }),
          { status: 200 }
        );
      }
      return new Response("[]", { status: 200 });
    });

    const { result } = renderHook(() => useWorkflows(), { wrapper: createQueryWrapper().wrapper });

    await vi.waitFor(() => {
      expect(result.current.orderKeys).toEqual(["TEST-1"]);
    });

    // Second fetch: TEST-1 + TEST-2 — TEST-2 should be appended, not re-sorted.
    listBody = [wf1, wf2];
    await act(async () => {
      await result.current.fetchWorkflows();
    });

    await vi.waitFor(() => {
      expect(result.current.orderKeys).toEqual(["TEST-1", "TEST-2"]);
    });
  });

  it("step_output appends terminal lines", async () => {
    mockFetchWorkflows([makeWorkflow({ ticket_key: "TEST-1" })]);

    const { result } = renderHook(() => useWorkflows(), { wrapper: createQueryWrapper().wrapper });

    await vi.waitFor(() => {
      expect(result.current.workflows["TEST-1"]).toBeDefined();
    });

    act(() => {
      result.current.handleEvent({
        event_type: "step_output",
        workflow_id: "uuid-1",
        ticket_key: "TEST-1",
        state: "",
        output_line: "Hello world",
        stream: "stdout",
      });
    });

    const lines = result.current.terminalStates["TEST-1"]?.lines;
    expect(lines).toBeDefined();
    expect(lines![lines!.length - 1].text).toBe("Hello world");
  });
});

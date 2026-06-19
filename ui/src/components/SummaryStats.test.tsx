// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { SummaryStats } from "./SummaryStats";
import { workflowMatchesStatus } from "./statusFilter";
import type { WorkflowCounts, WorkflowSummary } from "../api/types";

const COUNTS: WorkflowCounts = { running: 2, completed: 1, errors: 3, paused: 0, pending: 4 };

afterEach(cleanup);

describe("SummaryStats — filter cards", () => {
  it("clicking a card selects its status filter", () => {
    const onSelect = vi.fn();
    render(<SummaryStats counts={COUNTS} activeFilter={null} onSelectFilter={onSelect} />);
    fireEvent.click(screen.getByText("Errors"));
    expect(onSelect).toHaveBeenCalledWith("errors");
  });

  it("clicking the active card clears the filter (resets to all)", () => {
    const onSelect = vi.fn();
    render(<SummaryStats counts={COUNTS} activeFilter="running" onSelectFilter={onSelect} />);
    fireEvent.click(screen.getByText("Running"));
    expect(onSelect).toHaveBeenCalledWith(null);
  });

  it("shows a Pending card with its count and filters by it", () => {
    const onSelect = vi.fn();
    render(<SummaryStats counts={COUNTS} activeFilter={null} onSelectFilter={onSelect} />);
    const pending = screen.getByText("Pending").closest("button")!;
    expect(pending.textContent).toContain("4");
    fireEvent.click(pending);
    expect(onSelect).toHaveBeenCalledWith("pending");
  });

  it("marks the active card as pressed", () => {
    render(<SummaryStats counts={COUNTS} activeFilter="completed" onSelectFilter={vi.fn()} />);
    const completed = screen.getByText("Completed").closest("button")!;
    const running = screen.getByText("Running").closest("button")!;
    expect(completed.getAttribute("aria-pressed")).toBe("true");
    expect(running.getAttribute("aria-pressed")).toBe("false");
  });
});

describe("workflowMatchesStatus", () => {
  const wf = (state: string, can_start = false): WorkflowSummary =>
    ({ state, can_start } as WorkflowSummary);

  it("categorizes like the server counts (Stopped counts as errors)", () => {
    expect(workflowMatchesStatus(wf("AddressingTicket"), "running")).toBe(true);
    expect(workflowMatchesStatus(wf("Done"), "completed")).toBe(true);
    expect(workflowMatchesStatus(wf("Error"), "errors")).toBe(true);
    expect(workflowMatchesStatus(wf("Stopped"), "errors")).toBe(true);
    expect(workflowMatchesStatus(wf("Paused"), "paused")).toBe(true);
    expect(workflowMatchesStatus(wf("Pending", true), "pending")).toBe(true);
    expect(workflowMatchesStatus(wf("Pending", true), "running")).toBe(false);
    // A streaming step-name state still classifies as running.
    expect(workflowMatchesStatus(wf("Lint and test"), "running")).toBe(true);
    // Cross-bucket negatives.
    expect(workflowMatchesStatus(wf("Done"), "running")).toBe(false);
    expect(workflowMatchesStatus(wf("Paused"), "errors")).toBe(false);
  });
});

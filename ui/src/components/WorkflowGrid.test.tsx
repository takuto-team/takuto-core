// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * The dashboard's add ("+") affordance is gated by `canAddWorkflow`. The
 * Dashboard sets that to false when "All repositories" is selected (no target
 * repo to add an item to), so the "+" card must not render in that case.
 * These tests pin the grid's side of that contract.
 */

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import type { WorkflowSummary } from "../api/types";

// Stub IssueCard so the grid can render a populated list without dragging in
// the card's controller/toast/provider tree.
vi.mock("./IssueCard", () => ({
  IssueCard: () => <div data-testid="issue-card" />,
}));

import { WorkflowGrid } from "./WorkflowGrid";

afterEach(cleanup);

const item = { ticket_key: "GH-1", workspace_name: "repo-a" } as unknown as WorkflowSummary;

function renderGrid(canAddWorkflow: boolean) {
  render(
    <WorkflowGrid
      workflows={{ "GH-1": item }}
      orderKeys={["GH-1"]}
      terminalStates={{}}
      dynamicForwards={{}}
      workflowDefs={[]}
      onRefresh={vi.fn()}
      onShowDescription={vi.fn()}
      onReport={vi.fn()}
      onAddWorkflow={vi.fn()}
      canAddWorkflow={canAddWorkflow}
      repoExists
      activeRepoName={null}
    />,
  );
}

describe("WorkflowGrid add button", () => {
  it("renders the + card when adding is allowed", () => {
    renderGrid(true);
    expect(screen.getByRole("button", { name: "+" })).toBeTruthy();
  });

  it("hides the + card when adding is not allowed (e.g. All repositories)", () => {
    renderGrid(false);
    expect(screen.queryByRole("button", { name: "+" })).toBeNull();
    // The item still renders — only the add affordance is gone.
    expect(screen.getByTestId("issue-card")).toBeTruthy();
  });
});

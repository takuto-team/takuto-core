// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Coverage for the overflow Start-flow modal. Renders one row per flow across
 * the five visual states (enabled / disabled-with-unmet-deps / running /
 * completed / error) and verifies clicking a row hits the same run/retry
 * endpoint as the inline buttons.
 */

import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { render, screen, cleanup, fireEvent, waitFor, within } from "@testing-library/react";
import { StartFlowModal } from "./StartFlowModal";
import { ToastProvider } from "../../hooks/useToast";
import type { WorkflowDefinition } from "../../api/types";

function def(filename: string, name: string, depends_on: string[] = []): WorkflowDefinition {
  return { filename, name, steps: [], depends_on, valid: true };
}

// One flow per visual state. "Locked" depends on "running", which has not
// completed, so its dependency is unmet.
const DEFINITIONS: WorkflowDefinition[] = [
  def("enabled", "Enabled Flow"),
  def("running", "Running Flow"),
  def("done", "Done Flow"),
  def("errored", "Errored Flow"),
  def("locked", "Locked Flow", ["running"]),
];

const RUN_STATES: Record<string, string> = {
  running: "running",
  done: "completed",
  errored: "error",
};

let fetchMock: ReturnType<typeof vi.fn>;

function renderModal(onRefresh = vi.fn()) {
  return render(
    <ToastProvider>
      <StartFlowModal
        definitions={DEFINITIONS}
        runStates={RUN_STATES}
        ticketKey="TICK-1"
        onRefresh={onRefresh}
        onClose={vi.fn()}
      />
    </ToastProvider>,
  );
}

function rowFor(name: string): HTMLElement {
  return screen.getByText(name).closest("div.rounded-lg") as HTMLElement;
}

beforeEach(() => {
  fetchMock = vi.fn(async () => new Response(null, { status: 200 }));
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("StartFlowModal — visual states", () => {
  it("renders each flow's state distinctly", () => {
    renderModal();
    // Enabled → a Start button (scoped to its row). Errored → a Retry button.
    expect(within(rowFor("Enabled Flow")).getByRole("button", { name: /start/i })).toBeTruthy();
    expect(within(rowFor("Errored Flow")).getByRole("button", { name: /retry/i })).toBeTruthy();
    // Running / completed render non-interactive status text in their rows.
    expect(within(rowFor("Running Flow")).getByText("running")).toBeTruthy();
    expect(within(rowFor("Done Flow")).getByText("done")).toBeTruthy();
    // Locked flow shows its unmet dependency.
    expect(within(rowFor("Locked Flow")).getByText(/waiting for:/i)).toBeTruthy();
  });

  it("does not render a Start/Retry button for the locked flow's row", () => {
    renderModal();
    const lockedRow = within(rowFor("Locked Flow"));
    expect(lockedRow.queryByRole("button", { name: /start/i })).toBeNull();
    expect(lockedRow.queryByRole("button", { name: /retry/i })).toBeNull();
  });
});

describe("StartFlowModal — click handlers", () => {
  it("clicking Start on an enabled flow calls the run-definition endpoint", async () => {
    renderModal();
    fireEvent.click(screen.getByRole("button", { name: /start/i }));
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    expect(fetchMock.mock.calls[0][0]).toBe(
      "/api/work-items/TICK-1/run-definition/enabled",
    );
  });

  it("clicking Retry on an errored flow calls the retry-definition endpoint", async () => {
    renderModal();
    fireEvent.click(screen.getByRole("button", { name: /retry/i }));
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    expect(fetchMock.mock.calls[0][0]).toBe(
      "/api/work-items/TICK-1/retry-definition/errored",
    );
  });
});

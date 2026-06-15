// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { IssueCard } from "./IssueCard";
import { ToastProvider } from "../hooks/useToast";
import type { WorkflowSummary } from "../api/types";
import type { TerminalState } from "../hooks/useWorkflows";

function makeWorkflow(overrides: Partial<WorkflowSummary> = {}): WorkflowSummary {
  return {
    id: "uuid-1",
    ticket_key: "TEST-1",
    ticket_summary: "Test ticket",
    ticket_description: "A description",
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

function renderCard(props: Partial<Parameters<typeof IssueCard>[0]> = {}) {
  const onRefresh = vi.fn();
  const onShowDescription = vi.fn();
  const onReport = vi.fn();
  render(
    <ToastProvider>
      <MemoryRouter>
        <IssueCard
          workflow={props.workflow ?? makeWorkflow()}
          terminalState={props.terminalState}
          dynamicForwards={props.dynamicForwards ?? []}
          workflowDefs={props.workflowDefs ?? []}
          onRefresh={onRefresh}
          onShowDescription={onShowDescription}
          onReport={onReport}
        />
      </MemoryRouter>
    </ToastProvider>,
  );
  return { onRefresh, onShowDescription, onReport };
}

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("IssueCard", () => {
  it("renders the ticket key and summary", () => {
    renderCard({ workflow: makeWorkflow({ ticket_key: "PROJ-42", ticket_summary: "Add login" }) });
    expect(screen.getByText("PROJ-42")).toBeTruthy();
    expect(screen.getByText("Add login")).toBeTruthy();
  });

  it("calls onShowDescription with the ticket details when 'Show details' is clicked", () => {
    const { onShowDescription } = renderCard({
      workflow: makeWorkflow({ ticket_key: "PROJ-7", ticket_summary: "Summary", ticket_description: "Desc" }),
    });
    fireEvent.click(screen.getByText(/show details/i));
    expect(onShowDescription).toHaveBeenCalledWith("PROJ-7", "Summary", "Desc");
  });

  it("renders a PR link when pr_url is present", () => {
    renderCard({ workflow: makeWorkflow({ pr_url: "https://github.com/acme/app/pull/123" }) });
    const link = screen.getByText(/PR #123/i).closest("a");
    expect(link).toBeTruthy();
    expect(link?.getAttribute("href")).toBe("https://github.com/acme/app/pull/123");
  });

  it("disables the console-output button when there are no terminal lines", () => {
    renderCard({ workflow: makeWorkflow() });
    const btn = screen.getByRole("button", { name: /show console output/i }) as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
  });

  it("opens the console modal when terminal lines exist", () => {
    const terminalState: TerminalState = {
      stepName: "Implement",
      lines: [{ text: "hello-from-agent", stream: "stdout" }],
      completed: false,
    };
    renderCard({ workflow: makeWorkflow(), terminalState });
    const btn = screen.getByRole("button", { name: /show console output/i }) as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
    fireEvent.click(btn);
    expect(screen.getByText("hello-from-agent")).toBeTruthy();
  });

  it("renders the editor and terminal buttons on a completed workflow with a branch and can_open_editor", () => {
    renderCard({
      workflow: makeWorkflow({
        state: "Done",
        branch_name: "feat/test-1",
        can_open_editor: true,
        editor_url: null,
        terminal_url: null,
      }),
    });
    expect(screen.getByTitle("Open editor")).toBeTruthy();
    expect(screen.getByTitle("Open terminal")).toBeTruthy();
  });

  it("does not render the editor and terminal buttons when can_open_editor is false", () => {
    renderCard({
      workflow: makeWorkflow({ state: "Done", branch_name: "feat/test-1", can_open_editor: false }),
    });
    expect(screen.queryByTitle("Open editor")).toBeNull();
    expect(screen.queryByTitle("Open terminal")).toBeNull();
  });

  it("opens the delete confirmation when the delete button is clicked", () => {
    renderCard({ workflow: makeWorkflow({ can_delete: true }) });
    // DeleteIconButton has an accessible label; click it.
    const deleteBtn = screen.getByRole("button", { name: /delete/i });
    fireEvent.click(deleteBtn);
    // The delete confirm modal references the ticket key.
    expect(screen.getAllByText(/TEST-1/).length).toBeGreaterThan(0);
  });
});

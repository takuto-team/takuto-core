// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Coverage for the manual-add ticket picker: rows already on the board are
 * disabled with an "Already added" message; selecting an item that already has
 * a PR routes through a confirmation (the new run opens a separate PR), while a
 * plain item adds immediately.
 */

import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { render, screen, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { TicketPickerModal } from "./TicketPickerModal";
import type { GitHubIssue } from "../../api/types";

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

const ISSUES: GitHubIssue[] = [
  { key: "GH-1", summary: "Plain issue", body: "b1", url: "https://x/1", already_added: false },
  { key: "GH-2", summary: "On the board", body: "b2", url: "https://x/2", already_added: true },
  {
    key: "GH-4",
    summary: "Has a prior PR",
    body: "b4",
    url: "https://x/4",
    already_added: false,
    existing_pr_url: "https://github.com/o/r/pull/18",
  },
];

let fetchMock: ReturnType<typeof vi.fn>;

function renderPicker(onSelect = vi.fn(), onClose = vi.fn()) {
  render(
    <TicketPickerModal
      ticketingSystem="github"
      activeRepoName="repo"
      onSelect={onSelect}
      onClose={onClose}
    />,
  );
  return { onSelect, onClose };
}

beforeEach(() => {
  // URL-aware: the issue list returns ISSUES; the per-ticket GitHub PR check
  // defaults to "no PR found" (individual tests override it).
  fetchMock = vi.fn(async (input: string) => {
    const url = typeof input === "string" ? input : String(input);
    if (url.startsWith("/api/github/existing-pr")) return json({ pr_url: null });
    return json(ISSUES);
  });
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("TicketPickerModal", () => {
  it("disables an already-added row with an 'Already added' message", async () => {
    renderPicker();
    expect(await screen.findByText("On the board")).toBeTruthy();
    expect(screen.getByText("Already added")).toBeTruthy();
    // The already-added row is not a clickable button.
    expect(screen.queryByRole("button", { name: /On the board/i })).toBeNull();
  });

  it("adds an item with no PR (local or GitHub) after the on-click check", async () => {
    const { onSelect } = renderPicker();
    fireEvent.click(await screen.findByText("Plain issue"));
    // The GitHub check runs on click; once it returns "no PR" the item is added.
    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith("GH-1", "Plain issue", "b1", "https://x/1"),
    );
    expect(screen.queryByText(/already has/i)).toBeNull();
  });

  it("prompts when the on-click GitHub check finds a PR (no local PR)", async () => {
    // No local existing_pr_url on GH-1, but GitHub reports an open PR.
    fetchMock.mockImplementation(async (input: string) => {
      const url = typeof input === "string" ? input : String(input);
      if (url.startsWith("/api/github/existing-pr")) {
        return json({ pr_url: "https://github.com/o/r/pull/77" });
      }
      return json(ISSUES);
    });
    const { onSelect } = renderPicker();
    fireEvent.click(await screen.findByText("Plain issue"));
    expect(await screen.findByText(/GH-1 already has #77/i)).toBeTruthy();
    expect(onSelect).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /add anyway/i }));
    expect(onSelect).toHaveBeenCalledWith("GH-1", "Plain issue", "b1", "https://x/1");
  });

  it("prompts before re-adding an item that already has a PR, and only adds after confirm", async () => {
    const { onSelect } = renderPicker();
    fireEvent.click(await screen.findByText("Has a prior PR"));
    // Confirmation appears naming the existing PR; nothing added yet.
    expect(screen.getByText(/GH-4 already has #18/i)).toBeTruthy();
    expect(onSelect).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /add anyway/i }));
    expect(onSelect).toHaveBeenCalledWith("GH-4", "Has a prior PR", "b4", "https://x/4");
  });

  it("does not add when the re-add confirmation is cancelled", async () => {
    const { onSelect } = renderPicker();
    fireEvent.click(await screen.findByText("Has a prior PR"));
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(onSelect).not.toHaveBeenCalled();
    expect(screen.queryByText(/already has #18/i)).toBeNull();
  });

  it("fetches the GitHub issue list scoped to the active repository", async () => {
    renderPicker();
    await screen.findByText("Plain issue");
    const issuesCall = fetchMock.mock.calls.find(([u]) =>
      String(u).startsWith("/api/github/issues"),
    );
    expect(issuesCall).toBeTruthy();
    expect(String(issuesCall![0])).toBe("/api/github/issues?repository=repo");
  });

  it("fetches the Jira manual list scoped to the active repository", async () => {
    // Jira mode hits the manual-picker endpoint, which requires ?repository=.
    fetchMock.mockImplementation(async () =>
      json([{ key: "PROJ-1", summary: "A Jira ticket", item_type: "Task", already_added: false }]),
    );
    render(
      <TicketPickerModal
        ticketingSystem="jira"
        activeRepoName="repo"
        onSelect={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    await screen.findByText("A Jira ticket");
    const jiraCall = fetchMock.mock.calls.find(([u]) =>
      String(u).startsWith("/api/jira/todo-tickets-manual"),
    );
    expect(jiraCall).toBeTruthy();
    expect(String(jiraCall![0])).toBe("/api/jira/todo-tickets-manual?repository=repo");
  });
});

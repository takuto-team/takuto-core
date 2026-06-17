// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Coverage for the manual-add ticket picker: rows already on the board are
 * disabled with an "Already added" message; selecting an item that already has
 * a PR routes through a confirmation (the new run opens a separate PR), while a
 * plain item adds immediately.
 */

import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { render, screen, cleanup, fireEvent } from "@testing-library/react";
import { TicketPickerModal } from "./TicketPickerModal";
import type { GitHubIssue } from "../../api/types";

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
  fetchMock = vi.fn(async () => new Response(JSON.stringify(ISSUES), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  }));
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

  it("adds a plain item immediately (no confirmation)", async () => {
    const { onSelect } = renderPicker();
    fireEvent.click(await screen.findByText("Plain issue"));
    expect(onSelect).toHaveBeenCalledWith("GH-1", "Plain issue", "b1", "https://x/1");
    expect(screen.queryByText(/already has/i)).toBeNull();
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
});

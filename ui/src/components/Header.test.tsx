// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Header dropdown contains a single "AI Settings" entry visible to every
 * authenticated user (admin-only sections are gated *inside* the tab, not
 * by the Header link). Server-side enforcement at `PUT /api/config/agent`
 * (04_architecture.md §2.3) remains the real security boundary.
 */

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { Header } from "./Header";

afterEach(() => {
  cleanup();
});

function renderHeader(props: Partial<React.ComponentProps<typeof Header>> = {}) {
  return render(
    <MemoryRouter>
      <Header
        connected
        authEnabled
        githubAppConfigured={false}
        onLogout={vi.fn()}
        {...props}
      />
    </MemoryRouter>,
  );
}

describe("Header — AI Settings link", () => {
  it("renders the 'AI Settings' link pointing at /config.html?tab=ai", () => {
    renderHeader();
    // Dropdown is collapsed by default — open it by clicking the
    // hamburger button.
    fireEvent.click(screen.getByRole("button", { name: /menu/i }));
    const link = screen.getByRole("link", { name: /^ai settings$/i });
    expect(link.getAttribute("href")).toBe("/config.html?tab=ai");
  });
});

describe("Header — dropdown lifecycle (existing behaviour preserved)", () => {
  it("does not render the dropdown items until the menu is opened", () => {
    renderHeader();
    // Before clicking, the dropdown links should not be present in the DOM.
    expect(screen.queryByRole("link", { name: /^ai settings$/i })).toBeNull();
  });
});

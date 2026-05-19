// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Phase 1 / #34 regression — the admin-only "AI Settings" entry in the
 * Header dropdown must be visible to admins and hidden from regular users.
 *
 * The Header gets `isAdmin` as a prop (forwarded from App.tsx via
 * Dashboard). Server-side enforcement still happens on
 * `PUT /api/config/agent` (04_architecture.md §2.3) — this test only
 * covers client-side visibility.
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

describe("Header — admin AI Settings link (#34)", () => {
  it("renders the 'AI Settings' link in the dropdown for admins", () => {
    renderHeader({ isAdmin: true });
    // Dropdown is collapsed by default — open it by clicking the
    // hamburger button.
    fireEvent.click(screen.getByRole("button", { name: /menu/i }));
    const link = screen.getByRole("link", { name: /^ai settings$/i });
    expect(link.getAttribute("href")).toBe("/admin/ai");
  });

  it("does NOT render the 'AI Settings' link for non-admins", () => {
    renderHeader({ isAdmin: false });
    fireEvent.click(screen.getByRole("button", { name: /menu/i }));
    // "My credentials" remains for everyone — sanity check that the
    // dropdown opened.
    expect(screen.getByRole("link", { name: /^my credentials$/i })).toBeTruthy();
    expect(screen.queryByRole("link", { name: /^ai settings$/i })).toBeNull();
  });

  it("defaults to non-admin (no isAdmin prop) → link hidden", () => {
    renderHeader();
    fireEvent.click(screen.getByRole("button", { name: /menu/i }));
    expect(screen.queryByRole("link", { name: /^ai settings$/i })).toBeNull();
  });
});

describe("Header — dropdown lifecycle (existing behaviour preserved)", () => {
  it("does not render the dropdown items until the menu is opened", () => {
    renderHeader({ isAdmin: true });
    // Before clicking, the dropdown links should not be present in the DOM.
    expect(
      screen.queryByRole("link", { name: /^my credentials$/i }),
    ).toBeNull();
    expect(screen.queryByRole("link", { name: /^ai settings$/i })).toBeNull();
  });
});

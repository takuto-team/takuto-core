// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Route-level guard tests for the #34 follow-up — admin-gated routes must
 * NOT bounce the user during the auth-loading window. The unit-level
 * `useAuth` race fix lives in `hooks/useAuth.test.ts`; this file exercises
 * the `<RequireAdmin>` guard, which today is unused by App.tsx (the
 * /admin/ai page was folded into a tab in /config.html) but is kept
 * exported for future admin-only routes. The tests still exercise the
 * loading-vs-redirect logic — the example URL is illustrative.
 */

import { describe, it, expect, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { RequireAdmin } from "./App";

afterEach(() => {
  cleanup();
});

function renderAtAdminAi(node: React.ReactNode) {
  return render(
    <MemoryRouter initialEntries={["/admin/ai"]}>
      <Routes>
        <Route path="/" element={<p>dashboard</p>} />
        <Route path="/admin/ai" element={node} />
      </Routes>
    </MemoryRouter>,
  );
}

describe("RequireAdmin — auth-loading race guard (#34 follow-up)", () => {
  it("renders the Loading spinner (NOT Navigate) when loading=true even if currentUser is null", () => {
    // This is the exact race the team-lead flagged: a direct mount at
    // /admin/ai while auth is still loading would previously Navigate the
    // user to "/" because currentUser was null. The guard must keep them
    // on the page until loading resolves.
    renderAtAdminAi(
      <RequireAdmin loading={true} currentUser={null}>
        <p>admin page</p>
      </RequireAdmin>,
    );
    expect(screen.getByText(/^Loading\.\.\.$/)).toBeTruthy();
    // The admin page MUST NOT have rendered.
    expect(screen.queryByText("admin page")).toBeNull();
    // And we must NOT have been navigated to "/".
    expect(screen.queryByText("dashboard")).toBeNull();
  });

  it("renders the Loading spinner even if currentUser is already an admin (loading wins)", () => {
    // Defensive: if loading is true, the guard waits regardless of what
    // currentUser reports — we trust loading as the source of truth for
    // "the auth state is still settling".
    renderAtAdminAi(
      <RequireAdmin
        loading={true}
        currentUser={{ role: "admin" }}
      >
        <p>admin page</p>
      </RequireAdmin>,
    );
    expect(screen.getByText(/^Loading\.\.\.$/)).toBeTruthy();
    expect(screen.queryByText("admin page")).toBeNull();
  });

  it("renders children when loading=false AND currentUser.role='admin'", () => {
    renderAtAdminAi(
      <RequireAdmin
        loading={false}
        currentUser={{ role: "admin" }}
      >
        <p>admin page</p>
      </RequireAdmin>,
    );
    expect(screen.getByText("admin page")).toBeTruthy();
    expect(screen.queryByText("dashboard")).toBeNull();
  });

  it("navigates to '/' when loading=false AND currentUser.role='user'", () => {
    renderAtAdminAi(
      <RequireAdmin
        loading={false}
        currentUser={{ role: "user" }}
      >
        <p>admin page</p>
      </RequireAdmin>,
    );
    expect(screen.queryByText("admin page")).toBeNull();
    expect(screen.getByText("dashboard")).toBeTruthy();
  });

  it("navigates to '/' when loading=false AND currentUser is null (auth/me returned non-OK)", () => {
    // This case represents a logged-out / broken-auth user — Navigate is
    // safer than sticking on a spinner forever.
    renderAtAdminAi(
      <RequireAdmin loading={false} currentUser={null}>
        <p>admin page</p>
      </RequireAdmin>,
    );
    expect(screen.queryByText("admin page")).toBeNull();
    expect(screen.getByText("dashboard")).toBeTruthy();
  });
});

describe("RequireAdmin — admin user direct URL load (full scenario)", () => {
  it("does NOT redirect prematurely when an admin direct-loads /admin/ai while auth is still loading", () => {
    // Simulate the full timeline: render once with loading=true (no
    // currentUser yet), then re-render with loading=false + admin
    // currentUser. Assert the page stays put across the transition.
    const { rerender } = render(
      <MemoryRouter initialEntries={["/admin/ai"]}>
        <Routes>
          <Route path="/" element={<p>dashboard</p>} />
          <Route
            path="/admin/ai"
            element={
              <RequireAdmin loading={true} currentUser={null}>
                <p>admin page</p>
              </RequireAdmin>
            }
          />
        </Routes>
      </MemoryRouter>,
    );
    // Loading phase: spinner shown, NOT bounced to dashboard.
    expect(screen.getByText(/^Loading\.\.\.$/)).toBeTruthy();
    expect(screen.queryByText("dashboard")).toBeNull();

    // Auth resolves with admin role.
    rerender(
      <MemoryRouter initialEntries={["/admin/ai"]}>
        <Routes>
          <Route path="/" element={<p>dashboard</p>} />
          <Route
            path="/admin/ai"
            element={
              <RequireAdmin
                loading={false}
                currentUser={{ role: "admin" }}
              >
                <p>admin page</p>
              </RequireAdmin>
            }
          />
        </Routes>
      </MemoryRouter>,
    );
    expect(screen.getByText("admin page")).toBeTruthy();
    expect(screen.queryByText("dashboard")).toBeNull();
  });
});

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Regression guard: an already-authenticated user must never see the login
 * form. The `/login.html` route (reachable only when authenticated) redirects
 * to the `?return=` target (safe same-origin path) or the dashboard.
 */

import { describe, it, expect, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { LoginRouteRedirect } from "./App";

afterEach(cleanup);

function renderAt(search: string) {
  window.history.replaceState({}, "", `/login.html${search}`);
  render(
    <MemoryRouter initialEntries={[`/login.html${search}`]}>
      <Routes>
        <Route path="/login.html" element={<LoginRouteRedirect />} />
        <Route path="/" element={<div>DASHBOARD</div>} />
        <Route path="/config.html" element={<div>CONFIG</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

describe("LoginRouteRedirect", () => {
  it("redirects to the dashboard when there is no return param", () => {
    renderAt("");
    expect(screen.getByText("DASHBOARD")).toBeTruthy();
  });

  it("redirects to a safe same-origin return path", () => {
    renderAt("?return=%2Fconfig.html");
    expect(screen.getByText("CONFIG")).toBeTruthy();
  });

  it("ignores an unsafe (protocol-relative) return and falls back to dashboard", () => {
    renderAt("?return=%2F%2Fevil.example.com");
    expect(screen.getByText("DASHBOARD")).toBeTruthy();
  });
});

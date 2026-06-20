// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * The dependency-install overlay shows "Installing dependencies" + the current
 * step while installing, an error message on failure, and nothing once ready.
 */

import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { render, screen, cleanup, waitFor } from "@testing-library/react";
import { DependencyInstallOverlay } from "./DependencyInstallOverlay";
import type { DependencyInstallStatus } from "../api/system";

function stub(status: DependencyInstallStatus) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      new Response(JSON.stringify(status), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    ),
  );
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("DependencyInstallOverlay", () => {
  beforeEach(() => vi.unstubAllGlobals());

  it("shows the current step while installing", async () => {
    stub({ phase: "installing", current_step: "Claude Code (latest)", done: 0, total: 4 });
    render(<DependencyInstallOverlay />);
    await waitFor(() => expect(screen.getByText("Installing dependencies")).toBeTruthy());
    expect(screen.getByText("Claude Code (latest)")).toBeTruthy();
    expect(screen.getByText("1 / 4")).toBeTruthy();
  });

  it("renders nothing once ready", async () => {
    stub({ phase: "ready", current_step: "", done: 4, total: 4 });
    const { container } = render(<DependencyInstallOverlay />);
    // Give the poll a tick; the overlay must stay empty.
    await new Promise((r) => setTimeout(r, 10));
    expect(container.textContent).toBe("");
  });

  it("shows the error on failure", async () => {
    stub({ phase: "error", current_step: "", done: 1, total: 4, error: "Cursor Agent: boom" });
    render(<DependencyInstallOverlay />);
    await waitFor(() =>
      expect(screen.getByText("Could not install dependencies")).toBeTruthy(),
    );
    expect(screen.getByText("Cursor Agent: boom")).toBeTruthy();
  });
});

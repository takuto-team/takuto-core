// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import { RunCommands } from "./RunCommands";
import type { RunCommandStatus } from "../api/types";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const noop = async () => {};

function cmd(overrides: Partial<RunCommandStatus> = {}): RunCommandStatus {
  return { index: 0, name: "Run app", running: false, forwarded_port: null, ...overrides };
}

function renderCmds(commands: RunCommandStatus[]) {
  return render(<RunCommands ticketKey="GH-8" commands={commands} withLoading={noop} />);
}

describe("RunCommands", () => {
  it("shows a Run button when the command is stopped", () => {
    renderCmds([cmd({ running: false })]);
    expect(screen.getByRole("button", { name: /run run app/i })).toBeTruthy();
    expect(screen.queryByText("Open")).toBeNull();
  });

  it("shows functional Copy/Open once a port is forwarded", () => {
    const { container } = renderCmds([
      cmd({ running: true, forwarded_port: [3001, "/p/3001/"] }),
    ]);
    expect(screen.getByRole("button", { name: /stop run app/i })).toBeTruthy();
    const open = screen.getByText("Open").closest("a") as HTMLAnchorElement;
    expect(open.getAttribute("href")).toBe("/p/3001/");
    // No pending spinner state.
    expect(container.querySelector('[aria-busy="true"]')).toBeNull();
    expect(container.querySelector(".animate-spin")).toBeNull();
  });

  it("covers Copy/Open with a spinner overlay while running before a port is detected", () => {
    const { container } = renderCmds([cmd({ running: true, forwarded_port: null })]);
    expect(screen.getByRole("button", { name: /stop run app/i })).toBeTruthy();
    // Both Copy and Open are present but marked busy with a spinner overlay.
    const busy = container.querySelectorAll('[aria-busy="true"]');
    expect(busy.length).toBe(2);
    expect(container.querySelectorAll(".animate-spin").length).toBe(2);
    // They are not actionable links/buttons yet.
    expect(screen.queryByText("Open")?.closest("a")).toBeNull();
  });
});

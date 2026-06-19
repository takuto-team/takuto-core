// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * The auto-saving report switch loads the workspace's worktree-commands row,
 * reflects `generate_report`, and on flip PUTs the row back with the new flag
 * while PRESERVING the existing init/run commands (the flag shares the row).
 */

import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { render, screen, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { GenerateReportSwitch } from "./GenerateReportSwitch";

const ROW = {
  workspace_name: "ws",
  init_commands: ["npm ci"],
  run_commands: [{ name: "dev", command: "npm run dev" }],
  generate_report: false,
  updated_at: 0,
};

let putBodies: Record<string, unknown>[];

beforeEach(() => {
  putBodies = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_input: string, init: RequestInit = {}) => {
      if (init.method === "PUT") {
        const body = JSON.parse(init.body as string);
        putBodies.push(body);
        return new Response(JSON.stringify({ ...ROW, ...body }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      return new Response(JSON.stringify(ROW), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }),
  );
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("GenerateReportSwitch", () => {
  it("loads and reflects the row's generate_report", async () => {
    render(<GenerateReportSwitch workspace="ws" />);
    await waitFor(() =>
      expect(screen.getByRole("switch").getAttribute("aria-checked")).toBe("false"),
    );
  });

  it("flipping on PUTs generate_report=true and preserves init/run commands", async () => {
    render(<GenerateReportSwitch workspace="ws" />);
    await waitFor(() => expect(screen.getByRole("switch")).toBeTruthy());

    fireEvent.click(screen.getByRole("switch"));

    await waitFor(() => expect(putBodies.length).toBe(1));
    expect(putBodies[0]).toEqual({
      init_commands: ["npm ci"],
      run_commands: [{ name: "dev", command: "npm run dev" }],
      generate_report: true,
    });
    await waitFor(() =>
      expect(screen.getByRole("switch").getAttribute("aria-checked")).toBe("true"),
    );
    // A transient "Saved" confirmation appears after a successful flip.
    await waitFor(() => expect(screen.getByText("Saved")).toBeTruthy());
  });

  it("clears a pending Saved message when flipped again", async () => {
    render(<GenerateReportSwitch workspace="ws" />);
    await waitFor(() => expect(screen.getByRole("switch")).toBeTruthy());

    fireEvent.click(screen.getByRole("switch")); // on → "Saved" shows
    await waitFor(() => expect(screen.getByText("Saved")).toBeTruthy());

    fireEvent.click(screen.getByRole("switch")); // off → message cleared immediately
    expect(screen.queryByText("Saved")).toBeNull();
    // …then reappears once the second save lands.
    await waitFor(() => expect(screen.getByText("Saved")).toBeTruthy());
  });
});

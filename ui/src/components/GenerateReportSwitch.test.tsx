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

/**
 * Wait until the switch is loaded and interactive. The switch renders while the
 * initial GET is in flight but stays `disabled` (loading), and `onChange` bails
 * out on a flip during load. `fireEvent.click` ignores `disabled`, so clicking
 * before the load resolves is silently dropped and the "Saved" confirmation
 * never appears — a timeout flake under slow CI. Real users can't click a
 * disabled control, so gate the test clicks the same way.
 */
async function switchWhenReady(): Promise<HTMLButtonElement> {
  const sw = () => screen.getByRole("switch") as HTMLButtonElement;
  await waitFor(() => expect(sw().disabled).toBe(false));
  return sw();
}

describe("GenerateReportSwitch", () => {
  it("loads and reflects the row's generate_report", async () => {
    render(<GenerateReportSwitch workspace="ws" />);
    await waitFor(() =>
      expect(screen.getByRole("switch").getAttribute("aria-checked")).toBe("false"),
    );
  });

  it("flipping on PUTs generate_report=true and preserves init/run commands", async () => {
    render(<GenerateReportSwitch workspace="ws" />);
    fireEvent.click(await switchWhenReady());

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

    fireEvent.click(await switchWhenReady()); // on → "Saved" shows
    await waitFor(() => expect(screen.getByText("Saved")).toBeTruthy());

    fireEvent.click(await switchWhenReady()); // off → message cleared immediately
    expect(screen.queryByText("Saved")).toBeNull();
    // …then reappears once the second save lands.
    await waitFor(() => expect(screen.getByText("Saved")).toBeTruthy());
  });
});

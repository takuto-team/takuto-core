// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Light coverage for the Workflows tab: it loads the repo list, defaults to the
 * first repo, loads that repo's flow list, and a drag-reorder persists the new
 * order via PUT /api/me/flows?workspace=<repo>.
 */

import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { render, screen, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { FlowsTab } from "./FlowsTab";
import { createQueryWrapper } from "../test/queryWrapper";
import type { UserFlow } from "../api/flows";

function flow(name: string): UserFlow {
  return { name, depends_on: [], steps: [{ name: "s", prompt: "p", skills: [] }] };
}

const INITIAL: UserFlow[] = [flow("Alpha"), flow("Bravo")];

let putBodies: unknown[];

function flowsResponse(flows: UserFlow[]): Response {
  return new Response(JSON.stringify({ flows, workspace: "ws" }), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

beforeEach(() => {
  putBodies = [];
  try {
    localStorage.clear();
  } catch {
    /* ignore */
  }
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init: RequestInit = {}) => {
      const url = typeof input === "string" ? input : String(input);
      const method = (init.method ?? "GET").toUpperCase();

      // Repo list — two repos; the first ("ws") is the default selection.
      if (url.startsWith("/api/repositories")) {
        const repos = [
          { id: "1", name: "ws", repo_url: null, local_path: "/ws", default_branch: "main" },
          { id: "2", name: "other", repo_url: null, local_path: "/o", default_branch: "main" },
        ];
        return new Response(JSON.stringify(repos), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }

      // Report toggle's row lookup — no row yet.
      if (url.startsWith("/api/worktree-commands/")) {
        return new Response(null, { status: 404 });
      }

      // Flows — PUT echoes, GET returns the initial list.
      if (url.startsWith("/api/me/flows")) {
        if (method === "PUT") {
          const body = JSON.parse(init.body as string);
          putBodies.push({ url, flows: body.flows });
          return flowsResponse(body.flows);
        }
        return flowsResponse(INITIAL);
      }
      return new Response(null, { status: 404 });
    }),
  );
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderTab() {
  const { wrapper } = createQueryWrapper();
  return render(<FlowsTab />, { wrapper });
}

function cardFor(name: string): HTMLElement {
  return screen.getByText(name).closest('div[draggable="true"]') as HTMLElement;
}

describe("FlowsTab", () => {
  it("renders the selected repo's flow list", async () => {
    renderTab();
    await waitFor(() => expect(screen.getByText("Alpha")).toBeTruthy());
    expect(screen.getByText("Bravo")).toBeTruthy();
    expect(screen.getByText("2 / 20")).toBeTruthy();
  });

  it("defaults to the first repo and shows it in the heading", async () => {
    renderTab();
    await waitFor(() => expect(screen.getByText("Alpha")).toBeTruthy());
    expect(screen.getByRole("heading", { name: /^Workflows — ws/i })).toBeTruthy();
    expect(screen.getByRole("button", { name: /\+ Add workflow/i })).toBeTruthy();
  });

  it("a drag-reorder PUTs the list for the selected workspace", async () => {
    renderTab();
    await waitFor(() => expect(screen.getByText("Alpha")).toBeTruthy());

    // Drag "Bravo" (index 1) over the top half of "Alpha" (index 0) and drop.
    // jsdom's `fireEvent.dragOver` strips `clientY`, so dispatch a MouseEvent
    // directly (its init carries clientY through to React's SyntheticEvent).
    const bravo = cardFor("Bravo");
    const alpha = cardFor("Alpha");
    Object.defineProperty(alpha, "getBoundingClientRect", {
      value: () => ({
        top: 0, left: 0, right: 0, bottom: 100, width: 0, height: 100, x: 0, y: 0,
        toJSON: () => ({}),
      }),
    });

    fireEvent.dragStart(bravo, { dataTransfer: { effectAllowed: "" } });
    alpha.dispatchEvent(
      new MouseEvent("dragover", { bubbles: true, cancelable: true, clientY: 10 }),
    );
    fireEvent.drop(alpha, { dataTransfer: {} });

    await waitFor(() => expect(putBodies.length).toBe(1));
    const sent = putBodies[0] as { url: string; flows: UserFlow[] };
    expect(sent.url).toContain("workspace=ws");
    expect(sent.flows.map((f) => f.name)).toEqual(["Bravo", "Alpha"]);
  });
});

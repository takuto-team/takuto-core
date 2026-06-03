// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Light coverage for the Flows tab: it loads the user's list and a drag-reorder
 * persists the new order via PUT /api/me/flows.
 */

import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { render, screen, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { FlowsTab } from "./FlowsTab";
import type { UserFlow } from "../api/flows";

function flow(name: string): UserFlow {
  return { name, depends_on: [], steps: [{ name: "s", prompt: "p", skills: [] }] };
}

const INITIAL: UserFlow[] = [flow("Alpha"), flow("Bravo")];

let putBodies: unknown[];

function jsonResponse(flows: UserFlow[]): Response {
  return new Response(JSON.stringify({ flows, workspace: "ws" }), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

beforeEach(() => {
  putBodies = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_input: string, init: RequestInit = {}) => {
      if (init.method === "PUT") {
        const body = JSON.parse(init.body as string);
        putBodies.push(body);
        return jsonResponse(body.flows);
      }
      // GET (and any other read) returns the initial list.
      return jsonResponse(INITIAL);
    }),
  );
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function cardFor(name: string): HTMLElement {
  return screen.getByText(name).closest('div[draggable="true"]') as HTMLElement;
}

describe("FlowsTab", () => {
  it("renders the user's flow list", async () => {
    render(<FlowsTab />);
    await waitFor(() => expect(screen.getByText("Alpha")).toBeTruthy());
    expect(screen.getByText("Bravo")).toBeTruthy();
    expect(screen.getByText("2 / 20")).toBeTruthy();
  });

  it("a drag-reorder PUTs the list in the new order", async () => {
    render(<FlowsTab />);
    await waitFor(() => expect(screen.getByText("Alpha")).toBeTruthy());

    // Drag "Bravo" (index 1) and drop it onto "Alpha" (index 0).
    const bravo = cardFor("Bravo");
    const alpha = cardFor("Alpha");
    fireEvent.dragStart(bravo, { dataTransfer: { effectAllowed: "" } });
    fireEvent.dragOver(alpha, { dataTransfer: {} });
    fireEvent.drop(alpha, { dataTransfer: {} });

    await waitFor(() => expect(putBodies.length).toBe(1));
    const sent = putBodies[0] as { flows: UserFlow[] };
    expect(sent.flows.map((f) => f.name)).toEqual(["Bravo", "Alpha"]);
  });
});

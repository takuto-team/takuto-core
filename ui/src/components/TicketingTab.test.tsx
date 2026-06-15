// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * The Ticketing tab hosts the deployment item-polling settings inline, but only
 * when a ticketing system is selected AND the caller is an admin. Selecting
 * "None" hides the polling section immediately (gated on the live selection,
 * not the saved value) — verified here per the user's requirement.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, fireEvent } from "@testing-library/react";
import { TicketingTab } from "./TicketingTab";
import { ToastProvider } from "../hooks/useToast";

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function stubFetch(ticketingSystem: "none" | "jira" | "github") {
  (fetch as ReturnType<typeof vi.fn>).mockImplementation(async (url: string) => {
    const json = (body: unknown) =>
      new Response(JSON.stringify(body), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    if (url === "/api/config") {
      return json({
        general: { ticketing_system: ticketingSystem, poll_interval_secs: 60 },
        agent: {},
        jira: { project_keys: [], site: "", item_types: [] },
        github: { app_id: 0, app_installation_id: 0 },
        polling: {},
        web: { dashboard_username: "admin" },
        jira_available: ticketingSystem === "jira",
        ticketing_system: ticketingSystem,
        github_app_configured: false,
        repo_exists: true,
      });
    }
    if (url === "/api/users/me/credentials") {
      return json({ provider: null, github: null, jira: null });
    }
    if (url === "/api/me/flows") {
      return json({ flows: [], workspace: "takuto-core" });
    }
    return json({});
  });
}

function renderTab(isAdmin: boolean) {
  return render(
    <ToastProvider>
      <TicketingTab isAdmin={isAdmin} />
    </ToastProvider>,
  );
}

describe("TicketingTab — item polling visibility", () => {
  it("hides the polling section when ticketing system is None", async () => {
    stubFetch("none");
    renderTab(true);
    // Ticketing controls render once loaded.
    await screen.findByLabelText("Ticketing system");
    expect(screen.queryByText("Item polling")).toBeNull();
  });

  it("shows the polling section for an admin when a system is configured", async () => {
    stubFetch("jira");
    renderTab(true);
    await waitFor(() => {
      expect(screen.getByText("Item polling")).toBeTruthy();
    });
  });

  it("hides the polling section for a non-admin even when a system is configured", async () => {
    stubFetch("jira");
    renderTab(false);
    await screen.findByLabelText("Ticketing system");
    expect(screen.queryByText("Item polling")).toBeNull();
  });

  it("hides the polling section as soon as the admin selects None, before saving", async () => {
    stubFetch("jira");
    renderTab(true);
    await waitFor(() => {
      expect(screen.getByText("Item polling")).toBeTruthy();
    });
    fireEvent.change(screen.getByLabelText("Ticketing system"), {
      target: { value: "none" },
    });
    await waitFor(() => {
      expect(screen.queryByText("Item polling")).toBeNull();
    });
  });
});

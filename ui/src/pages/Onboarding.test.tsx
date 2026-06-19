// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Wizard-level coverage for the v2 enhancements:
 *   - the item-polling section embeds in the Ticketing step, gated by
 *     `isAdmin && system !== "none"` (reactive to the live selection);
 *   - the Git step is relabeled "Git & GitHub" and seeds base branch / remote
 *     from `/api/config`;
 *   - the Workflows step renders the step-timeout field (seeded from config)
 *     and the database/port note.
 *
 * Navigation between steps here uses "Skip for now" so the steps render without
 * triggering the per-step save calls (those are covered by the hook tests).
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, fireEvent } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { Onboarding } from "./Onboarding";
import { ToastProvider } from "../hooks/useToast";
import { createQueryWrapper } from "../test/queryWrapper";

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
        agent: { provider: "claude", providers: {}, step_timeout_secs: 2400 },
        git: { base_branch: "develop", remote: "upstream" },
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
    if (url.startsWith("/api/me/flows")) {
      return json({ flows: [], workspace: "takuto-core" });
    }
    if (url.startsWith("/api/repositories")) {
      return json([]);
    }
    return json({});
  });
}

function renderWizard(isAdmin: boolean) {
  const { wrapper } = createQueryWrapper();
  return render(
    <MemoryRouter>
      <ToastProvider>
        <Onboarding onLogout={() => {}} authEnabled={true} isAdmin={isAdmin} />
      </ToastProvider>
    </MemoryRouter>,
    { wrapper },
  );
}

async function skipToStep(target: number) {
  for (let i = 1; i < target; i++) {
    fireEvent.click(await screen.findByText("Skip for now"));
  }
}

describe("Onboarding wizard — item polling in the Ticketing step", () => {
  it("hides the polling section when ticketing system is None", async () => {
    stubFetch("none");
    renderWizard(true);
    await screen.findByLabelText("Ticketing system");
    expect(screen.queryByText("Item polling")).toBeNull();
  });

  it("shows the polling section for an admin when a system is configured", async () => {
    stubFetch("github");
    renderWizard(true);
    await waitFor(() => {
      expect(screen.getByText("Item polling")).toBeTruthy();
    });
  });

  it("hides the polling section for a non-admin even with a system configured", async () => {
    stubFetch("github");
    renderWizard(false);
    await screen.findByLabelText("Ticketing system");
    expect(screen.queryByText("Item polling")).toBeNull();
  });

  it("hides the polling section reactively when the admin selects None", async () => {
    stubFetch("github");
    renderWizard(true);
    await waitFor(() => expect(screen.getByText("Item polling")).toBeTruthy());
    fireEvent.change(screen.getByLabelText("Ticketing system"), {
      target: { value: "none" },
    });
    await waitFor(() => expect(screen.queryByText("Item polling")).toBeNull());
  });
});

describe("Onboarding wizard — Git & GitHub step", () => {
  it("relabels the step and seeds base branch + remote from config", async () => {
    stubFetch("none");
    renderWizard(true);
    await screen.findByLabelText("Ticketing system");
    await skipToStep(3);
    // The stepper pill + heading read "Git & GitHub".
    expect(screen.getAllByText("Git & GitHub").length).toBeGreaterThan(0);
    const baseBranch = (await screen.findByLabelText("Base branch")) as HTMLInputElement;
    const remote = screen.getByLabelText("Remote") as HTMLInputElement;
    expect(baseBranch.value).toBe("develop");
    expect(remote.value).toBe("upstream");
    expect(baseBranch.disabled).toBe(false);
  });

  it("renders the git inputs read-only for a non-admin", async () => {
    stubFetch("none");
    renderWizard(false);
    await screen.findByLabelText("Ticketing system");
    await skipToStep(3);
    const baseBranch = (await screen.findByLabelText("Base branch")) as HTMLInputElement;
    expect(baseBranch.disabled).toBe(true);
    expect(
      screen.getByText(/Only an admin can change the deployment's git settings/i),
    ).toBeTruthy();
  });
});

describe("Onboarding wizard — Workflows step", () => {
  it("renders the step-timeout field (seeded) and the database/port note", async () => {
    stubFetch("none");
    renderWizard(true);
    await screen.findByLabelText("Ticketing system");
    await skipToStep(4);
    const timeout = (await screen.findByLabelText("Timeout (seconds)")) as HTMLInputElement;
    expect(timeout.value).toBe("2400");
    expect(
      screen.getByText(/Database and dashboard port are not configured/i),
    ).toBeTruthy();
  });
});

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Wizard-level coverage. Step order is:
 *   1. Git & GitHub   — base branch / remote seeded from `/api/config`.
 *   2. Repositories   — MyRepositoriesTab.
 *   3. AI provider    — provider form + AI key.
 *   4. Ticketing      — system selector + per-repo polling section (shown once
 *                        a system is selected; repos exist from step 2).
 *   5. Workflows      — step-timeout field + database/port note.
 *
 * Navigation between steps here clicks "Save and Continue"; the per-step saves
 * resolve against the stubbed PUT endpoints (covered in detail by the hook
 * tests and Onboarding.saveOnContinue.test).
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup, fireEvent, waitFor } from "@testing-library/react";
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
    if (url.startsWith("/api/github/repos")) {
      return json([]);
    }
    if (url === "/api/me/polling-settings") {
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

// A landmark unique to each step body, used to confirm a "Save and Continue"
// click has fully advanced (its per-step save is async) before clicking again.
// Order: 1 Git&GitHub · 2 Repositories · 3 AI provider · 4 Ticketing · 5 Workflows.
const STEP_LANDMARK: Record<number, () => Promise<unknown>> = {
  1: () => screen.findByLabelText("Base branch"),
  2: () => screen.findByText("My Repositories"),
  3: () => screen.findByLabelText(/Claude API key/i),
  4: () => screen.findByLabelText("Ticketing system"),
  5: () => screen.findByLabelText("Timeout (seconds)"),
};

async function skipToStep(target: number) {
  for (let i = 1; i < target; i++) {
    fireEvent.click(await screen.findByRole("button", { name: /save and continue/i }));
    await STEP_LANDMARK[i + 1]();
  }
}

describe("Onboarding wizard — Git & GitHub step (step 1)", () => {
  it("opens on Git & GitHub and seeds base branch + remote from config", async () => {
    stubFetch("none");
    renderWizard(true);
    // Step 1 is now Git & GitHub — it renders first, no navigation needed.
    expect(screen.getAllByText("Git & GitHub").length).toBeGreaterThan(0);
    const baseBranch = (await screen.findByLabelText("Base branch")) as HTMLInputElement;
    const remote = screen.getByLabelText("Remote") as HTMLInputElement;
    // The git form seeds from /api/config one tick after the input first
    // mounts, so wait for the seeded values to land.
    await waitFor(() => expect(baseBranch.value).toBe("develop"));
    expect(remote.value).toBe("upstream");
    expect(baseBranch.disabled).toBe(false);
  });

  it("renders the git inputs read-only for a non-admin", async () => {
    stubFetch("none");
    renderWizard(false);
    const baseBranch = (await screen.findByLabelText("Base branch")) as HTMLInputElement;
    expect(baseBranch.disabled).toBe(true);
    expect(
      screen.getByText(/Only an admin can change the deployment's git settings/i),
    ).toBeTruthy();
  });
});

describe("Onboarding wizard — fixed footer", () => {
  it("keeps 'Save and Continue' enabled and offers no Skip button", async () => {
    stubFetch("none");
    renderWizard(true);
    await screen.findByLabelText("Base branch");

    const saveContinue = () =>
      screen.getByRole("button", { name: /save and continue/i }) as HTMLButtonElement;
    // Always clickable, even with nothing changed yet.
    expect(saveContinue().disabled).toBe(false);
    // The "Skip for now" affordance has been removed.
    expect(screen.queryByRole("button", { name: "Skip for now" })).toBeNull();
  });
});

describe("Onboarding wizard — Repositories step (step 2)", () => {
  it("renders the add-repositories UI before the AI / Ticketing steps", async () => {
    stubFetch("none");
    renderWizard(true);
    await screen.findByLabelText("Base branch");
    await skipToStep(2);
    expect(await screen.findByText("My Repositories")).toBeTruthy();
    expect(screen.queryByLabelText("Ticketing system")).toBeNull();
  });
});

describe("Onboarding wizard — Ticketing step (step 4)", () => {
  it("embeds the per-repo polling section once a system is selected", async () => {
    stubFetch("jira");
    renderWizard(true);
    await screen.findByLabelText("Base branch");
    await skipToStep(4);
    // The ticketing selector and the per-repo polling section render together;
    // the global general-limits / Jira-context sections stay Config-only.
    expect(await screen.findByText("Item polling")).toBeTruthy();
    expect(screen.queryByText("General limits")).toBeNull();
    expect(screen.queryByText("Jira context")).toBeNull();
  });

  it("does not embed the polling section when ticketing system is None", async () => {
    stubFetch("none");
    renderWizard(true);
    await screen.findByLabelText("Base branch");
    await skipToStep(4);
    await screen.findByLabelText("Ticketing system");
    expect(screen.queryByText("Item polling")).toBeNull();
  });
});

describe("Onboarding wizard — Workflows step (step 5)", () => {
  it("renders the step-timeout field (seeded) and the database/port note", async () => {
    stubFetch("none");
    renderWizard(true);
    await screen.findByLabelText("Base branch");
    await skipToStep(5);
    const timeout = (await screen.findByLabelText("Timeout (seconds)")) as HTMLInputElement;
    expect(timeout.value).toBe("2400");
    expect(
      screen.getByText(/Database and dashboard port are not configured/i),
    ).toBeTruthy();
  });
});

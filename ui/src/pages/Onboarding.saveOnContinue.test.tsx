// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Coverage for save-on-continue: the wizard's "Continue" button must persist
 * the per-user credential the user typed inline, not just the provider/git
 * config PUTs.
 *
 *   Step 2 — a typed AI API key is POSTed to
 *     `/api/users/me/credentials/{provider}` before the wizard advances; a
 *     blank key advances with NO credential POST; a failing save keeps the
 *     wizard on step 2.
 *   Step 3 — a typed GitHub PAT is POSTed to `/api/users/me/github-pat`;
 *     a blank PAT advances with no POST.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  render,
  screen,
  waitFor,
  cleanup,
  fireEvent,
} from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { Onboarding } from "./Onboarding";
import { ToastProvider } from "../hooks/useToast";
import { createQueryWrapper } from "../test/queryWrapper";

interface RecordedCall {
  url: string;
  body: unknown;
}

let calls: RecordedCall[] = [];

beforeEach(() => {
  calls = [];
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

/**
 * Stub every endpoint the wizard touches. `failCredential` forces the
 * per-user AI credential POST to return 400 so we can assert the wizard
 * blocks forward navigation on save failure.
 */
function stubFetch(opts: { failCredential?: boolean; failPat?: boolean } = {}) {
  (fetch as ReturnType<typeof vi.fn>).mockImplementation(
    async (url: string, init?: RequestInit) => {
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        });

      if (init?.body) {
        let parsed: unknown = null;
        try {
          parsed = JSON.parse(init.body as string);
        } catch {
          parsed = init.body;
        }
        calls.push({ url, body: parsed });
      } else if (init?.method && init.method !== "GET") {
        calls.push({ url, body: null });
      }

      if (url === "/api/config") {
        return json({
          general: { ticketing_system: "none", poll_interval_secs: 60 },
          agent: { provider: "claude", providers: {}, step_timeout_secs: 1800 },
          git: { base_branch: "main", remote: "origin" },
          jira: { project_keys: [], site: "", item_types: [] },
          github: { app_id: 0, app_installation_id: 0 },
          polling: {},
          web: { dashboard_username: "admin" },
          jira_available: false,
          ticketing_system: "none",
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
      if (url === "/api/config/agent") return json({});
      if (url === "/api/config/git") return json({});
      if (url.startsWith("/api/users/me/credentials/")) {
        return opts.failCredential
          ? json({ error: "invalid_api_key", message: "bad key" }, 400)
          : json({ provider: null, github: null });
      }
      if (url === "/api/users/me/github-pat") {
        return opts.failPat
          ? json({ error: "gh_transport_error", message: "could not reach github" }, 502)
          : json({ provider: null, github: { login: "octocat", scopes: ["repo"], attribute_commits: true } });
      }
      if (url === "/api/config/polling" || url === "/api/config/jira") {
        return json({
          general: { ticketing_system: "github", poll_interval_secs: 60 },
          agent: {},
          jira: { project_keys: [], site: "", item_types: [] },
          github: { app_id: 0, app_installation_id: 0 },
          polling: {},
          web: {},
          persisted: true,
        });
      }
      return json({});
    },
  );
}

function renderWizard() {
  const { wrapper } = createQueryWrapper();
  return render(
    <MemoryRouter>
      <ToastProvider>
        <Onboarding onLogout={() => {}} authEnabled={true} isAdmin={true} />
      </ToastProvider>
    </MemoryRouter>,
    { wrapper },
  );
}

async function continueButton() {
  return (await screen.findByRole("button", { name: /Continue/i })) as HTMLButtonElement;
}

/** Click "Save and Continue" once and wait for the click's async work to settle. */
async function clickContinue() {
  fireEvent.click(await continueButton());
}

/** Click "Skip for now" — advances without saving (used for pure-advance steps
 *  where the primary button is dirty-gated and disabled with no changes). */
async function clickSkip() {
  fireEvent.click(await screen.findByRole("button", { name: "Skip for now" }));
}

/** Advance from step 1 to step 2 via Skip (ticketing = none, nothing dirty). */
async function goToStep2() {
  await screen.findByLabelText("Ticketing system");
  await clickSkip();
  await screen.findByLabelText(/Claude API key/i);
}

const credentialPosts = () =>
  calls.filter((c) => c.url.startsWith("/api/users/me/credentials/"));
const patPosts = () => calls.filter((c) => c.url === "/api/users/me/github-pat");
const gitPuts = () => calls.filter((c) => c.url === "/api/config/git");
const pollingPuts = () => calls.filter((c) => c.url === "/api/config/polling");

describe("Onboarding — save-on-continue, step 1 (item polling)", () => {
  it("persists the item-polling section (e.g. disabling polling) on Continue", async () => {
    stubFetch();
    renderWizard();

    // Select a ticketing system so the admin-only polling section renders.
    const select = await screen.findByLabelText("Ticketing system");
    fireEvent.change(select, { target: { value: "github" } });

    // Toggle "Enable item polling" off (defaults on). Generous timeout: the
    // polling section mounts its own data-loading component, and under the full
    // parallel test gate (CPU contention) the default 1s findBy can flake.
    const toggle = await screen.findByRole(
      "switch",
      { name: "Enable item polling" },
      { timeout: 5000 },
    );
    fireEvent.click(toggle);

    await clickContinue();

    // The polling section must be saved by Continue — not left unsaved as
    // before — carrying the disabled flag through to the backend.
    await waitFor(() => {
      expect(pollingPuts().length).toBeGreaterThanOrEqual(1);
    });
    expect(pollingPuts()[0].body).toMatchObject({ auto_polling: false });
  });
});

describe("Onboarding — save-on-continue, step 2 (AI key)", () => {
  it("POSTs the typed API key to the provider credential endpoint before advancing", async () => {
    stubFetch();
    renderWizard();
    await goToStep2();

    const keyInput = await screen.findByLabelText(/Claude API key/i);
    fireEvent.change(keyInput, { target: { value: "sk-ant-test-key" } });

    await clickContinue();

    await waitFor(() => {
      expect(credentialPosts().length).toBe(1);
    });
    expect(credentialPosts()[0].url).toContain("/api/users/me/credentials/claude");
    expect(credentialPosts()[0].body).toMatchObject({ api_key: "sk-ant-test-key" });

    // Wizard advanced to step 3 (Git & GitHub).
    await waitFor(() => {
      expect(screen.getByLabelText("Base branch")).toBeTruthy();
    });
  });

  it("advances with NO credential POST when the API key is blank", async () => {
    stubFetch();
    renderWizard();
    await goToStep2();

    // Do not type a key — "Save and Continue" is disabled, so Skip advances.
    await clickSkip();

    await waitFor(() => {
      expect(screen.getByLabelText("Base branch")).toBeTruthy();
    });
    expect(credentialPosts().length).toBe(0);
  });

  it("stays on step 2 when the credential save fails", async () => {
    stubFetch({ failCredential: true });
    renderWizard();
    await goToStep2();

    const keyInput = await screen.findByLabelText(/Claude API key/i);
    fireEvent.change(keyInput, { target: { value: "sk-ant-bad" } });

    await clickContinue();

    // The POST was attempted but failed → wizard must NOT advance.
    await waitFor(() => {
      expect(credentialPosts().length).toBe(1);
    });
    // Still on step 2: the AI key field is present, the Git step is not.
    expect(screen.getByLabelText(/Claude API key/i)).toBeTruthy();
    expect(screen.queryByLabelText("Base branch")).toBeNull();
  });
});

describe("Onboarding — save-on-continue, step 3 (GitHub PAT)", () => {
  async function goToStep3() {
    await goToStep2();
    // Step 2 → 3 with a blank key (Skip; "Save and Continue" is dirty-gated).
    await clickSkip();
    await screen.findByLabelText("Base branch");
  }

  it("POSTs the typed PAT to the github-pat endpoint before advancing", async () => {
    stubFetch();
    renderWizard();
    await goToStep3();

    const patInput = await screen.findByLabelText(/Personal access token/i);
    fireEvent.change(patInput, { target: { value: "ghp_testtoken" } });

    await clickContinue();

    await waitFor(() => {
      expect(patPosts().length).toBe(1);
    });
    expect(patPosts()[0].body).toMatchObject({ pat: "ghp_testtoken" });
  });

  it("advances with no PAT POST when the field is blank", async () => {
    stubFetch();
    renderWizard();
    await goToStep3();

    // Blank PAT and blank git edits → "Save and Continue" is disabled; Skip.
    await clickSkip();

    // Advanced to step 4 (Repositories step — the embedded MyRepositoriesTab).
    await waitFor(() => {
      expect(screen.getByText("My Repositories")).toBeTruthy();
    });
    expect(patPosts().length).toBe(0);
  });

  it("does NOT save git settings when the PAT save fails (no misleading success)", async () => {
    stubFetch({ failPat: true });
    renderWizard();
    await goToStep3();

    const patInput = await screen.findByLabelText(/Personal access token/i);
    fireEvent.change(patInput, { target: { value: "ghp_bad" } });

    await clickContinue();

    // The PAT POST was attempted and failed.
    await waitFor(() => {
      expect(patPosts().length).toBe(1);
    });
    // Git settings must NOT have been saved — the PAT save runs first and
    // blocks the step, so the user never sees a "Git settings saved." toast
    // alongside the PAT error.
    expect(gitPuts().length).toBe(0);
    // Still on step 3 (Git step visible, step 4 timeout field absent).
    expect(screen.getByLabelText("Base branch")).toBeTruthy();
    expect(screen.queryByLabelText("Timeout (seconds)")).toBeNull();
  });
});

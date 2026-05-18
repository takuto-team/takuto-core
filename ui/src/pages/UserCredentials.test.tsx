// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Phase 2 regression guards.
 *
 * Two specific behaviors are non-negotiable per the architecture amendments
 * and the team-lead's dispatch:
 *
 *   A1 — The Cursor card MUST NOT mention ttyd / browser flows. Cursor is
 *        **API-key only** in v1 (per 04_architecture.md amendment A1).
 *   A3 — The per-user toggle MUST be **"Attribute commits to me"**, NOT
 *        "Sign commits". v1 does NOT do GPG/SSH cryptographic signing.
 *
 * Both guards live here as standalone tests so any future renderer change
 * (component split, copy tweak, third-party library swap) trips them
 * immediately.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  render,
  screen,
  waitFor,
  within,
  cleanup,
  fireEvent,
} from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { UserCredentials } from "./UserCredentials";
import { ToastProvider } from "../hooks/useToast";
import {
  clearMocksOverride,
  resetMocks,
  setMocksEnabled,
} from "../api/mocks";
import type { UserCredentialsStatus } from "../api/types";

function renderPage() {
  return render(
    <ToastProvider>
      <MemoryRouter>
        <UserCredentials onLogout={vi.fn()} authEnabled />
      </MemoryRouter>
    </ToastProvider>,
  );
}

/**
 * Stub `/api/auth/status` (the page also fetches it via `apiJson`). We
 * intercept ONLY that URL and let the mock layer answer the credential
 * endpoints — so the rest of the page renders against the documented
 * contract, not raw fetch mocks.
 *
 * `githubMode` parameterises the effective GitHub mode (per #29 it lives on
 * /api/auth/status, NOT on the per-user credentials response).
 */
function stubAuthStatus(provider: string, githubMode: string = "app") {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string) => {
      if (typeof input === "string" && input.startsWith("/api/auth/status")) {
        return new Response(
          JSON.stringify({
            dashboard_auth_enabled: true,
            multi_user: true,
            setup_required: false,
            provider_selected: provider,
            github_mode: githubMode,
            degraded: false,
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        );
      }
      // Everything else: 404 — credential endpoints flow through the mock
      // layer, not fetch.
      return new Response("not found", { status: 404 });
    }),
  );
}

/**
 * Baseline status: no provider credential, no GitHub PAT. `github: null`
 * matches the backend's `Option<GithubCredentialStatus>` wire shape (see
 * routes/credentials.rs::UserCredentialsStatus).
 */
const BLANK_STATUS: UserCredentialsStatus = {
  provider: null,
  github: null,
};

beforeEach(() => {
  setMocksEnabled(true);
  resetMocks(BLANK_STATUS);
});

afterEach(() => {
  cleanup();
  clearMocksOverride();
  vi.restoreAllMocks();
});

describe("UserCredentials — A1 regression (Cursor is API-key only)", () => {
  it("Cursor card shows the API-key copy AND never mentions ttyd / browser flows", async () => {
    stubAuthStatus("cursor");
    renderPage();

    // The page resolves /api/auth/status → provider_selected = cursor, then
    // renders the Cursor AI card. The API-key helper text is the canonical
    // copy from `providerHelper("cursor")`.
    await waitFor(() => {
      expect(screen.getByText(/AI provider — Cursor/i)).toBeTruthy();
    });
    expect(screen.getByText(/cursor.com\/dashboard/i)).toBeTruthy();

    // A1 forbidden vocabulary. Each pattern is matched case-insensitively
    // against the *entire* body so a future copy change can't sneak any of
    // them past review.
    const body = document.body.textContent ?? "";
    const banned = [
      /ttyd/i,
      /\bbrowser flow\b/i,
      /\bdevice login\b/i,
      /\binteractive terminal\b/i,
      /\bsign in to cursor\b/i,
      /\bcli capture\b/i,
    ];
    for (const re of banned) {
      expect(re.test(body)).toBe(false);
    }
  });
});

describe("UserCredentials — wire-format regression (#28)", () => {
  it("renders the ✅ Connected pill when the server returns { provider, active } (the real wire shape)", async () => {
    stubAuthStatus("claude");
    // Hard-coded from `routes/credentials.rs::ProviderCredentialStatus` so a
    // future Rust rename trips the typecheck or this test, not the user.
    resetMocks({
      provider: {
        provider: "claude",
        kind: "api_key",
        active: true,
        last_validated_at: "2026-05-19T08:00:00Z",
        last_used_at: null,
      },
      // GitHub absent — `null` matches the backend's Option<> wire shape.
      github: null,
    });
    renderPage();

    // The pill should read "Connected" — NOT "Not connected". This is the
    // exact regression the user reported: row saved, audit logged, but pill
    // stuck at "not connected" because the UI read `provider_name`/`valid`.
    await waitFor(() => {
      expect(screen.getByText(/AI provider — Claude/i)).toBeTruthy();
    });
    // Disambiguate from the GitHub card's pill (which is also "Connected"
    // in some states) by scoping the query to the AI card via the heading.
    const aiHeading = screen.getByText(/AI provider — Claude/i);
    const aiSection = aiHeading.closest("section");
    expect(aiSection).toBeTruthy();
    expect(
      within(aiSection!).getByText(/^Connected$/i),
    ).toBeTruthy();
    expect(
      within(aiSection!).queryByText(/^Not connected$/i),
    ).toBeNull();
  });

  it("renders the ⚠ Not connected pill when the row is inactive (post-provider-switch)", async () => {
    stubAuthStatus("claude");
    // `active: false` → the row was deactivated by a provider switch and
    // must NOT count as a live credential, even though it's still in the
    // DB. Per 04_architecture.md §2.4.
    resetMocks({
      provider: {
        provider: "claude",
        kind: "api_key",
        active: false,
        last_validated_at: "2026-05-19T08:00:00Z",
        last_used_at: null,
      },
      // GitHub absent — `null` matches the backend's Option<> wire shape.
      github: null,
    });
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/AI provider — Claude/i)).toBeTruthy();
    });
    const aiSection = screen.getByText(/AI provider — Claude/i).closest("section");
    expect(within(aiSection!).getByText(/^Not connected$/i)).toBeTruthy();
  });
});

describe("UserCredentials — wire-format regression #29 (GitHub side)", () => {
  it("renders 'Connected' on the GitHub card when github = { login, scopes, attribute_commits, last_validated_at } (real wire shape)", async () => {
    stubAuthStatus("claude", "app_plus_pat");
    // Hard-coded from `routes/credentials.rs::GithubCredentialStatus` (no
    // `has_pat`, no `mode`). The presence of the row means hasPat = true;
    // the effective mode comes from /api/auth/status::github_mode.
    resetMocks({
      provider: null,
      github: {
        login: "alice",
        scopes: ["repo", "read:org"],
        attribute_commits: true,
        last_validated_at: "2026-05-19T08:00:00Z",
      },
    });
    renderPage();

    await waitFor(() => {
      expect(screen.getByText(/^GitHub$/i)).toBeTruthy();
    });
    // Scope to the GitHub section so we don't pick up the AI card's pill.
    const ghSection = screen.getByText(/^GitHub$/i).closest("section");
    expect(ghSection).toBeTruthy();
    expect(within(ghSection!).getByText(/^Connected$/i)).toBeTruthy();
    expect(within(ghSection!).queryByText(/^Not connected$/i)).toBeNull();
    // The login should also be surfaced — confirms the panel rendered the
    // PAT-present branch, not the "no PAT" CTA.
    expect(within(ghSection!).getByText(/alice/)).toBeTruthy();
  });

  it("renders 'Not connected' on the GitHub card when github = null in PAT-only mode", async () => {
    // Mode C: no shared App, no user PAT → must show "Not connected".
    stubAuthStatus("claude", "pat_only");
    resetMocks({
      provider: null,
      github: null,
    });
    renderPage();

    await waitFor(() => {
      expect(screen.getByText(/^GitHub$/i)).toBeTruthy();
    });
    const ghSection = screen.getByText(/^GitHub$/i).closest("section");
    expect(within(ghSection!).getByText(/^Not connected$/i)).toBeTruthy();
  });

  it("renders 'Connected' (App-only path) when github = null but mode is 'app'", async () => {
    // Mode A: shared App handles everything → the pill should still read
    // "Connected" because Maestro can talk to GitHub via the App. This
    // preserves the pre-existing logic in describeMode().
    stubAuthStatus("claude", "app");
    resetMocks({
      provider: null,
      github: null,
    });
    renderPage();

    await waitFor(() => {
      expect(screen.getByText(/^GitHub$/i)).toBeTruthy();
    });
    const ghSection = screen.getByText(/^GitHub$/i).closest("section");
    expect(within(ghSection!).getByText(/^Connected$/i)).toBeTruthy();
  });
});

describe("UserCredentials — A3 regression (Attribute commits, not Sign commits)", () => {
  it("renders the toggle as 'Attribute commits to me' and never says 'Sign commits'", async () => {
    stubAuthStatus("claude", "app_plus_pat");
    resetMocks({
      provider: null,
      // Wire shape: no `has_pat`, no `mode` — see #29.
      github: {
        login: "alice",
        scopes: ["repo"],
        attribute_commits: true,
        last_validated_at: "2026-05-19T08:00:00Z",
      },
    });
    renderPage();

    // The label is what the screen reader will pick up via for/id linkage.
    const toggle = await waitFor(() =>
      screen.getByLabelText(/attribute commits to me/i),
    );
    expect(toggle).toBeTruthy();

    // Belt-and-braces: the entire body must not contain "Sign commits" or
    // "GPG sign" anywhere — copy or aria-label.
    const body = document.body.textContent ?? "";
    expect(/sign commits/i.test(body)).toBe(false);
    expect(/gpg sign/i.test(body)).toBe(false);
    expect(/ssh sign/i.test(body)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// #31 issue A + B — Rotate / Disconnect / Remove-PAT buttons removed.
// ---------------------------------------------------------------------------

describe("UserCredentials — #31 issue A + B: no Rotate / Disconnect / Remove-PAT buttons", () => {
  it("AI provider card never renders Rotate or Disconnect buttons (connected state)", async () => {
    stubAuthStatus("claude");
    resetMocks({
      provider: {
        provider: "claude",
        kind: "api_key",
        active: true,
        last_validated_at: "2026-05-19T08:00:00Z",
        last_used_at: null,
      },
      github: null,
    });
    renderPage();

    await waitFor(() => {
      expect(screen.getByText(/AI provider — Claude/i)).toBeTruthy();
    });
    const aiSection = screen.getByText(/AI provider — Claude/i).closest("section");
    expect(within(aiSection!).queryByRole("button", { name: /^rotate( key)?$/i })).toBeNull();
    expect(within(aiSection!).queryByRole("button", { name: /^disconnect$/i })).toBeNull();
    // The single "Replace" / "Save" button must still exist.
    const saveBtn = within(aiSection!).getByRole("button", { name: /^(save|replace)$/i });
    expect(saveBtn).toBeTruthy();
  });

  it("GitHub card never renders a 'Remove PAT' button (even when a PAT is saved)", async () => {
    stubAuthStatus("claude", "app_plus_pat");
    resetMocks({
      provider: null,
      github: {
        login: "alice",
        scopes: ["repo"],
        attribute_commits: true,
        last_validated_at: "2026-05-19T08:00:00Z",
      },
    });
    renderPage();

    await waitFor(() => {
      expect(screen.getByText(/^GitHub$/i)).toBeTruthy();
    });
    const ghSection = screen.getByText(/^GitHub$/i).closest("section");
    expect(
      within(ghSection!).queryByRole("button", { name: /remove pat/i }),
    ).toBeNull();
    expect(
      within(ghSection!).queryByRole("button", { name: /^disconnect$/i }),
    ).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// #31 issue C — pill flips synchronously on save without manual refresh.
// ---------------------------------------------------------------------------

describe("UserCredentials — #31 issue C: pill flips to Connected synchronously on save", () => {
  it("after a successful POST + refresh, the pill renders 'Connected' WITHOUT a manual page refresh", async () => {
    stubAuthStatus("claude");
    // Start with NO credential — pill should be "Not connected".
    resetMocks({ provider: null, github: null });
    renderPage();

    // Wait for initial load to settle and the form to mount.
    const inputId = await waitFor(() =>
      screen.getByLabelText(/Claude API key/i),
    );
    const aiSection = screen.getByText(/AI provider — Claude/i).closest("section")!;
    expect(within(aiSection).getByText(/^Not connected$/i)).toBeTruthy();

    // Type a key and click Save. NOTE: do NOT trigger any extra refresh /
    // re-render — the test verifies the pill flips on its own.
    fireEvent.change(inputId, { target: { value: "sk-test-123" } });
    const saveBtn = within(aiSection).getByRole("button", { name: /^save$/i });
    fireEvent.click(saveBtn);

    // The mock layer transitions state synchronously on the POST handler;
    // the page's refresh() then re-reads it. waitFor lets React flush the
    // post-save state update + re-render before we assert.
    await waitFor(() => {
      // Re-query the section because React replaced its children on
      // re-render — the closure-captured reference may be stale.
      const section = screen.getByText(/AI provider — Claude/i).closest("section")!;
      expect(within(section).getByText(/^Connected$/i)).toBeTruthy();
    });

    // Belt-and-braces: the "Not connected" pill must be gone from the AI
    // card after the save completes.
    const finalSection = screen
      .getByText(/AI provider — Claude/i)
      .closest("section")!;
    expect(within(finalSection).queryByText(/^Not connected$/i)).toBeNull();
  });

  it("the page does NOT show a 'Loading…' state during a save-triggered refetch", async () => {
    stubAuthStatus("claude");
    resetMocks({ provider: null, github: null });
    renderPage();

    // Settle initial load.
    const input = await waitFor(() =>
      screen.getByLabelText(/Claude API key/i),
    );

    fireEvent.change(input, { target: { value: "sk-test" } });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

    // The card must stay mounted across save — i.e. no full "Loading…"
    // takeover that hides the panel. The visible "Saving…" label inside
    // the paste field is fine (that's local to the field), but the
    // page-level Loading… branch must not re-fire.
    //
    // We assert by checking that the AI section heading is continuously
    // visible while the save resolves.
    await waitFor(() => {
      expect(screen.getByText(/AI provider — Claude/i)).toBeTruthy();
    });
    // No page-level "Loading…" text should exist at this point.
    expect(screen.queryByText(/^Loading…$/i)).toBeNull();
  });
});

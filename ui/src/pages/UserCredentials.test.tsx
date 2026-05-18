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
import { render, screen, waitFor, within, cleanup } from "@testing-library/react";
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

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Regression guards for the per-user GitHub credentials section (its own tab):
 *
 *   #29 — The "Connected / Not connected" pill is driven by the effective
 *         GitHub mode on `/api/auth/status::github_mode` (App-only still reads
 *         "Connected"), NOT by the per-user credentials response alone.
 *   A3  — The per-user toggle MUST be **"Attribute commits to me"**, NOT
 *         "Sign commits". v1 does NOT do GPG/SSH cryptographic signing.
 *   #31 — No "Remove PAT" / "Disconnect" buttons (single Save/Replace flow).
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, within, cleanup } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { GitHubCredentialsSection } from "./GitHubCredentialsSection";
import { ToastProvider } from "../../hooks/useToast";
import { clearMocksOverride, resetMocks, setMocksEnabled } from "../../api/mocks";
import type { UserCredentialsStatus } from "../../api/types";

function renderPage() {
  return render(
    <ToastProvider>
      <MemoryRouter>
        <GitHubCredentialsSection />
      </MemoryRouter>
    </ToastProvider>,
  );
}

/**
 * Stub `/api/auth/status` (the section fetches it via `apiJson`). We intercept
 * ONLY that URL and let the mock layer answer the credential endpoints.
 *
 * `githubMode` parameterises the effective GitHub mode (per #29 it lives on
 * /api/auth/status, NOT on the per-user credentials response).
 */
function stubAuthStatus(githubMode: string = "app") {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string) => {
      if (typeof input === "string" && input.startsWith("/api/auth/status")) {
        return new Response(
          JSON.stringify({
            dashboard_auth_enabled: true,
            multi_user: true,
            setup_required: false,
            provider_selected: "claude",
            github_mode: githubMode,
            degraded: false,
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        );
      }
      return new Response("not found", { status: 404 });
    }),
  );
}

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

describe("GitHubCredentialsSection — wire-format regression #29", () => {
  it("renders 'Token provided' when github = { login, scopes, attribute_commits, last_validated_at } (real wire shape)", async () => {
    stubAuthStatus("app_plus_pat");
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
    const ghSection = screen.getByText(/^GitHub$/i).closest("section");
    expect(ghSection).toBeTruthy();
    // A per-user PAT is present → "Token provided" (not the App-level "Connected").
    expect(within(ghSection!).getByText(/^Token provided$/i)).toBeTruthy();
    expect(within(ghSection!).queryByText(/^Not connected$/i)).toBeNull();
    // Login surfaced confirms the PAT-present branch rendered, not the CTA.
    expect(within(ghSection!).getByText(/alice/)).toBeTruthy();
  });

  it("renders 'Not connected' when github = null in PAT-only mode", async () => {
    stubAuthStatus("pat_only");
    resetMocks({ provider: null, github: null });
    renderPage();

    await waitFor(() => {
      expect(screen.getByText(/^GitHub$/i)).toBeTruthy();
    });
    const ghSection = screen.getByText(/^GitHub$/i).closest("section");
    expect(within(ghSection!).getByText(/^Not connected$/i)).toBeTruthy();
  });

  it("renders 'Connected' (App-only path) when github = null but mode is 'app'", async () => {
    stubAuthStatus("app");
    resetMocks({ provider: null, github: null });
    renderPage();

    await waitFor(() => {
      expect(screen.getByText(/^GitHub$/i)).toBeTruthy();
    });
    const ghSection = screen.getByText(/^GitHub$/i).closest("section");
    expect(within(ghSection!).getByText(/^Connected$/i)).toBeTruthy();
  });
});

describe("GitHubCredentialsSection — A3 regression (Attribute commits, not Sign commits)", () => {
  it("renders the toggle as 'Attribute commits to me' and never says 'Sign commits'", async () => {
    stubAuthStatus("app_plus_pat");
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

    const toggle = await waitFor(() =>
      screen.getByLabelText(/attribute commits to me/i),
    );
    expect(toggle).toBeTruthy();

    const body = document.body.textContent ?? "";
    expect(/sign commits/i.test(body)).toBe(false);
    expect(/gpg sign/i.test(body)).toBe(false);
    expect(/ssh sign/i.test(body)).toBe(false);
  });
});

describe("GitHubCredentialsSection — #31: no 'Remove PAT' / 'Disconnect' buttons", () => {
  it("never renders a 'Remove PAT' button (even when a PAT is saved)", async () => {
    stubAuthStatus("app_plus_pat");
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

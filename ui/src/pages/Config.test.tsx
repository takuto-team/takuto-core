// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Config page — the single page-level Save footer.
 *
 * The fixed SettingsFooter (one "Save changes" button) renders only on the
 * form-like tabs (AI Settings, Ticketing, Repository Settings, GitHub). The
 * action-based tabs (Security, Users, My Repositories, Workflows) keep their
 * own discrete buttons and show no footer.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Config } from "./Config";
import { ToastProvider } from "../hooks/useToast";
import { setMocksEnabled, resetMocks, clearMocksOverride } from "../api/mocks";

function baseConfig(): unknown {
  return {
    general: { ticketing_system: "none" },
    agent: {
      provider: "claude",
      available_providers: ["claude", "cursor", "codex", "opencode"],
      share_conversation_across_steps: false,
      providers: { claude: { model: "", base_url: "", extra_args: [] } },
    },
    jira: { project_keys: [], site: "" },
    github: { app_id: 0, app_installation_id: 0 },
    web: { dashboard_username: "" },
    jira_available: false,
    ticketing_system: "none",
    github_app_configured: false,
    repo_exists: true,
  };
}

beforeEach(() => {
  setMocksEnabled(true);
  resetMocks({ provider: null, github: null });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string) => {
      const url = typeof input === "string" ? input : String(input);
      const json = (b: unknown) => new Response(JSON.stringify(b), { status: 200 });
      if (url.startsWith("/api/auth/status"))
        return json({ dashboard_auth_enabled: true, multi_user: true, provider_selected: "claude", github_mode: "app" });
      if (url === "/api/config") return json(baseConfig());
      if (url === "/api/users") return json([]);
      if (url.startsWith("/api/users/me/credentials")) return json({ provider: null, github: null, jira: null });
      if (url.startsWith("/api/me/flows")) return json({ flows: [], workspace: "ws" });
      if (url.startsWith("/api/repositories")) return json([]);
      return json({});
    }),
  );
});

afterEach(() => {
  cleanup();
  clearMocksOverride();
  vi.restoreAllMocks();
});

function renderConfig() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <ToastProvider>
        <MemoryRouter>
          <Config onLogout={() => {}} authEnabled isAdmin />
        </MemoryRouter>
      </ToastProvider>
    </QueryClientProvider>,
  );
}

const footerSave = () => screen.queryByRole("button", { name: /^save changes$/i });
const tabButton = (name: RegExp) => screen.getByRole("button", { name });

describe("Config — single page-level Save footer", () => {
  it("shows the footer Save on form tabs and hides it on action tabs", async () => {
    renderConfig();

    // Default tab is "Security" (action tab) → no footer Save.
    await screen.findByText(/change password/i);
    expect(footerSave()).toBeNull();

    // Switch to "AI Settings" (form tab) → the single footer Save appears.
    fireEvent.click(tabButton(/^AI Settings$/));
    await waitFor(() => expect(footerSave()).not.toBeNull());

    // Switch to "Users" (action tab) → footer gone again.
    fireEvent.click(tabButton(/^Users$/));
    await waitFor(() => expect(footerSave()).toBeNull());
  });
});

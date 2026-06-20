// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * The AI Settings tab must expose a SINGLE "Save changes" button for the admin
 * config sections (provider + share + guardrails) — the multi-button layout is
 * what let a user miss saving a provider switch.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AiSettingsTab } from "./AiSettingsTab";
import { ToastProvider, ToastContainer } from "../hooks/useToast";
import {
  clearMocksOverride,
  resetMocks,
  setMocksEnabled,
} from "../api/mocks";
import type { UserCredentialsStatus } from "../api/types";

const BLANK_STATUS: UserCredentialsStatus = { provider: null, github: null };

function baseConfig(): unknown {
  return {
    general: { ticketing_system: "none" },
    agent: {
      provider: "claude",
      available_providers: ["claude", "cursor", "codex", "opencode"],
      share_conversation_across_steps: false,
      providers: {
        claude: { model: "", base_url: "", extra_args: [], allow_shared_default: false },
      },
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

let putCount = 0;

beforeEach(() => {
  putCount = 0;
  setMocksEnabled(true);
  resetMocks(BLANK_STATUS);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init?: RequestInit) => {
      if (typeof input === "string" && input.startsWith("/api/auth/status")) {
        return new Response(
          JSON.stringify({
            dashboard_auth_enabled: true,
            multi_user: true,
            setup_required: false,
            provider_selected: "claude",
            github_mode: "app",
            degraded: false,
          }),
          { status: 200 },
        );
      }
      if (typeof input === "string" && input === "/api/config" && (!init || !init.method)) {
        return new Response(JSON.stringify(baseConfig()), { status: 200 });
      }
      if (typeof input === "string" && input === "/api/config/agent" && init?.method === "PUT") {
        putCount += 1;
        return new Response(JSON.stringify({ ...(baseConfig() as object), persisted: true }), {
          status: 200,
        });
      }
      return new Response("not found", { status: 404 });
    }),
  );
});

afterEach(() => {
  cleanup();
  clearMocksOverride();
  vi.restoreAllMocks();
});

function renderTab() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <ToastProvider>
        <MemoryRouter>
          <AiSettingsTab isAdmin />
          <ToastContainer />
        </MemoryRouter>
      </ToastProvider>
    </QueryClientProvider>,
  );
}

describe("AiSettingsTab — single Save button", () => {
  it("renders exactly one 'Save changes' button", async () => {
    renderTab();
    await waitFor(() => expect(document.getElementById("model-input")).toBeTruthy());
    const buttons = screen.getAllByRole("button", { name: /^save changes$/i });
    expect(buttons.length).toBe(1);
  });

  it("Save is disabled when clean, enables after a config edit, and PUTs once", async () => {
    renderTab();
    await waitFor(() => expect(document.getElementById("model-input")).toBeTruthy());

    const save = screen.getByRole("button", { name: /^save changes$/i }) as HTMLButtonElement;
    expect(save.disabled).toBe(true);

    fireEvent.change(document.getElementById("model-input") as HTMLInputElement, {
      target: { value: "claude-opus-4-8" },
    });
    await waitFor(() => expect(save.disabled).toBe(false));

    fireEvent.click(save);
    await waitFor(() => expect(putCount).toBe(1));
  });
});

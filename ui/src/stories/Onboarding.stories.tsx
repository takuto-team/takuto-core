// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { useEffect, useState, type ReactNode } from "react";
import { fn } from "storybook/test";
import { MemoryRouter } from "react-router-dom";
import { Onboarding } from "../pages/Onboarding";
import { ToastProvider } from "../hooks/useToast";

interface ApiMock {
  /** Saved ticketing system surfaced by GET /api/config. */
  ticketingSystem?: "none" | "jira" | "github";
  /** When set, GET /api/users/me/credentials reports a connected Jira account. */
  jiraConnected?: { site: string; email: string };
}

/**
 * Patches `window.fetch` for the endpoints the wizard touches so its
 * data-fetching steps (ticketing credentials, AI key, GitHub PAT, flows)
 * render deterministically without a backend. Installed in a `useState`
 * initializer so the patch lands before child mount effects fire — mirrors
 * `FlowsTab.stories.tsx`.
 */
function withApiMock(mock: ApiMock = {}) {
  return function Decorator(Story: () => ReactNode) {
    const [realFetch] = useState(() => {
      const original = window.fetch;
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        });

      window.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.toString();
        const method = (init?.method ?? "GET").toUpperCase();

        if (url === "/api/config" && method === "GET") {
          return json({
            general: { ticketing_system: mock.ticketingSystem ?? "none" },
            agent: { provider: "claude", providers: {} },
            jira: { project_keys: [], site: "" },
            github: { app_id: 0, app_installation_id: 0 },
            web: { dashboard_username: "admin" },
            jira_available: false,
            ticketing_system: mock.ticketingSystem ?? "none",
            github_app_configured: false,
            repo_exists: true,
          });
        }
        if (url === "/api/users/me/credentials" && method === "GET") {
          return json({
            provider: null,
            github: null,
            jira: mock.jiraConnected
              ? {
                  site: mock.jiraConnected.site,
                  email: mock.jiraConnected.email,
                  account_id: "acc-1",
                  account_name: "Demo User",
                  last_validated_at: null,
                }
              : null,
          });
        }
        if (url === "/api/auth/status" && method === "GET") {
          return json({
            dashboard_auth_enabled: true,
            multi_user: true,
            setup_required: false,
            github_mode: "missing",
          });
        }
        if (url === "/api/me/flows" && method === "GET") {
          return json({ flows: [], workspace: "takuto-core" });
        }
        // Writes the wizard issues on Continue / Finish.
        if (
          method !== "GET" &&
          (url === "/api/config" ||
            url === "/api/config/agent" ||
            url.startsWith("/api/users/me/credentials") ||
            url === "/api/users/me/jira-credential" ||
            url === "/api/users/me/github-pat" ||
            url === "/api/onboarding/complete")
        ) {
          if (url === "/api/users/me/jira-credential") {
            return json({
              site: mock.jiraConnected?.site ?? "https://demo.atlassian.net",
              email: mock.jiraConnected?.email ?? "you@demo.com",
              account: { account_id: "acc-1", display_name: "Demo User" },
            });
          }
          return json({});
        }
        return original(input as RequestInfo, init);
      }) as typeof window.fetch;

      return original;
    });

    useEffect(() => {
      return () => {
        window.fetch = realFetch;
      };
    }, [realFetch]);

    return <Story />;
  };
}

const meta = {
  title: "Pages/Onboarding",
  component: Onboarding,
  parameters: {
    layout: "fullscreen",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  decorators: [
    (Story) => (
      <ToastProvider>
        <MemoryRouter>
          <Story />
        </MemoryRouter>
      </ToastProvider>
    ),
  ],
  args: {
    onLogout: fn(),
    authEnabled: true,
  },
} satisfies Meta<typeof Onboarding>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Fresh install: no ticketing system selected yet. Click through
 * Continue / Skip to walk the four steps within a single story.
 */
export const FreshInstall: Story = {
  name: "Step 1 — fresh install (no ticketing system)",
  decorators: [withApiMock()],
};

export const WithJiraTicketing: Story = {
  name: "Step 1 — Jira already connected",
  decorators: [
    withApiMock({
      ticketingSystem: "jira",
      jiraConnected: { site: "https://acme.atlassian.net", email: "dev@acme.com" },
    }),
  ],
};

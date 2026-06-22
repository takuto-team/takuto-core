// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { useEffect, useState, type ReactNode } from "react";
import { fn, userEvent, within } from "storybook/test";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Onboarding } from "../pages/Onboarding";
import { ToastProvider } from "../hooks/useToast";

interface ApiMock {
  /** Saved ticketing system surfaced by GET /api/config. */
  ticketingSystem?: "none" | "jira" | "github";
  /** When set, GET /api/users/me/credentials reports a connected Jira account. */
  jiraConnected?: { site: string; email: string };
}

/** A couple of GitHub-accessible repos the user can add on the Repositories step. */
const AVAILABLE_REPOS = [
  { id: "r1", name: "acme/web", default_branch: "main", private: false, cloned: false },
  { id: "r2", name: "acme/api", default_branch: "main", private: true, cloned: false },
];

/**
 * Patches `window.fetch` for every endpoint the 5-step wizard touches so all
 * steps — ticketing, AI provider, Git & GitHub, Repositories (MyRepositoriesTab),
 * and Workflows (FlowsTab) — render deterministically without a backend.
 * Installed in a `useState` initializer so the patch lands before child mount
 * effects fire (mirrors `FlowsTab.stories.tsx`).
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
        const path = url.split("?")[0];
        const method = (init?.method ?? "GET").toUpperCase();

        if (path === "/api/config" && method === "GET") {
          return json({
            general: { ticketing_system: mock.ticketingSystem ?? "none" },
            agent: { provider: "claude", providers: {}, step_timeout_secs: 1800 },
            git: { base_branch: "main", remote: "origin" },
            jira: { project_keys: [], site: "" },
            github: { app_id: 0, app_installation_id: 0 },
            web: { dashboard_username: "admin" },
            jira_available: false,
            ticketing_system: mock.ticketingSystem ?? "none",
            github_app_configured: true,
            repo_exists: true,
          });
        }
        if (path === "/api/users/me/credentials" && method === "GET") {
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
        if (path === "/api/auth/status" && method === "GET") {
          return json({
            dashboard_auth_enabled: true,
            multi_user: true,
            setup_required: false,
            github_mode: "missing",
          });
        }
        if (path === "/api/me/flows" && method === "GET") {
          return json({ flows: [], workspace: "acme/web" });
        }
        // Repositories step (MyRepositoriesTab → useRepositoryAdmin).
        if (path === "/api/repositories" && method === "GET") return json([]);
        if (path === "/api/repositories/_available" && method === "GET") return json(AVAILABLE_REPOS);
        if (path === "/api/repositories/access" && method === "GET") return json([]);

        // Writes the wizard issues on Continue / Finish / Add-repo.
        if (method !== "GET") {
          if (path === "/api/users/me/jira-credential") {
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

    useEffect(
      () => () => {
        window.fetch = realFetch;
      },
      [realFetch],
    );

    return <Story />;
  };
}

/** Click "Skip for now" `count` times, awaiting each next step to render. */
async function skip(canvasElement: HTMLElement, count: number) {
  const canvas = within(canvasElement);
  for (let i = 0; i < count; i++) {
    const btn = await canvas.findByText("Skip for now");
    await userEvent.click(btn);
  }
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
    (Story) => {
      // Fresh QueryClient per story — the Repositories + Workflows steps use
      // react-query (useRepositoryAdmin / useMyRepositories).
      const [client] = useState(
        () =>
          new QueryClient({
            defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
          }),
      );
      return (
        <QueryClientProvider client={client}>
          <ToastProvider>
            <MemoryRouter>
              <Story />
            </MemoryRouter>
          </ToastProvider>
        </QueryClientProvider>
      );
    },
    withApiMock(),
  ],
  args: {
    onLogout: fn(),
    authEnabled: true,
    isAdmin: true,
  },
} satisfies Meta<typeof Onboarding>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * The whole wizard, fully wired: start at step 1 and click Continue / Skip to
 * walk all five steps (Ticketing → AI provider → Git & GitHub → Repositories →
 * Workflows). Every step's data-fetching renders against the mock.
 */
export const FullWalkthrough: Story = {
  name: "Full wizard (interactive — click through all 5 steps)",
};

// Per-step snapshots: each auto-advances to its step via "Skip for now" so the
// Storybook sidebar showcases the entire wizard step by step.

export const Step1Ticketing: Story = {
  name: "Step 1 — Ticketing",
};

export const Step2Provider: Story = {
  name: "Step 2 — AI provider",
  play: async ({ canvasElement }) => skip(canvasElement, 1),
};

export const Step3GitHub: Story = {
  name: "Step 3 — Git & GitHub",
  play: async ({ canvasElement }) => skip(canvasElement, 2),
};

export const Step4Repositories: Story = {
  name: "Step 4 — Repositories",
  play: async ({ canvasElement }) => skip(canvasElement, 3),
};

export const Step5Workflows: Story = {
  name: "Step 5 — Workflows",
  play: async ({ canvasElement }) => skip(canvasElement, 4),
};

/** Variant: Jira already connected on the Ticketing step. */
export const WithJiraTicketing: Story = {
  name: "Variant — Jira already connected",
  decorators: [
    withApiMock({
      ticketingSystem: "jira",
      jiraConnected: { site: "https://acme.atlassian.net", email: "dev@acme.com" },
    }),
  ],
};

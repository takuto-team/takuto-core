// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useState } from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router-dom";
import { ItemPollingSettingsSection } from "../components/admin/ItemPollingSettingsSection";
import { ToastProvider } from "../hooks/useToast";
import type { ConfigResponse, PollingConfig } from "../api/types";
import type { UserFlow, UserFlowsResponse } from "../api/flows";

type FetchMock = {
  polling: PollingConfig;
  itemTypes: string[];
  flows: UserFlow[];
  /** Active ticketing system the mocked /api/config reports. Defaults to "jira". */
  ticketingSystem?: string;
  /** When set, GET /api/config responds with this delay forever — loading state. */
  getDelayMs?: number;
  /** When true, GET /api/config responds 500. */
  getFails?: boolean;
  /** When true, PUT /api/config/polling responds with persisted=false. */
  persistFails?: boolean;
};

function buildConfig(
  polling: PollingConfig,
  itemTypes: string[],
  ticketingSystem: string,
): ConfigResponse {
  return {
    general: {
      dry_mode: true,
      max_concurrent_workflows: 4,
      max_active_workflows: 0,
      max_concurrent_manual_workflows: 2,
      poll_interval_secs: 60,
      auto_polling: true,
      ticketing_system: ticketingSystem,
      pr_merge_poll_interval_secs: 30,
      generate_report: true,
      work_item_log_retention_days: 14,
    },
    jira: {
      project_keys: ["PROJ"],
      site: "example.atlassian.net",
      item_types: itemTypes,
      linked_items_in_prompt: "summary_only",
      ticket_context_max_description_bytes: 8192,
      linked_issue_description_max_bytes: 2048,
      jql_filter: 'labels = "takuto"',
      done_status: "Done",
    },
    github: { app_id: 0, app_installation_id: 0 },
    web: { dashboard_username: "admin" },
    jira_available: ticketingSystem === "jira",
    ticketing_system: ticketingSystem,
    github_app_configured: ticketingSystem === "github",
    repo_exists: true,
    polling,
  };
}

/**
 * Patches `window.fetch` for /api/config + /api/me/flows + /api/config/polling
 * while the story is mounted, so the un-prop-driven section renders without a
 * backend. Installed in a `useState` initializer (not `useEffect`) because
 * child effects fire before parent effects on mount.
 */
function withConfigMock(mock: FetchMock) {
  return function Decorator(Story: () => React.ReactNode) {
    const [realFetch] = useState(() => {
      const original = window.fetch;
      const ticketingSystem = mock.ticketingSystem ?? "jira";
      const state = { polling: mock.polling, itemTypes: mock.itemTypes };

      window.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.toString();
        const method = (init?.method ?? "GET").toUpperCase();
        if (url === "/api/config" && method === "GET") {
          if (mock.getFails) return new Response("simulated server error", { status: 500 });
          if (mock.getDelayMs) await new Promise((r) => setTimeout(r, mock.getDelayMs));
          return new Response(
            JSON.stringify(buildConfig(state.polling, state.itemTypes, ticketingSystem)),
            { status: 200, headers: { "Content-Type": "application/json" } },
          );
        }
        if (url === "/api/me/flows" && method === "GET") {
          const body: UserFlowsResponse = { flows: mock.flows, workspace: "takuto-core" };
          return new Response(JSON.stringify(body), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        if (url === "/api/config/polling" && method === "PUT") {
          const parsed = JSON.parse((init?.body as string) ?? "{}") as {
            auto_start_flow?: string;
            max_parallel_items?: number;
            max_parallel_per_user?: boolean;
            jira?: { summary_keywords?: string[] };
            github?: { labels?: string[]; title_keywords?: string[] };
            item_types?: string[];
          };
          state.polling = {
            auto_start_flow: parsed.auto_start_flow ?? state.polling.auto_start_flow,
            max_parallel_items: parsed.max_parallel_items ?? state.polling.max_parallel_items,
            max_parallel_per_user:
              parsed.max_parallel_per_user ?? state.polling.max_parallel_per_user,
            jira: { summary_keywords: parsed.jira?.summary_keywords ?? [] },
            github: {
              labels: parsed.github?.labels ?? [],
              title_keywords: parsed.github?.title_keywords ?? [],
            },
          };
          state.itemTypes = parsed.item_types ?? state.itemTypes;
          const config = buildConfig(state.polling, state.itemTypes, ticketingSystem);
          config.persisted = !mock.persistFails;
          if (mock.persistFails) config.persist_warning = "read-only config volume (EROFS)";
          return new Response(JSON.stringify(config), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        if (url === "/api/config/jira" && method === "PUT") {
          // The Jira-context PUT only mutates the [jira] block; for the story
          // we just echo a fresh config so the dual-endpoint save resolves.
          const config = buildConfig(state.polling, state.itemTypes, ticketingSystem);
          config.persisted = !mock.persistFails;
          if (mock.persistFails) config.persist_warning = "read-only config volume (EROFS)";
          return new Response(JSON.stringify(config), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
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

const FLOWS: UserFlow[] = [
  { name: "Implement ticket", depends_on: [], steps: [] },
  { name: "Review changes", depends_on: ["Implement ticket"], steps: [] },
];

const EMPTY_POLLING: PollingConfig = {
  auto_start_flow: "",
  max_parallel_items: 0,
  max_parallel_per_user: false,
  jira: { summary_keywords: [] },
  github: { labels: [], title_keywords: [] },
};

const POPULATED_POLLING: PollingConfig = {
  auto_start_flow: "implement-ticket",
  max_parallel_items: 5,
  max_parallel_per_user: true,
  jira: { summary_keywords: ["crash", "regression"] },
  github: { labels: ["bug"], title_keywords: ["panic"] },
};

const meta = {
  title: "Components/ItemPollingSettingsSection",
  component: ItemPollingSettingsSection,
  parameters: {
    layout: "fullscreen",
    backgrounds: { default: "dark", values: [{ name: "dark", value: "#030712" }] },
  },
  decorators: [
    (Story) => (
      <ToastProvider>
        <MemoryRouter>
          <div className="p-8 max-w-3xl mx-auto">
            <Story />
          </div>
        </MemoryRouter>
      </ToastProvider>
    ),
  ],
} satisfies Meta<typeof ItemPollingSettingsSection>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loading: Story = {
  decorators: [
    withConfigMock({ polling: EMPTY_POLLING, itemTypes: [], flows: FLOWS, getDelayMs: 100000 }),
  ],
};

export const Loaded: Story = {
  name: "Loaded — Jira (shows Jira filters only)",
  decorators: [
    withConfigMock({
      polling: POPULATED_POLLING,
      itemTypes: ["Bug", "Task"],
      flows: FLOWS,
      ticketingSystem: "jira",
    }),
  ],
};

export const LoadedGitHub: Story = {
  name: "Loaded — GitHub (shows GitHub filters only)",
  decorators: [
    withConfigMock({
      polling: POPULATED_POLLING,
      itemTypes: [],
      flows: FLOWS,
      ticketingSystem: "github",
    }),
  ],
};

export const NoTicketing: Story = {
  name: "Loaded — no ticketing system (filters hidden)",
  decorators: [
    withConfigMock({
      polling: EMPTY_POLLING,
      itemTypes: [],
      flows: FLOWS,
      ticketingSystem: "none",
    }),
  ],
};

export const Error: Story = {
  name: "Error — config fetch fails",
  decorators: [
    withConfigMock({ polling: EMPTY_POLLING, itemTypes: [], flows: FLOWS, getFails: true }),
  ],
};

export const Saved: Story = {
  name: "Saved — click Save to see the success toast",
  decorators: [
    withConfigMock({ polling: POPULATED_POLLING, itemTypes: ["Bug"], flows: FLOWS }),
  ],
};

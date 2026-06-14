// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useState } from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router-dom";
import { StepGuardrailsSection } from "../components/admin/StepGuardrailsSection";
import { ToastProvider } from "../hooks/useToast";

type FetchMock = {
  step_timeout_secs?: number;
  improve_timeout_secs?: number;
  max_repeated_output_lines?: number;
  /** When set, GET /api/config responds with this delay forever — loading state. */
  getDelayMs?: number;
  /** When true, GET /api/config responds 500. */
  getFails?: boolean;
  /** When true, PUT /api/config/agent responds with persisted=false. */
  persistFails?: boolean;
};

function buildConfig(mock: FetchMock): unknown {
  return {
    general: { ticketing_system: "none" },
    agent: {
      provider: "claude",
      step_timeout_secs: mock.step_timeout_secs,
      improve_timeout_secs: mock.improve_timeout_secs,
      max_repeated_output_lines: mock.max_repeated_output_lines,
      providers: {},
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

/** Patches window.fetch for /api/config (GET) + /api/config/agent (PUT). */
function withConfigMock(mock: FetchMock) {
  return function Decorator(Story: () => React.ReactNode) {
    const [realFetch] = useState(() => {
      const original = window.fetch;
      window.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
        const u = typeof input === "string" ? input : input.toString();
        const method = (init?.method ?? "GET").toUpperCase();
        if (u === "/api/config" && method === "GET") {
          if (mock.getFails) return new Response("simulated server error", { status: 500 });
          if (mock.getDelayMs) await new Promise((r) => setTimeout(r, mock.getDelayMs));
          return new Response(JSON.stringify(buildConfig(mock)), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        if (u === "/api/config/agent" && method === "PUT") {
          const config = buildConfig(mock) as Record<string, unknown>;
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

const meta = {
  title: "Components/StepGuardrailsSection",
  component: StepGuardrailsSection,
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
} satisfies Meta<typeof StepGuardrailsSection>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loading: Story = {
  decorators: [withConfigMock({ getDelayMs: 100000 })],
};

export const Defaults: Story = {
  name: "Defaults (all empty)",
  decorators: [withConfigMock({})],
};

export const Populated: Story = {
  decorators: [
    withConfigMock({
      step_timeout_secs: 900,
      improve_timeout_secs: 120,
      max_repeated_output_lines: 8,
    }),
  ],
};

export const Error: Story = {
  name: "Error — config fetch fails",
  decorators: [withConfigMock({ getFails: true })],
};

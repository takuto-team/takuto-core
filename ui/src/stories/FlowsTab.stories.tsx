// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { useEffect } from "react";
import { FlowsTab } from "../components/FlowsTab";
import type { UserFlow, UserFlowsResponse } from "../api/flows";

type FetchMock = {
  workspace: string;
  flows: UserFlow[];
  /** When set, GET responds with this delay forever — exercises the loading state. */
  getDelayMs?: number;
  /** When true, GET responds 500. */
  getFails?: boolean;
};

/**
 * Decorator that patches `window.fetch` for /api/me/flows* while the story is
 * mounted, so the un-prop-driven FlowsTab can render without a real backend.
 */
function withFlowsMock(mock: FetchMock) {
  return function Decorator(Story: () => React.ReactNode) {
    useEffect(() => {
      const realFetch = window.fetch;
      const state: { workspace: string; flows: UserFlow[] } = {
        workspace: mock.workspace,
        flows: mock.flows.map((f) => ({ ...f })),
      };

      window.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.toString();
        if (url.startsWith("/api/me/flows")) {
          const method = (init?.method ?? "GET").toUpperCase();
          if (url === "/api/me/flows" && method === "GET") {
            if (mock.getFails) {
              return new Response("simulated server error", { status: 500 });
            }
            if (mock.getDelayMs) {
              await new Promise((r) => setTimeout(r, mock.getDelayMs));
            }
            const body: UserFlowsResponse = {
              flows: state.flows,
              workspace: state.workspace,
            };
            return new Response(JSON.stringify(body), {
              status: 200,
              headers: { "Content-Type": "application/json" },
            });
          }
          if (url === "/api/me/flows" && method === "PUT") {
            const parsed = JSON.parse((init?.body as string) ?? "{}") as { flows: UserFlow[] };
            state.flows = parsed.flows;
            return new Response(
              JSON.stringify({ flows: state.flows, workspace: state.workspace }),
              { status: 200, headers: { "Content-Type": "application/json" } },
            );
          }
          if (url === "/api/me/flows/reseed" && method === "POST") {
            state.flows = mock.flows.map((f) => ({ ...f }));
            return new Response(
              JSON.stringify({ flows: state.flows, workspace: state.workspace }),
              { status: 200, headers: { "Content-Type": "application/json" } },
            );
          }
        }
        return realFetch(input as RequestInfo, init);
      }) as typeof window.fetch;

      return () => {
        window.fetch = realFetch;
      };
    }, []);
    return <Story />;
  };
}

const implement: UserFlow = {
  name: "implement_ticket",
  depends_on: [],
  steps: [
    {
      name: "Implement",
      prompt: "Implement the changes described in the ticket. Do not commit yet.",
      skills: [],
    },
    {
      name: "Review",
      prompt: "Review your diff against `{base_branch}` and address findings.",
      skills: [],
    },
  ],
};

const review: UserFlow = {
  name: "review_changes",
  depends_on: ["implement_ticket"],
  steps: [
    {
      name: "Code review",
      prompt: "Walk the diff against `{base_branch}` and call out issues with severity labels.",
      skills: [{ name: "review-rubric", args: ["--strict"] }],
    },
  ],
};

const createPr: UserFlow = {
  name: "create_pr",
  depends_on: ["review_changes"],
  steps: [
    {
      name: "Open PR",
      prompt: "Open a PR targeting `{base_branch}` with a conventional-commit title.",
      skills: [{ name: "create-pr", args: ["--no-draft"] }],
    },
  ],
};

const seededDefaults: UserFlow[] = [implement, review, createPr];

const filledToCap: UserFlow[] = Array.from({ length: 20 }, (_, i) => ({
  name: `flow_${i + 1}`,
  depends_on: i === 0 ? [] : [`flow_${i}`],
  steps: [{ name: "Step", prompt: `Run step ${i + 1}.`, skills: [] }],
}));

const meta = {
  title: "Pages/FlowsTab",
  component: FlowsTab,
  parameters: {
    layout: "padded",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
} satisfies Meta<typeof FlowsTab>;

export default meta;
type Story = StoryObj<typeof meta>;

export const SeededDefaults: Story = {
  name: "Seeded defaults",
  decorators: [withFlowsMock({ workspace: "maestro-core", flows: seededDefaults })],
};

export const EmptyByChoice: Story = {
  name: "Empty (user cleared all)",
  decorators: [withFlowsMock({ workspace: "maestro-core", flows: [] })],
};

export const AtCap: Story = {
  name: "At the 20-flow cap",
  decorators: [withFlowsMock({ workspace: "maestro-core", flows: filledToCap })],
};

export const Loading: Story = {
  name: "Loading (slow GET)",
  decorators: [
    withFlowsMock({ workspace: "maestro-core", flows: seededDefaults, getDelayMs: 1_000_000 }),
  ],
};

export const LoadError: Story = {
  name: "GET failed",
  decorators: [withFlowsMock({ workspace: "maestro-core", flows: [], getFails: true })],
};

export const SingleFlow: Story = {
  name: "Just one flow",
  decorators: [withFlowsMock({ workspace: "rust-experiments", flows: [implement] })],
};

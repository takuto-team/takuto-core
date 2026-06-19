// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { useEffect, useState } from "react";
import { GenerateReportSwitch } from "../components/GenerateReportSwitch";
import type { WorktreeCommandsRow } from "../api/worktreeCommands";

const WORKSPACE = "takuto-core";

type FetchMock = {
  /** Initial row the GET returns; null → 404 (no row yet, toggle starts off). */
  row: WorktreeCommandsRow | null;
  /** When set, GET hangs this long — exercises the disabled-while-loading state. */
  getDelayMs?: number;
  /** When true, PUT responds 500 so flipping surfaces the inline error + reverts. */
  putFails?: boolean;
};

/**
 * Patches `window.fetch` for `/api/worktree-commands/{workspace}` while the
 * story is mounted, so the auto-saving switch loads + persists against an
 * in-memory row without a real backend. Installed in a `useState` initializer
 * (not `useEffect`) so the child's mount GET hits the mock, not the real fetch.
 */
function withWorktreeCommandsMock(mock: FetchMock) {
  return function Decorator(Story: () => React.ReactNode) {
    const [realFetch] = useState(() => {
      const original = window.fetch;
      const state: { row: WorktreeCommandsRow | null } = { row: mock.row && { ...mock.row } };
      const path = `/api/worktree-commands/${WORKSPACE}`;

      window.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.toString();
        if (url === path) {
          const method = (init?.method ?? "GET").toUpperCase();
          if (method === "GET") {
            if (mock.getDelayMs) await new Promise((r) => setTimeout(r, mock.getDelayMs));
            if (!state.row) return new Response(null, { status: 404 });
            return new Response(JSON.stringify(state.row), {
              status: 200,
              headers: { "Content-Type": "application/json" },
            });
          }
          if (method === "PUT") {
            if (mock.putFails) return new Response("simulated server error", { status: 500 });
            const body = JSON.parse((init?.body as string) ?? "{}") as {
              init_commands: string[];
              run_commands: WorktreeCommandsRow["run_commands"];
              generate_report: boolean;
            };
            state.row = {
              workspace_name: WORKSPACE,
              init_commands: body.init_commands,
              run_commands: body.run_commands,
              generate_report: body.generate_report,
              updated_at: 0,
            };
            return new Response(JSON.stringify(state.row), {
              status: 200,
              headers: { "Content-Type": "application/json" },
            });
          }
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

const rowOff: WorktreeCommandsRow = {
  workspace_name: WORKSPACE,
  init_commands: ["npm ci"],
  run_commands: [{ name: "dev", command: "npm run dev" }],
  generate_report: false,
  updated_at: 0,
};

const meta = {
  title: "Components/GenerateReportSwitch",
  component: GenerateReportSwitch,
  parameters: {
    layout: "fullscreen",
    backgrounds: { default: "dark", values: [{ name: "dark", value: "#030712" }] },
  },
  decorators: [
    (Story) => (
      <div className="p-8 max-w-3xl mx-auto">
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof GenerateReportSwitch>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Off: Story = {
  name: "Off (existing row, flip to save)",
  args: { workspace: WORKSPACE },
  decorators: [withWorktreeCommandsMock({ row: rowOff })],
};

export const On: Story = {
  name: "On (existing row)",
  args: { workspace: WORKSPACE },
  decorators: [withWorktreeCommandsMock({ row: { ...rowOff, generate_report: true } })],
};

export const NoRowYet: Story = {
  name: "No row yet (404 → off; flip creates the row)",
  args: { workspace: WORKSPACE },
  decorators: [withWorktreeCommandsMock({ row: null })],
};

export const Loading: Story = {
  name: "Loading (slow GET, disabled)",
  args: { workspace: WORKSPACE },
  decorators: [withWorktreeCommandsMock({ row: rowOff, getDelayMs: 1_000_000 })],
};

export const SaveError: Story = {
  name: "Save fails (flip reverts + shows error)",
  args: { workspace: WORKSPACE },
  decorators: [withWorktreeCommandsMock({ row: rowOff, putFails: true })],
};

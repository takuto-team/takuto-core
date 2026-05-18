// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { MemoryRouter } from "react-router-dom";
import { Onboarding } from "../pages/Onboarding";
import { ToastProvider } from "../hooks/useToast";

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
 * The wizard fetches `/api/config` on mount. Storybook has no real server, so
 * `fetch` returns a 404 and the form lands on its default-state branches —
 * which is exactly what step-1-fresh-install looks like. Each story is the
 * wizard rendered as-is; click through the Continue/Skip buttons to walk the
 * four steps within a single story.
 */
export const FreshInstall: Story = {
  name: "Step 1 — fresh install (no ticketing system)",
};

export const WithJiraTicketing: Story = {
  name: "Step 1 — Jira already configured (server mocked)",
  parameters: {
    msw: {
      // Pseudo-doc: real MSW handler would live here. Storybook's server
      // mocking isn't wired into this repo yet — the story still renders the
      // wizard and exercises its layout.
    },
  },
};

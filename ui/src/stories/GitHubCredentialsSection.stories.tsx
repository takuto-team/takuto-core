// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router-dom";
import { GitHubCredentialsSection } from "../components/credentials/GitHubCredentialsSection";
import { ToastProvider } from "../hooks/useToast";
import { resetMocks, setMocksEnabled, setNextFailure } from "../api/mocks";
import type { UserCredentialsStatus } from "../api/types";

/**
 * Stories for the per-user GitHub credentials tab. Each seeds a fixture via
 * `resetMocks()` so Storybook doesn't depend on a running backend. The
 * effective App/PAT mode lives on `/api/auth/status::github_mode`; Storybook
 * can't set it, so the App-only pill state is exercised by the vitest test.
 */

function withMocks(fixture: UserCredentialsStatus, failOnce?: () => void) {
  return function MocksDecorator(Story: React.ComponentType) {
    setMocksEnabled(true);
    resetMocks(fixture);
    failOnce?.();
    return (
      <ToastProvider>
        <MemoryRouter>
          <Story />
        </MemoryRouter>
      </ToastProvider>
    );
  };
}

const meta = {
  title: "Components/GitHubCredentialsSection",
  component: GitHubCredentialsSection,
  parameters: {
    layout: "fullscreen",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  args: {},
} satisfies Meta<typeof GitHubCredentialsSection>;

export default meta;
type Story = StoryObj<typeof meta>;

export const AppOnly: Story = {
  name: "Mode A — App only (no PAT)",
  decorators: [
    withMocks({
      provider: null,
      // A missing PAT is `github: null` per routes/credentials.rs
      // (Option<...>). The effective mode lives on /api/auth/status.
      github: null,
    }),
  ],
};

export const AppPlusPat: Story = {
  name: "Mode B — App + PAT (attribute on)",
  decorators: [
    withMocks({
      provider: null,
      github: {
        login: "alice-gh",
        scopes: ["repo", "read:org"],
        attribute_commits: true,
        last_validated_at: new Date().toISOString(),
      },
    }),
  ],
};

export const PatOnly: Story = {
  name: "Mode C — PAT only (attribute off)",
  decorators: [
    withMocks({
      provider: null,
      github: {
        login: "alice-gh",
        scopes: ["repo"],
        attribute_commits: false,
        last_validated_at: new Date().toISOString(),
      },
    }),
  ],
};

export const SsoRequiredToast: Story = {
  name: "GitHub SSO required (toast on next save)",
  decorators: [
    withMocks(
      {
        provider: null,
        github: null,
      },
      () =>
        setNextFailure({
          kind: "sso_required",
          orgUrl: "https://github.com/orgs/acme/sso",
        }),
    ),
  ],
};

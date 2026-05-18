// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { MemoryRouter } from "react-router-dom";
import { UserCredentials } from "../pages/UserCredentials";
import { ToastProvider } from "../hooks/useToast";
import {
  resetMocks,
  setMocksEnabled,
  setNextFailure,
} from "../api/mocks";
import type { UserCredentialsStatus } from "../api/types";

/**
 * Phase 2 stories: render the per-user credential page with the in-memory
 * mock layer. Each story seeds a fixture via `resetMocks()` so Storybook
 * doesn't depend on a running backend. The `setMocksEnabled(true)`
 * decorator below also flips the runtime override regardless of whether
 * `VITE_USE_MOCKS` is set at storybook startup time.
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
  title: "Pages/UserCredentials",
  component: UserCredentials,
  parameters: {
    layout: "fullscreen",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  args: {
    onLogout: fn(),
    authEnabled: true,
  },
} satisfies Meta<typeof UserCredentials>;

export default meta;
type Story = StoryObj<typeof meta>;

/* ── AI provider states ── */

export const ClaudeMissing: Story = {
  name: "Claude — ⚠ missing (paste field shown)",
  decorators: [
    withMocks({
      provider: null,
      github: {
        has_pat: false,
        login: null,
        scopes: [],
        attribute_commits: true,
        mode: "app",
      },
    }),
  ],
};

export const ClaudeConnected: Story = {
  name: "Claude — ✅ connected",
  decorators: [
    withMocks({
      provider: {
        provider: "claude",
        kind: "api_key",
        active: true,
        last_validated_at: new Date(Date.now() - 12 * 60 * 1000).toISOString(),
        last_used_at: null,
      },
      github: {
        has_pat: false,
        login: null,
        scopes: [],
        attribute_commits: true,
        mode: "app",
      },
    }),
  ],
};

export const CursorMissingNoTtydCopy: Story = {
  name: "Cursor — A1 regression guard (no ttyd copy)",
  decorators: [
    withMocks({
      provider: null,
      github: {
        has_pat: false,
        login: null,
        scopes: [],
        attribute_commits: true,
        mode: "app",
      },
    }),
  ],
  // The mock layer surfaces provider_selected via /api/auth/status which the
  // page also fetches. Storybook can't intercept that — but the page
  // defaults to "claude" when auth status is missing. To exercise the
  // Cursor card, override `localStorage` or accept that Storybook shows the
  // claude card. The regression guard is enforced by the **vitest** test,
  // which is unconditional. This story exists for the form-field shape
  // review.
};

export const CodexPhase4Placeholder: Story = {
  name: "Codex — Phase 4 placeholder card",
  decorators: [
    withMocks({
      provider: null,
      github: {
        has_pat: false,
        login: null,
        scopes: [],
        attribute_commits: true,
        mode: "app",
      },
    }),
  ],
};

/* ── GitHub modes ── */

export const GitHubAppOnly: Story = {
  name: "GitHub — Mode A (App only)",
  decorators: [
    withMocks({
      provider: null,
      github: {
        has_pat: false,
        login: null,
        scopes: [],
        attribute_commits: true,
        mode: "app",
      },
    }),
  ],
};

export const GitHubAppPlusPat: Story = {
  name: "GitHub — Mode B (App + PAT)",
  decorators: [
    withMocks({
      provider: null,
      github: {
        has_pat: true,
        login: "alice-gh",
        scopes: ["repo", "read:org"],
        attribute_commits: true,
        mode: "app_plus_pat",
      },
    }),
  ],
};

export const GitHubPatOnly: Story = {
  name: "GitHub — Mode C (PAT only)",
  decorators: [
    withMocks({
      provider: null,
      github: {
        has_pat: true,
        login: "alice-gh",
        scopes: ["repo"],
        attribute_commits: false,
        mode: "pat_only",
      },
    }),
  ],
};

/* ── Forced-failure stories ── */

export const SsoRequiredToast: Story = {
  name: "GitHub SSO required (toast on next save)",
  decorators: [
    withMocks(
      {
        provider: null,
        github: {
          has_pat: false,
          login: null,
          scopes: [],
          attribute_commits: true,
          mode: "app",
        },
      },
      () =>
        setNextFailure({
          kind: "sso_required",
          orgUrl: "https://github.com/orgs/acme/sso",
        }),
    ),
  ],
};

export const ProviderMismatch: Story = {
  name: "Provider mismatch banner (admin set Cursor, user has Claude)",
  decorators: [
    withMocks({
      provider: {
        provider: "claude",
        kind: "api_key",
        active: true,
        last_validated_at: new Date().toISOString(),
        last_used_at: null,
      },
      github: {
        has_pat: false,
        login: null,
        scopes: [],
        attribute_commits: true,
        mode: "app",
      },
    }),
  ],
};

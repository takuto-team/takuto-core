// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router-dom";
import { MyCredentialsSection } from "../components/MyCredentialsSection";
import { ToastProvider } from "../hooks/useToast";
import { resetMocks, setMocksEnabled } from "../api/mocks";
import type { UserCredentialsStatus } from "../api/types";

/**
 * Stories that render the per-user credential page with the in-memory mock
 * layer. Each story seeds a fixture via `resetMocks()` so Storybook
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
  title: "Components/MyCredentialsSection",
  component: MyCredentialsSection,
  parameters: {
    layout: "fullscreen",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  // MyCredentialsSection takes no props — the old page-level
  // onLogout/authEnabled belonged to the deleted page chrome. Stories that
  // need a logout button live on the Config page's story instead.
  args: {},
} satisfies Meta<typeof MyCredentialsSection>;

export default meta;
type Story = StoryObj<typeof meta>;

/* ── AI provider states ── */

export const ClaudeMissing: Story = {
  name: "Claude — ⚠ missing (paste field shown)",
  decorators: [
    withMocks({
      provider: null,
      // Wire-format note: a missing PAT is `github: null` per
      // routes/credentials.rs::UserCredentialsStatus (Option<...>). The
      // page reads the effective mode from /api/auth/status::github_mode.
      github: null,
    }),
  ],
};

export const ClaudeConnected: Story = {
  name: "Claude — ✅ connected",
  decorators: [
    withMocks({
      // Bundle layout per #39 — { provider, api_key?, cli_state? }.
      provider: {
        provider: "claude",
        api_key: {
          provider: "claude",
          kind: "api_key",
          active: true,
          last_validated_at: new Date(Date.now() - 12 * 60 * 1000).toISOString(),
          last_used_at: null,
        },
      },
      // Wire-format note: a missing PAT is `github: null` per
      // routes/credentials.rs::UserCredentialsStatus (Option<...>). The
      // page reads the effective mode from /api/auth/status::github_mode.
      github: null,
    }),
  ],
};

export const CursorMissingNoTtydCopy: Story = {
  name: "Cursor — A1 regression guard (no ttyd copy)",
  decorators: [
    withMocks({
      provider: null,
      // Wire-format note: a missing PAT is `github: null` per
      // routes/credentials.rs::UserCredentialsStatus (Option<...>). The
      // page reads the effective mode from /api/auth/status::github_mode.
      github: null,
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
      // Wire-format note: a missing PAT is `github: null` per
      // routes/credentials.rs::UserCredentialsStatus (Option<...>). The
      // page reads the effective mode from /api/auth/status::github_mode.
      github: null,
    }),
  ],
};

/* ── Provider mismatch ── */

export const ProviderMismatch: Story = {
  name: "Provider mismatch banner (admin set Cursor, user has Claude)",
  decorators: [
    withMocks({
      provider: {
        provider: "claude",
        api_key: {
          provider: "claude",
          kind: "api_key",
          active: true,
          last_validated_at: new Date().toISOString(),
          last_used_at: null,
        },
      },
      // Wire-format note: a missing PAT is `github: null` per
      // routes/credentials.rs::UserCredentialsStatus (Option<...>). The
      // page reads the effective mode from /api/auth/status::github_mode.
      github: null,
    }),
  ],
};

// ── Claude session-state mode (kind=cli_state) ──

export const ClaudeSessionOnly: Story = {
  name: "Claude — Session only (kind=cli_state, no API key)",
  decorators: [
    withMocks({
      provider: {
        provider: "claude",
        cli_state: {
          provider: "claude",
          kind: "cli_state",
          active: true,
          last_validated_at: new Date().toISOString(),
          last_used_at: null,
        },
      },
      github: null,
    }),
  ],
};

export const ClaudeBothKindsConnected: Story = {
  name: "Claude — both API key + Session connected",
  decorators: [
    withMocks({
      provider: {
        provider: "claude",
        api_key: {
          provider: "claude",
          kind: "api_key",
          active: true,
          last_validated_at: new Date().toISOString(),
          last_used_at: null,
        },
        cli_state: {
          provider: "claude",
          kind: "cli_state",
          active: true,
          last_validated_at: new Date().toISOString(),
          last_used_at: null,
        },
      },
      github: null,
    }),
  ],
};

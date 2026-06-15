// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router-dom";
import { OnboardingBanner } from "../components/OnboardingBanner";
import type { SystemStatus } from "../api/types";

const meta = {
  title: "Components/OnboardingBanner",
  component: OnboardingBanner,
  parameters: {
    layout: "fullscreen",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  decorators: [
    (Story) => (
      <MemoryRouter>
        <Story />
      </MemoryRouter>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof OnboardingBanner>;

export default meta;
type Story = StoryObj<typeof meta>;

/** A baseline healthy status — used as a starting point for each story. */
function healthy(): SystemStatus {
  return {
    config_toml_ok: true,
    github: {
      mode: "app",
      app_configured: true,
      app_id: 12345,
      app_name: "takuto-bot",
    },
    provider: {
      selected: "claude",
      deployment_default_credential_present: true,
      headless_capable: true,
      custom_base_url: null,
    },
    ticketing: { system: "jira", acli_ok: true },
    per_user_required: true,
    warnings: [],
  };
}

export const Healthy: Story = {
  name: "Healthy (no banner rendered)",
  args: {
    status: healthy(),
  },
};

/* ── User-facing deep-links (every authenticated user sees them) ── */

export const ClaudeNotAuthenticated: Story = {
  name: "Critical · claude_not_authenticated → /me/credentials",
  args: {
    status: {
      ...healthy(),
      warnings: [
        {
          code: "claude_not_authenticated",
          severity: "critical",
          message:
            "Claude Code is not authenticated and no CLAUDE_CODE_OAUTH_TOKEN env var is set.",
        },
      ],
    },
  },
};

export const CursorNotAuthenticated: Story = {
  name: "Critical · cursor_not_authenticated → /me/credentials",
  args: {
    status: {
      ...healthy(),
      warnings: [
        {
          code: "cursor_not_authenticated",
          severity: "critical",
          message:
            "Cursor is not authenticated. Paste a CURSOR_API_KEY from cursor.com/dashboard.",
        },
      ],
    },
  },
};

export const GhAuthMissing: Story = {
  name: "Critical · gh_auth_missing → /me/credentials",
  args: {
    status: {
      ...healthy(),
      warnings: [
        {
          code: "gh_auth_missing",
          severity: "critical",
          message:
            "GitHub authentication is missing. Takuto can't read repos, push commits, or open PRs.",
        },
      ],
    },
  },
};

/* ── Admin-only deep-link (provider_not_implemented) ── */

export const ProviderNotImplementedAdmin: Story = {
  name: "Critical · provider_not_implemented (admin view: 'Change provider')",
  args: {
    isAdmin: true,
    status: {
      ...healthy(),
      warnings: [
        {
          code: "provider_not_implemented",
          severity: "critical",
          message:
            "Provider 'codex' adapter is not yet available — work items can't start until then.",
        },
      ],
    },
  },
};

export const ProviderNotImplementedNonAdmin: Story = {
  name: "Critical · provider_not_implemented (non-admin view: hint, no link)",
  args: {
    isAdmin: false,
    status: {
      ...healthy(),
      warnings: [
        {
          code: "provider_not_implemented",
          severity: "critical",
          message:
            "Provider 'codex' adapter is not yet available — work items can't start until then.",
        },
      ],
    },
  },
};

/* ── Admin-only docs links ── */

export const MasterKeyUnavailableAdmin: Story = {
  name: "Critical · master_key_unavailable (admin view: 'Read docs')",
  args: {
    isAdmin: true,
    status: {
      ...healthy(),
      warnings: [
        {
          code: "master_key_unavailable",
          severity: "critical",
          message:
            "Master encryption key not available — credentials cannot be sealed.",
        },
      ],
    },
  },
};

export const SecretKeyWorldReadableNonAdmin: Story = {
  name: "Critical · secret_key_world_readable (non-admin: hint, no link)",
  args: {
    isAdmin: false,
    status: {
      ...healthy(),
      warnings: [
        {
          code: "secret_key_world_readable",
          severity: "critical",
          message:
            "${data_dir}/secret.key has insecure permissions — restrict to mode 0600.",
        },
      ],
    },
  },
};

/* ── Multiple criticals, mixed audiences ── */

export const MultipleCriticalMixed: Story = {
  name: "Multiple critical warnings (mixed user + admin CTAs)",
  args: {
    isAdmin: true,
    status: {
      ...healthy(),
      warnings: [
        {
          code: "claude_not_authenticated",
          severity: "critical",
          message: "Claude credential missing for current user.",
        },
        {
          code: "gh_auth_missing",
          severity: "critical",
          message: "GitHub PAT missing.",
        },
        {
          code: "provider_not_implemented",
          severity: "critical",
          message: "Provider 'codex' adapter ships in Phase 4.",
        },
      ],
    },
  },
};

/* ── Unknown code ── */

export const UnknownCodeNoCta: Story = {
  name: "Critical with unknown code (no CTA rendered)",
  args: {
    status: {
      ...healthy(),
      warnings: [
        {
          code: "some_future_code_we_dont_know_about",
          severity: "critical",
          message: "Mystery warning that the UI doesn't recognise.",
        },
      ],
    },
  },
};

/* ── Legacy fallback (no codes, no CTAs) ── */

export const LegacyFallback: Story = {
  name: "Legacy fallback (endpoint 404 — no deep-links)",
  args: {
    status: null,
    legacyPreflightError:
      "GITHUB_APP_PRIVATE_KEY is not set\nCLAUDE_CODE_OAUTH_TOKEN is not set",
  },
};

export const Loading: Story = {
  name: "Loading (status undefined — no banner rendered)",
  args: {
    status: undefined,
  },
};

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
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
      app_name: "maestro-bot",
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

export const MissingGitHub: Story = {
  name: "Missing GitHub credential",
  args: {
    status: {
      ...healthy(),
      github: { mode: "missing", app_configured: false, app_id: null, app_name: null },
      warnings: [
        {
          code: "github_missing",
          severity: "critical",
          message:
            "GitHub authentication is not configured. Maestro can't read repos, push commits, or open PRs.",
        },
      ],
    },
  },
};

export const MissingProvider: Story = {
  name: "Missing AI provider credential",
  args: {
    status: {
      ...healthy(),
      provider: {
        selected: "none",
        deployment_default_credential_present: false,
        headless_capable: false,
        custom_base_url: null,
      },
      warnings: [
        {
          code: "provider_missing",
          severity: "critical",
          message:
            "No AI provider is selected. Pick one in AI Settings before your team can run workflows.",
        },
      ],
    },
  },
};

export const MissingAcliInfo: Story = {
  name: "Missing acli (info — no banner rendered)",
  args: {
    status: {
      ...healthy(),
      ticketing: { system: "jira", acli_ok: false },
      warnings: [
        {
          code: "acli_missing",
          severity: "info",
          message:
            "`acli` isn't authenticated. Jira polling is paused until you run `acli auth`.",
        },
      ],
    },
  },
};

export const MultipleCritical: Story = {
  name: "Multiple critical warnings",
  args: {
    status: {
      ...healthy(),
      github: { mode: "missing", app_configured: false, app_id: null, app_name: null },
      provider: {
        selected: "none",
        deployment_default_credential_present: false,
        headless_capable: false,
        custom_base_url: null,
      },
      warnings: [
        {
          code: "github_missing",
          severity: "critical",
          message:
            "GitHub authentication is not configured. Maestro can't read repos, push commits, or open PRs.",
        },
        {
          code: "provider_missing",
          severity: "critical",
          message:
            "No AI provider is selected. Pick one in AI Settings before your team can run workflows.",
        },
        {
          code: "acli_missing",
          severity: "info",
          message:
            "`acli` isn't authenticated. Jira polling is paused until you run `acli auth`.",
        },
      ],
    },
  },
};

export const LegacyFallback: Story = {
  name: "Legacy fallback (endpoint 404)",
  args: {
    status: null,
    legacyPreflightError:
      "GITHUB_APP_PRIVATE_KEY is not set\nCLAUDE_CODE_OAUTH_TOKEN is not set",
  },
};

export const LegacyFallbackEmpty: Story = {
  name: "Legacy fallback with no error (no banner rendered)",
  args: {
    status: null,
    legacyPreflightError: null,
  },
};

export const Loading: Story = {
  name: "Loading (status undefined — no banner rendered)",
  args: {
    status: undefined,
  },
};

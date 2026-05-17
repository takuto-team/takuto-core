// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { MemoryRouter } from "react-router-dom";
import { Header } from "../components/Header";

const meta = {
  title: "Components/Header",
  component: Header,
  decorators: [(Story) => <MemoryRouter><Story /></MemoryRouter>],
  parameters: {
    layout: "fullscreen",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
  args: {
    connected: true,
    authEnabled: false,
    githubAppConfigured: false,
    onLogout: fn(),
  },
} satisfies Meta<typeof Header>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Minimal: Story = {
  name: "Minimal (no bot, no auth)",
};

export const WithNamedApp: Story = {
  name: "Named GitHub App (sous-coder)",
  args: {
    githubAppConfigured: true,
    githubAppInstallationId: 12345,
    githubAppName: "sous-coder",
  },
};

export const WithAppNoName: Story = {
  name: "GitHub App without name configured",
  args: {
    githubAppConfigured: true,
    githubAppInstallationId: 12345,
  },
};

export const Disconnected: Story = {
  name: "WebSocket disconnected",
  args: {
    connected: false,
    githubAppConfigured: true,
    githubAppName: "sous-coder",
  },
};

export const FullFeatured: Story = {
  name: "All features enabled",
  args: {
    connected: true,
    authEnabled: true,
    githubAppConfigured: true,
    githubAppInstallationId: 12345,
    githubAppName: "sous-coder",
  },
};

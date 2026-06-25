// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router-dom";
import { PollingLabel } from "../components/PollingLabel";

const meta = {
  title: "Components/PollingLabel",
  component: PollingLabel,
  parameters: {
    layout: "fullscreen",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
  decorators: [
    (Story) => (
      <MemoryRouter>
        <Story />
      </MemoryRouter>
    ),
  ],
  argTypes: {
    ticketingSystem: {
      control: "select",
      options: ["none", "jira", "github"],
    },
    autoPolling: { control: "boolean" },
  },
} satisfies Meta<typeof PollingLabel>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Active: Story = {
  args: {
    autoPolling: true,
    ticketingSystem: "jira",
  },
};

export const Off: Story = {
  args: {
    autoPolling: false,
    ticketingSystem: "jira",
  },
};

export const HiddenWhenNone: Story = {
  name: "Hidden (ticketingSystem = none)",
  args: {
    autoPolling: true,
    ticketingSystem: "none",
  },
};

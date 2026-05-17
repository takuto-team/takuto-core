// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
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
  argTypes: {
    ticketingSystem: {
      control: "select",
      options: ["none", "jira", "github"],
    },
    paused: { control: "boolean" },
    toggling: { control: "boolean" },
  },
} satisfies Meta<typeof PollingLabel>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Active: Story = {
  args: {
    paused: false,
    toggling: false,
    ticketingSystem: "jira",
    onToggle: fn(),
  },
};

export const Paused: Story = {
  args: {
    paused: true,
    toggling: false,
    ticketingSystem: "jira",
    onToggle: fn(),
  },
};

export const Toggling: Story = {
  args: {
    paused: false,
    toggling: true,
    ticketingSystem: "github",
    onToggle: fn(),
  },
};

export const HiddenWhenNone: Story = {
  name: "Hidden (ticketingSystem = none)",
  args: {
    paused: false,
    toggling: false,
    ticketingSystem: "none",
    onToggle: fn(),
  },
};

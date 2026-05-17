// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { SummaryStats } from "../components/SummaryStats";

const zeroCounts = { running: 0, completed: 0, errors: 0, paused: 0 };

const meta = {
  title: "Components/SummaryStats",
  component: SummaryStats,
  parameters: {
    layout: "padded",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
  args: {
    counts: zeroCounts,
  },
} satisfies Meta<typeof SummaryStats>;

export default meta;
type Story = StoryObj<typeof meta>;

export const ZeroCounts: Story = {
  name: "All zero counts",
};

export const ActiveWorkflows: Story = {
  name: "Active workflows",
  args: {
    counts: { running: 2, completed: 3, errors: 1, paused: 1 },
  },
};

export const OnlyRunning: Story = {
  name: "Only running",
  args: {
    counts: { running: 5, completed: 0, errors: 0, paused: 0 },
  },
};

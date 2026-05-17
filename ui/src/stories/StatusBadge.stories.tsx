// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { StatusBadge, getStatusInfo } from "../components/StatusBadge";

const meta = {
  title: "Atoms/StatusBadge",
  component: StatusBadge,
  parameters: {
    layout: "centered",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
} satisfies Meta<typeof StatusBadge>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Completed: Story = {
  args: { status: getStatusInfo("done") },
};

export const Running: Story = {
  args: { status: getStatusInfo("running") },
};

export const Paused: Story = {
  args: { status: getStatusInfo("paused") },
};

export const Error: Story = {
  args: { status: getStatusInfo("error: lint failed") },
};

export const Stopped: Story = {
  args: { status: getStatusInfo("stopped") },
};

export const Pending: Story = {
  args: { status: getStatusInfo("pending", true) },
};

export const AllStatuses: Story = {
  args: { status: getStatusInfo("done") },
  render: () => (
    <div style={{ display: "flex", gap: "16px", flexWrap: "wrap", alignItems: "center" }}>
      <StatusBadge status={getStatusInfo("done")} />
      <StatusBadge status={getStatusInfo("running")} />
      <StatusBadge status={getStatusInfo("paused")} />
      <StatusBadge status={getStatusInfo("error: x")} />
      <StatusBadge status={getStatusInfo("stopped")} />
      <StatusBadge status={getStatusInfo("pending", true)} />
    </div>
  ),
};

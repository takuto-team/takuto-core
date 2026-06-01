// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { StopIconButton } from "../components/StopIconButton";

const meta = {
  title: "Atoms/StopIconButton",
  component: StopIconButton,
  parameters: {
    layout: "centered",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
} satisfies Meta<typeof StopIconButton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: { onClick: fn() },
};

export const WithTitle: Story = {
  args: { onClick: fn(), title: "Stop work item" },
};

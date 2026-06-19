// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { ProgressBar } from "../components/ProgressBar";

/**
 * Segmented workflow progress bar. Segment colours:
 * - **light blue** — the step currently in progress (`activeIndex`);
 * - **blue** — a completed step while the flow is still running (status `blue`);
 * - **green** — a completed step in a finished flow (status `green`);
 * - **grey** — a pending step.
 */
const meta = {
  title: "Atoms/ProgressBar",
  component: ProgressBar,
  parameters: {
    layout: "centered",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  decorators: [
    (Story) => (
      <div style={{ width: 480 }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof ProgressBar>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Just started: the first step is in progress (light blue), the rest pending. */
export const JustStarted: Story = {
  args: { pct: 0, total: 6, filled: 0, color: "blue", activeIndex: 0 },
};

/** Running flow: completed steps blue, the current step light blue, rest grey. */
export const InProgress: Story = {
  args: { pct: 33, total: 6, filled: 2, color: "blue", activeIndex: 2 },
};

/** Last step running: five completed (blue), one in progress (light blue). */
export const AlmostDone: Story = {
  args: { pct: 83, total: 6, filled: 5, color: "blue", activeIndex: 5 },
};

/** Completed flow: every step green, no in-progress segment. */
export const Completed: Story = {
  args: { pct: 100, total: 6, filled: 6, color: "green", activeIndex: null },
};

/** Errored flow: completed segments take the error (red) status colour. */
export const Errored: Story = {
  args: { pct: 50, total: 6, filled: 3, color: "red", activeIndex: null },
};

/** Continuous (non-segmented) bar — rendered when the step total is unknown. */
export const Continuous: Story = {
  args: { pct: 40, total: 0, filled: 0, color: "blue" },
};

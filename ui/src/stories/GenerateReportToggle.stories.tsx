// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState } from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import { GenerateReportToggle } from "../components/GenerateReportToggle";

/**
 * Local-state harness so the presentational toggle flips live in the story
 * (it is controlled — `value` + `onChange`). Interactions are local-only.
 */
function Harness({ initialValue = false, disabled = false }: { initialValue?: boolean; disabled?: boolean }) {
  const [value, setValue] = useState(initialValue);
  return <GenerateReportToggle value={value} onChange={setValue} disabled={disabled} />;
}

// `component` is intentionally omitted: the stories drive a stateful Harness
// via `render`, not args, so declaring the component (whose `value`/`onChange`
// are required) would force every story to also supply `args`. Mirrors
// `GeneralLimitsFields.stories.tsx`.
const meta = {
  title: "Components/GenerateReportToggle",
  parameters: {
    layout: "fullscreen",
    backgrounds: { default: "dark", values: [{ name: "dark", value: "#030712" }] },
  },
  decorators: [
    (Story) => (
      <div className="p-8 max-w-3xl mx-auto">
        <Story />
      </div>
    ),
  ],
} satisfies Meta;

export default meta;
type Story = StoryObj<typeof meta>;

export const Off: Story = {
  render: () => <Harness initialValue={false} />,
};

export const On: Story = {
  render: () => <Harness initialValue />,
};

export const Disabled: Story = {
  name: "Disabled (saving / loading)",
  render: () => <Harness initialValue disabled />,
};

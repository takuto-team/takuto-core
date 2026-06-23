// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState } from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import { GeneralLimitsFields } from "../components/admin/GeneralLimitsFields";

/**
 * Local-state harness so each story behaves like a real form (typing, toggling
 * the report switch). Stories are static starting points — interactions are
 * local-only.
 */
function Harness({
  initialPollInterval = "60",
  initialMaxParallelPerUser = false,
  initialMaxConcurrentManual = "0",
  initialPrMergePollInterval = "",
  initialGenerateReport = false,
  initialWorkItemLogRetention = "0",
}: {
  initialPollInterval?: string;
  initialMaxParallelPerUser?: boolean;
  initialMaxConcurrentManual?: string;
  initialPrMergePollInterval?: string;
  initialGenerateReport?: boolean;
  initialWorkItemLogRetention?: string;
}) {
  const [pollInterval, setPollInterval] = useState(initialPollInterval);
  const [maxParallelPerUser, setMaxParallelPerUser] = useState(initialMaxParallelPerUser);
  const [maxConcurrentManual, setMaxConcurrentManual] = useState(initialMaxConcurrentManual);
  const [prMergePollInterval, setPrMergePollInterval] = useState(initialPrMergePollInterval);
  const [generateReport, setGenerateReport] = useState(initialGenerateReport);
  const [workItemLogRetention, setWorkItemLogRetention] = useState(
    initialWorkItemLogRetention,
  );
  return (
    <div className="bg-gray-900 border border-gray-800 rounded-xl p-6">
      <GeneralLimitsFields
        pollInterval={pollInterval}
        maxParallelPerUser={maxParallelPerUser}
        maxConcurrentManual={maxConcurrentManual}
        prMergePollInterval={prMergePollInterval}
        generateReport={generateReport}
        workItemLogRetention={workItemLogRetention}
        onPollIntervalChange={setPollInterval}
        onMaxParallelPerUserChange={setMaxParallelPerUser}
        onMaxConcurrentManualChange={setMaxConcurrentManual}
        onPrMergePollIntervalChange={setPrMergePollInterval}
        onGenerateReportChange={setGenerateReport}
        onWorkItemLogRetentionChange={setWorkItemLogRetention}
      />
    </div>
  );
}

const meta = {
  title: "Components/GeneralLimitsFields",
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

export const Defaults: Story = {
  name: "Defaults (unlimited / report off)",
  render: () => <Harness />,
};

export const Populated: Story = {
  name: "Populated (report on)",
  render: () => (
    <Harness
      initialMaxConcurrentManual="3"
      initialPrMergePollInterval="30"
      initialGenerateReport
      initialWorkItemLogRetention="14"
    />
  ),
};

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState } from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import { JiraContextFields } from "../components/admin/JiraContextFields";
import type { LinkedItemsInPrompt } from "../api/types";

/**
 * Local-state harness so each story behaves like a real form (typing into
 * fields, changing the linked-items select, editing the project-keys chips).
 */
function Harness({
  initialLinkedItemsInPrompt = "full",
  initialTicketContextMaxDescriptionBytes = "0",
  initialLinkedIssueDescriptionMaxBytes = "0",
  initialJqlFilter = "",
  initialDoneStatus = "",
  initialProjectKeys = [],
}: {
  initialLinkedItemsInPrompt?: LinkedItemsInPrompt;
  initialTicketContextMaxDescriptionBytes?: string;
  initialLinkedIssueDescriptionMaxBytes?: string;
  initialJqlFilter?: string;
  initialDoneStatus?: string;
  initialProjectKeys?: string[];
}) {
  const [linkedItemsInPrompt, setLinkedItemsInPrompt] = useState<LinkedItemsInPrompt>(
    initialLinkedItemsInPrompt,
  );
  const [ticketContextMaxDescriptionBytes, setTicketContextMaxDescriptionBytes] = useState(
    initialTicketContextMaxDescriptionBytes,
  );
  const [linkedIssueDescriptionMaxBytes, setLinkedIssueDescriptionMaxBytes] = useState(
    initialLinkedIssueDescriptionMaxBytes,
  );
  const [jqlFilter, setJqlFilter] = useState(initialJqlFilter);
  const [doneStatus, setDoneStatus] = useState(initialDoneStatus);
  const [projectKeys, setProjectKeys] = useState<string[]>(initialProjectKeys);
  return (
    <div className="bg-gray-900 border border-gray-800 rounded-xl p-6">
      <JiraContextFields
        linkedItemsInPrompt={linkedItemsInPrompt}
        ticketContextMaxDescriptionBytes={ticketContextMaxDescriptionBytes}
        linkedIssueDescriptionMaxBytes={linkedIssueDescriptionMaxBytes}
        jqlFilter={jqlFilter}
        doneStatus={doneStatus}
        projectKeys={projectKeys}
        onLinkedItemsInPromptChange={setLinkedItemsInPrompt}
        onTicketContextMaxDescriptionBytesChange={setTicketContextMaxDescriptionBytes}
        onLinkedIssueDescriptionMaxBytesChange={setLinkedIssueDescriptionMaxBytes}
        onJqlFilterChange={setJqlFilter}
        onDoneStatusChange={setDoneStatus}
        onProjectKeysChange={setProjectKeys}
      />
    </div>
  );
}

const meta = {
  title: "Components/JiraContextFields",
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
  name: "Defaults (full inclusion, unlimited)",
  render: () => <Harness />,
};

export const Populated: Story = {
  name: "Populated (summary-only, capped, filtered)",
  render: () => (
    <Harness
      initialLinkedItemsInPrompt="summary_only"
      initialTicketContextMaxDescriptionBytes="8192"
      initialLinkedIssueDescriptionMaxBytes="2048"
      initialJqlFilter='labels = "maestro"'
      initialDoneStatus="Done"
      initialProjectKeys={["PROJ", "OPS"]}
    />
  ),
};

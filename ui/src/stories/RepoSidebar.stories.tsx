// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState } from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import { RepoSidebar, type RepoSidebarItem } from "../components/RepoSidebar";

/** Local-state harness so selection is interactive in the story. */
function Harness({ repos, initial = null }: { repos: RepoSidebarItem[]; initial?: string | null }) {
  const [selected, setSelected] = useState<string | null>(initial);
  return (
    <div className="max-w-xs">
      <RepoSidebar repos={repos} loading={false} selected={selected} onSelect={setSelected} />
    </div>
  );
}

const meta = {
  title: "Components/RepoSidebar",
  parameters: {
    layout: "fullscreen",
    backgrounds: { default: "dark", values: [{ name: "dark", value: "#030712" }] },
  },
  decorators: [
    (Story) => (
      <div className="p-8">
        <Story />
      </div>
    ),
  ],
} satisfies Meta;

export default meta;
type Story = StoryObj<typeof meta>;

export const Plain: Story = {
  name: "Plain (no badges)",
  render: () => <Harness repos={[{ name: "quantum-budget" }, { name: "cheat-sheets" }]} initial="quantum-budget" />,
};

export const WithCommandBadges: Story = {
  name: "With set/none badges",
  render: () => (
    <Harness
      repos={[
        { name: "quantum-budget", hasCommands: true },
        { name: "cheat-sheets", hasCommands: false },
      ]}
      initial="quantum-budget"
    />
  ),
};

export const WithNoAccess: Story = {
  name: "One repo lost access (sorted last, disabled)",
  render: () => (
    <Harness
      repos={[
        { name: "quantum-budget", accessible: false, hasCommands: true },
        { name: "cheat-sheets", accessible: true, hasCommands: false },
      ]}
      initial="cheat-sheets"
    />
  ),
};

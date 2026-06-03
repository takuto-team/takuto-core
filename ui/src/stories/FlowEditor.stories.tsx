// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { FlowEditor } from "../components/FlowEditor";
import type { UserFlow } from "../api/flows";

const implement: UserFlow = {
  name: "implement_ticket",
  depends_on: [],
  steps: [
    {
      name: "Implement",
      prompt:
        "Implement the changes described in the ticket. Do not commit; a follow-up step handles commits.",
      skills: [],
    },
    {
      name: "Review",
      prompt:
        "Review your changes against `{base_branch}`. Address any genuine findings; leave a # TODO for anything out of scope.",
      skills: [],
    },
  ],
};

const review: UserFlow = {
  name: "review_changes",
  depends_on: ["implement_ticket"],
  steps: [
    {
      name: "Code review",
      prompt: "Walk the diff against `{base_branch}` and call out issues, with severity labels.",
      skills: [{ name: "review-rubric", args: ["--strict"] }],
    },
  ],
};

const createPr: UserFlow = {
  name: "create_pr",
  depends_on: ["review_changes"],
  steps: [
    {
      name: "Open PR",
      prompt: "Open a pull request targeting `{base_branch}` with a conventional-commit title.",
      skills: [{ name: "create-pr", args: ["--no-draft"] }],
    },
  ],
};

const sampleFlows: UserFlow[] = [implement, review, createPr];

const noopSubmit = async () => {};
const failingSubmit = async () => {
  throw new Error("This name is already taken by another flow in this workspace.");
};

const meta = {
  title: "Components/FlowEditor",
  component: FlowEditor,
  parameters: {
    layout: "padded",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
  args: {
    flows: sampleFlows,
    editIndex: null,
    name: "",
    onSubmit: fn(noopSubmit),
    onCancel: fn(),
  },
} satisfies Meta<typeof FlowEditor>;

export default meta;
type Story = StoryObj<typeof meta>;

export const AddFlow: Story = {
  name: "Add — fresh draft",
  args: { name: "lint_and_test" },
};

export const EditFlow: Story = {
  name: "Edit — pre-populated",
  args: {
    editIndex: 0,
    name: "implement_ticket",
  },
};

export const EditMultiStepWithSkills: Story = {
  name: "Edit — multi-step + skills",
  args: {
    editIndex: 2,
    name: "create_pr",
  },
};

const cycleSetup: UserFlow[] = [
  { ...implement, depends_on: ["create_pr"] },
  review,
  createPr,
];

export const CycleSetup: Story = {
  name: "Cycle warning visible",
  args: {
    flows: cycleSetup,
    editIndex: 0,
    name: "implement_ticket",
  },
};

export const ServerRejects: Story = {
  name: "Server rejection on save",
  args: {
    editIndex: 0,
    name: "implement_ticket",
    onSubmit: fn(failingSubmit),
  },
};

export const NoSiblings: Story = {
  name: "Add — no siblings to depend on",
  args: {
    flows: [],
    name: "first_flow",
  },
};

import type { Meta, StoryObj } from "@storybook/react-vite";
import { SummaryStats } from "../components/SummaryStats";
import type { WorkflowSummary } from "../api/types";

function makeWorkflow(state: string): WorkflowSummary {
  return {
    id: "wf-1",
    ticket_key: "PROJ-1",
    ticket_summary: "Sample ticket",
    ticket_description: "",
    ticket_type: "Task",
    state,
    started_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    branch_name: "feat/proj-1",
    pr_url: null,
    pr_merged: false,
    steps_log: [],
    error: null,
    terminal_lines: [],
    can_mark_done: false,
    can_delete: true,
    can_start: false,
    progress_percent: 0,
    progress_steps_total: 0,
    started_manually: false,
    counts_toward_manual_cap: false,
    jira_browse_url: "",
    issue_url: null,
    can_open_editor: false,
    editor_url: null,
    editor_port_mappings: [],
    jira_available: false,
    ticketing_system: "github",
    can_resume_from_error: false,
    terminal_url: null,
    run_commands: [],
    generate_report: false,
    has_report: false,
    workflow_def_runs: {},
    worktree_path: undefined,
  };
}

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
} satisfies Meta<typeof SummaryStats>;

export default meta;
type Story = StoryObj<typeof meta>;

export const NoRepo: Story = {
  name: "No repository cloned",
  args: {
    workflows: [],
  },
};

export const WithRepo: Story = {
  name: "With repository (clickable link)",
  args: {
    workflows: [],
    repoName: "maestro-core",
    repoHtmlUrl: "https://github.com/morphet81/maestro-core",
  },
};

export const WithRepoNoUrl: Story = {
  name: "With repository (no remote URL)",
  args: {
    workflows: [],
    repoName: "my-private-repo",
    repoHtmlUrl: null,
  },
};

export const WithRepoAndCounts: Story = {
  name: "With repository + active workflows",
  args: {
    repoName: "maestro-core",
    repoHtmlUrl: "https://github.com/morphet81/maestro-core",
    workflows: [
      makeWorkflow("Running"),
      makeWorkflow("Running"),
      makeWorkflow("Done"),
      makeWorkflow("Done"),
      makeWorkflow("Done"),
      makeWorkflow("Error: something failed"),
      makeWorkflow("Paused"),
    ],
  },
};

export const CountsOnly: Story = {
  name: "No repository, active workflows",
  args: {
    workflows: [
      makeWorkflow("Running"),
      makeWorkflow("Done"),
      makeWorkflow("Error: lint failed"),
      makeWorkflow("Paused"),
    ],
  },
};

export const LongRepoName: Story = {
  name: "With long repository name",
  args: {
    workflows: [],
    repoName: "my-very-long-repository-name-that-might-overflow",
    repoHtmlUrl: "https://github.com/some-org/my-very-long-repository-name-that-might-overflow",
  },
};

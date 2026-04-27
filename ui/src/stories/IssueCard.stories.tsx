import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { IssueCard } from "../components/IssueCard";
import type { WorkflowSummary } from "../api/types";
import { ToastProvider } from "../hooks/useToast";

const baseWorkflow: WorkflowSummary = {
  id: "wf-1",
  ticket_key: "PROJ-123",
  ticket_summary: "Implement user authentication with OAuth2",
  ticket_description: "Add OAuth2 login flow using GitHub as the identity provider. Include logout, session management, and protected route handling.",
  ticket_type: "Task",
  state: "Pending",
  started_at: new Date(Date.now() - 1000 * 60 * 5).toISOString(),
  updated_at: new Date().toISOString(),
  branch_name: "feat/proj-123-implement-user-authentication",
  pr_url: null,
  pr_merged: false,
  steps_log: [],
  error: null,
  terminal_lines: [],
  can_mark_done: false,
  can_delete: true,
  can_start: true,
  progress_percent: 0,
  progress_steps_total: 0,
  started_manually: true,
  counts_toward_manual_cap: true,
  jira_browse_url: "https://example.atlassian.net/browse/PROJ-123",
  can_open_editor: false,
  editor_url: null,
  editor_port_mappings: [],
  jira_available: true,
  ticketing_system: "jira",
  can_resume_from_error: false,
  terminal_url: null,
  run_commands: [],
  generate_report: false,
  has_report: false,
  workflow_def_runs: {},
  worktree_path: undefined,
};

const defaultProps = {
  dynamicForwards: [] as [number, number][],
  workflowDefs: [],
  onRefresh: fn(),
  onShowDescription: fn(),
  onReport: fn(),
};

const meta = {
  title: "Components/IssueCard",
  component: IssueCard,
  parameters: {
    layout: "padded",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
  decorators: [
    (Story: React.ComponentType) => (
      <ToastProvider>
        <div style={{ maxWidth: "600px", margin: "0 auto" }}>
          <Story />
        </div>
      </ToastProvider>
    ),
  ],
} satisfies Meta<typeof IssueCard>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Pending: Story = {
  args: {
    ...defaultProps,
    workflow: {
      ...baseWorkflow,
      state: "Pending",
      can_start: true,
      can_delete: true,
    },
  },
};

export const PendingWithWorktree: Story = {
  name: "Pending (worktree ready)",
  args: {
    ...defaultProps,
    workflow: {
      ...baseWorkflow,
      state: "Pending",
      can_start: true,
      worktree_path: "/home/maestro/worktrees/proj-123",
    },
  },
};

export const PendingPreparingWorktree: Story = {
  name: "Pending (preparing worktree)",
  args: {
    ...defaultProps,
    workflow: {
      ...baseWorkflow,
      state: "Pending",
      can_start: true,
      branch_name: "feat/proj-123-implement-user-authentication",
      worktree_path: undefined,
    },
  },
};

export const Running: Story = {
  args: {
    ...defaultProps,
    workflow: {
      ...baseWorkflow,
      state: "Running",
      can_start: false,
      can_delete: false,
      can_mark_done: false,
      progress_percent: 45,
      progress_steps_total: 5,
    },
    terminalState: {
      stepName: "implement",
      lines: [
        { text: "Running Claude Code agent...", stream: "stdout" },
        { text: "Reading repository structure", stream: "stdout" },
        { text: "Implementing OAuth2 flow", stream: "stdout" },
      ],
      completed: false,
    },
  },
};

export const Paused: Story = {
  args: {
    ...defaultProps,
    workflow: {
      ...baseWorkflow,
      state: "Paused",
      can_start: false,
      progress_percent: 60,
      progress_steps_total: 5,
    },
    terminalState: {
      stepName: "review",
      lines: [
        { text: "Workflow paused by user.", stream: "stdout" },
      ],
      completed: false,
    },
  },
};

export const Completed: Story = {
  args: {
    ...defaultProps,
    workflow: {
      ...baseWorkflow,
      state: "Done",
      can_start: false,
      can_delete: true,
      can_mark_done: true,
      progress_percent: 100,
      progress_steps_total: 5,
      pr_url: "https://github.com/org/repo/pull/42",
      terminal_lines: [
        { text: "All steps completed successfully.", stream: "stdout" },
        { text: "PR created: https://github.com/org/repo/pull/42", stream: "stdout" },
      ],
    },
  },
};

export const CompletedMerged: Story = {
  name: "Completed (PR merged)",
  args: {
    ...defaultProps,
    workflow: {
      ...baseWorkflow,
      state: "Done",
      can_start: false,
      can_delete: true,
      can_mark_done: false,
      progress_percent: 100,
      progress_steps_total: 5,
      pr_url: "https://github.com/org/repo/pull/42",
      pr_merged: true,
      terminal_lines: [
        { text: "PR merged successfully.", stream: "stdout" },
      ],
    },
  },
};

export const Error: Story = {
  args: {
    ...defaultProps,
    workflow: {
      ...baseWorkflow,
      state: "Error: lint check failed with exit code 1",
      can_start: false,
      can_delete: true,
      can_mark_done: false,
      can_resume_from_error: true,
      progress_percent: 60,
      progress_steps_total: 5,
      error: "lint check failed with exit code 1",
      terminal_lines: [
        { text: "Running lint checks...", stream: "stdout" },
        { text: "ESLint: 3 errors found", stream: "stderr" },
        { text: "Process exited with code 1", stream: "stderr" },
      ],
    },
  },
};

export const Stopped: Story = {
  args: {
    ...defaultProps,
    workflow: {
      ...baseWorkflow,
      state: "Stopped",
      can_start: false,
      can_delete: true,
      progress_percent: 40,
      progress_steps_total: 5,
      terminal_lines: [
        { text: "Workflow stopped by user.", stream: "stdout" },
      ],
    },
  },
};

export const WithPortMappings: Story = {
  name: "Running (with port mappings)",
  args: {
    ...defaultProps,
    workflow: {
      ...baseWorkflow,
      state: "Running",
      can_open_editor: true,
      editor_url: "https://editor.example.com",
      editor_port_mappings: [[3000, 13000], [5173, 15173]] as [number, number][],
      progress_percent: 30,
      progress_steps_total: 3,
    },
    dynamicForwards: [[8080, 18080]] as [number, number][],
  },
};

export const CompletedWithRunCommands: Story = {
  name: "Completed (run commands: done / pending / disabled)",
  args: {
    ...defaultProps,
    workflow: {
      ...baseWorkflow,
      state: "Done",
      can_start: false,
      can_delete: true,
      can_mark_done: true,
      progress_percent: 100,
      progress_steps_total: 5,
      pr_url: "https://github.com/org/repo/pull/42",
      terminal_lines: [
        { text: "All steps completed successfully.", stream: "stdout" },
      ],
      run_commands: [
        { index: 0, name: "Dev server", running: true, forwarded_port: [3000, 13000] },
        { index: 1, name: "Storybook", running: false, forwarded_port: null },
        { index: 2, name: "E2E tests", running: false, forwarded_port: null, disabled: true },
      ],
    },
  },
};

export const GitHubTicketing: Story = {
  name: "Pending (GitHub Issues)",
  args: {
    ...defaultProps,
    workflow: {
      ...baseWorkflow,
      state: "Pending",
      can_start: true,
      ticketing_system: "github",
      jira_available: false,
      jira_browse_url: "https://github.com/org/repo/issues/42",
    },
  },
};

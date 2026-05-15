// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

export interface TerminalLine {
  text: string;
  stream: string;
}

export interface StepLog {
  name: string;
  status: string;
  started_at?: string;
  completed_at?: string;
  error?: string;
}

export interface WorkflowSummary {
  id: string;
  ticket_key: string;
  ticket_summary: string;
  ticket_description: string;
  ticket_type: string;
  state: string;
  started_at: string;
  updated_at: string;
  branch_name: string;
  pr_url: string | null;
  pr_merged: boolean;
  steps_log: StepLog[];
  error: string | null;
  terminal_lines: TerminalLine[];
  can_mark_done: boolean;
  can_delete: boolean;
  can_start: boolean;
  progress_percent: number;
  progress_steps_total: number;
  started_manually: boolean;
  counts_toward_manual_cap: boolean;
  jira_browse_url: string;
  issue_url: string | null;
  can_open_editor: boolean;
  editor_url: string | null;
  editor_port_mappings: [number, string][];
  jira_available: boolean;
  ticketing_system: string;
  can_resume_from_error: boolean;
  terminal_url: string | null;
  run_commands: RunCommandStatus[];
  generate_report: boolean;
  has_report: boolean;
  workflow_def_runs: Record<string, string>;
  /** Absolute path of the git worktree on disk. Absent while being pre-created in the background. */
  worktree_path?: string;
  /** Name of the repository (workspace) the workflow belongs to. Plan-10.
   *  Always present on the wire; may be empty string for legacy snapshots
   *  that pre-date workspace_name being recorded. */
  workspace_name: string;
  /** UUID of the repository row the workflow belongs to. Plan-10.
   *  `None` for legacy snapshots not yet back-filled by reconciliation. */
  repository_id?: string;
}

export interface RunCommandStatus {
  index: number;
  name: string;
  running: boolean;
  forwarded_port: [number, string] | null;
}

export interface WorkflowEvent {
  event_type: string;
  workflow_id: string;
  ticket_key: string;
  state: string;
  step_name?: string;
  output_line?: string;
  stream?: string;
  error?: string;
  progress_percent?: number;
  progress_steps_total?: number;
  forwarded_port?: [number, number];
  pr_merged?: boolean;
  workflow_def_name?: string;
}

export interface ConfigResponse {
  general: {
    dry_mode: boolean;
    max_concurrent_workflows: number;
    max_active_workflows: number;
    max_concurrent_manual_workflows: number;
    ticketing_system: string;
    [key: string]: unknown;
  };
  agent?: {
    improve_timeout_secs?: number;
    [key: string]: unknown;
  };
  jira: {
    project_keys: string[];
    site: string;
    [key: string]: unknown;
  };
  github: {
    app_id: number;
    app_installation_id: number;
    app_name?: string;
    [key: string]: unknown;
  };
  web: {
    dashboard_username: string;
    [key: string]: unknown;
  };
  jira_available: boolean;
  ticketing_system: string;
  github_app_configured: boolean;
  github_app_name?: string | null;
  preflight_error?: string | null;
  repo_exists: boolean;
  repo_name?: string | null;
  repo_html_url?: string | null;
  [key: string]: unknown;
}

export interface Workspace {
  name: string;
  html_url?: string | null;
  active: boolean;
}

export interface WorkflowCounts {
  running: number;
  completed: number;
  errors: number;
  paused: number;
}

export interface GitHubRepo {
  full_name: string;
  description: string;
  private: boolean;
  html_url: string;
}

export interface PollingStatus {
  paused: boolean;
}

export interface AuthStatus {
  dashboard_auth_enabled: boolean;
  multi_user: boolean;
  setup_required: boolean;
}

export interface User {
  id: string;
  username: string;
  role: "admin" | "user";
  suspended: boolean;
  created_at: string;
  updated_at: string;
}

export interface TodoTicket {
  key: string;
  summary: string;
}

export interface TicketPreview {
  key: string;
  summary: string;
  description_markdown: string;
}

export interface GitHubIssue {
  key: string;
  summary: string;
  body: string;
  url: string;
}

export interface OpenEditorResponse {
  url: string;
  connection_token: string;
  vscode_port: number;
  port_mappings: [number, number][];
}

export interface OpenTerminalResponse {
  url: string;
  credential: string;
}

export interface MarkDoneOutcome {
  jira_ok: boolean;
  worktree_ok: boolean;
  jira_error?: string;
  worktree_error?: string;
}

export interface WorkflowDefinition {
  filename: string;
  name: string;
  steps: unknown[];
  depends_on: string[];
  valid: boolean;
  error?: string;
}

export interface ImproveResponse {
  improved_description: string;
  improved_summary?: string;
}

export interface PromptResponse {
  response: string;
}

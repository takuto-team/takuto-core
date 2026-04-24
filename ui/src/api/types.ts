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
  can_address_pr_comments: boolean;
  can_merge_base: boolean;
  can_mark_done: boolean;
  can_delete: boolean;
  can_start: boolean;
  progress_percent: number;
  progress_steps_total: number;
  started_manually: boolean;
  counts_toward_manual_cap: boolean;
  jira_browse_url: string;
  can_open_editor: boolean;
  editor_url: string | null;
  editor_port_mappings: [number, number][];
  jira_available: boolean;
  ticketing_system: string;
  can_resume_from_error: boolean;
  terminal_url: string | null;
  run_commands: RunCommandStatus[];
  generate_report: boolean;
  has_report: boolean;
  workflow_def_runs: Record<string, string>;
}

export interface RunCommandStatus {
  index: number;
  name: string;
  running: boolean;
  forwarded_port: [number, number] | null;
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
  jira: {
    project_keys: string[];
    site: string;
    [key: string]: unknown;
  };
  github: {
    app_id: number;
    app_installation_id: number;
    [key: string]: unknown;
  };
  web: {
    dashboard_username: string;
    [key: string]: unknown;
  };
  jira_available: boolean;
  ticketing_system: string;
  github_app_configured: boolean;
  preflight_error?: string | null;
  [key: string]: unknown;
}

export interface PollingStatus {
  paused: boolean;
}

export interface AuthStatus {
  dashboard_auth_enabled: boolean;
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

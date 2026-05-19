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
  /**
   * Auth-overhaul Phase 1: `provider_changed` events carry from/to instead of
   * workflow fields (which are sent empty). 04_architecture.md §2.3.
   */
  from?: string;
  to?: string;
  affected_users?: string[];
}

// ---------------------------------------------------------------------------
// Agent config (Phase 1 — auth-overhaul).
//
// Mirrors the Rust sub-tables in tmp/multi-agents/04_architecture.md §2.2.
// Each provider table is its own interface so callers can patch a single
// provider without bloating the parent type. All fields are optional in the
// patch shapes so a `PUT /api/config/agent` body can carry deltas.
// ---------------------------------------------------------------------------

/** Identifier of a v1 AI provider, plus the v2 placeholder + "none" disable. */
export type AgentProviderId =
  | "claude"
  | "cursor"
  | "codex"
  | "opencode"
  | "gemini"
  | "none";

/** Common shape: every provider has `model`, `extra_args`, `allow_shared_default`. */
interface AgentProviderConfigBase {
  model: string;
  extra_args: string[];
  allow_shared_default: boolean;
}

export interface AgentClaudeConfig extends AgentProviderConfigBase {
  base_url: string;
}

/**
 * Cursor's CLI does not support custom upstream endpoints (amendment A1 in
 * 04_architecture.md). The `base_url` field is intentionally absent here so
 * the dashboard never lets an admin set one.
 */
export interface AgentCursorConfig extends AgentProviderConfigBase {
  cli: string;
}

export interface AgentCodexConfig extends AgentProviderConfigBase {
  /** Named entry in `~/.codex/config.toml [model_providers]`. */
  provider_name: string;
  base_url: string;
}

export interface AgentOpenCodeConfig extends AgentProviderConfigBase {
  base_url: string;
}

export interface AgentGeminiConfig extends AgentProviderConfigBase {
  base_url: string;
}

export interface AgentProvidersConfig {
  claude?: AgentClaudeConfig;
  cursor?: AgentCursorConfig;
  codex?: AgentCodexConfig;
  opencode?: AgentOpenCodeConfig;
  gemini?: AgentGeminiConfig;
}

/** Top-level [agent] table as returned by `GET /api/config`. */
export interface AgentConfig {
  provider: AgentProviderId;
  available_providers: AgentProviderId[];
  step_timeout_secs?: number;
  improve_timeout_secs?: number;
  providers: AgentProvidersConfig;
  /** Forward-compat for unknown fields surfaced by older / newer servers. */
  [key: string]: unknown;
}

// --- Patch shapes -----------------------------------------------------------

/** Partial provider patch: only the keys the admin actually changed. */
export type AgentClaudeConfigPatch = Partial<AgentClaudeConfig>;
export type AgentCursorConfigPatch = Partial<AgentCursorConfig>;
export type AgentCodexConfigPatch = Partial<AgentCodexConfig>;
export type AgentOpenCodeConfigPatch = Partial<AgentOpenCodeConfig>;

export interface AgentProvidersConfigPatch {
  claude?: AgentClaudeConfigPatch;
  cursor?: AgentCursorConfigPatch;
  codex?: AgentCodexConfigPatch;
  opencode?: AgentOpenCodeConfigPatch;
}

/**
 * Body for `PUT /api/config/agent`. All fields optional — the server treats
 * absent keys as "leave alone" and rejects unknown keys via
 * `deny_unknown_fields` (04_architecture.md §2.3).
 */
export interface AgentConfigPatch {
  provider?: AgentProviderId;
  available_providers?: AgentProviderId[];
  providers?: AgentProvidersConfigPatch;
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
  /**
   * Phase 1 (auth-overhaul) populates this with the full sub-table shape.
   * Pre-Phase-1 servers send only `improve_timeout_secs` and the agent table
   * may be absent altogether — hence the optional. Callers that need the
   * structured shape should also tolerate a partial / undefined value.
   */
  agent?: Partial<AgentConfig> & {
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
  /**
   * Present on the response of `PUT /api/config` and `PUT /api/config/agent`.
   * Anchored to `crates/maestro-web/src/routes/config.rs::UpdateConfigResponse`
   * (the backend uses `#[serde(flatten)]` on the config, so these fields
   * appear at the top level alongside `general` / `agent` / etc.).
   *
   * - `persisted: true`: in-memory patch AND disk write both succeeded.
   * - `persisted: false`: patch is live in memory but the on-disk write
   *   failed (read-only mount, EACCES, etc.) — admin must fix the mount
   *   or the change is lost at restart.
   * - `persist_warning`: human-readable error from the write attempt.
   *   Backend omits the key on success (serde `skip_serializing_if`), so
   *   it's optional on the wire.
   *
   * Both fields are absent on `GET /api/config` — they only appear on PUT
   * responses. Use `persisted === false` (strict) to detect failure so
   * legacy servers that don't return the field default to "assume OK".
   */
  persisted?: boolean;
  persist_warning?: string | null;
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
  /** Phase 0 (04_architecture §1.3): present once the extended /api/auth/status
   *  ships in the auth-overhaul Rust changes. Optional for back-compat. */
  provider_selected?: string;
  github_mode?: string;
  degraded?: boolean;
}

/**
 * Severity of a structured onboarding warning. Mirrors the Rust enum in
 * 04_architecture.md §1.2. `critical` blocks the user from running workflows
 * and is what the dashboard banner surfaces; `warning` / `info` are reserved
 * for non-blocking hints (Phase 1+).
 */
export type WarningSeverity = "critical" | "warning" | "info";

/** A single entry in `SystemStatus.warnings`. See 04_architecture.md §1.2. */
export interface StructuredWarning {
  code: string;
  severity: WarningSeverity;
  message: string;
}

/**
 * Phase 0 system status returned by `GET /api/onboarding/status`. Mirrors the
 * Rust struct in 04_architecture.md §1.2. The dashboard banner is derived
 * from `warnings` (filtered to `severity === "critical"`).
 *
 * Back-compat: when the server is older and the endpoint 404s, the dashboard
 * falls back to the legacy `ConfigResponse.preflight_error` string.
 */
export interface SystemStatus {
  config_toml_ok: boolean;
  github: {
    mode: "app" | "pat_required" | "missing";
    app_configured: boolean;
    app_id: number | null;
    app_name: string | null;
  };
  provider: {
    selected: string;
    deployment_default_credential_present: boolean;
    headless_capable: boolean;
    custom_base_url: string | null;
  };
  ticketing: {
    system: "none" | "jira" | "github";
    acli_ok: boolean;
  };
  per_user_required: boolean;
  warnings: StructuredWarning[];
}

export interface User {
  id: string;
  username: string;
  role: "admin" | "user";
  suspended: boolean;
  created_at: string;
  updated_at: string;
}

// ---------------------------------------------------------------------------
// Per-user credentials (Phase 2 — auth-overhaul).
//
// Source of truth: tmp/multi-agents/04_architecture.md §3 (per-user provider
// store) + §4 (GitHub auth resolver) + 05_ux_design.md §2.2 / §2.3.
//
// **A3 rename:** the architecture renamed `sign_commits` to
// `attribute_commits` BEFORE any rows existed (no DB migration needed). The
// task description text still mentioned `sign_commits` but the team-lead's
// dispatch and the architecture doc are explicit: the field is
// `attribute_commits` and v1 does NOT do GPG/SSH signing. Honour the rename.
// ---------------------------------------------------------------------------

export type ProviderCredentialKind = "api_key" | "cli_state" | "oauth_token";

/**
 * What credential the user currently has stored for the deployment-wide
 * active provider. Matches the wire shape returned by
 * `crates/maestro-web/src/routes/credentials.rs::ProviderCredentialStatus`
 * — do NOT rename these fields without also updating the Rust struct.
 *
 * - `provider`: "claude" | "cursor" | "codex" | "opencode" — the provider
 *   the row was sealed for.
 * - `active`: true when the row is NOT a leftover from a provider switch
 *   (inactive rows are kept for audit/restore per 04_architecture.md §2.4).
 * - `last_used_at`: stamped by the engine on every workflow start.
 */
export interface UserProviderCredentialStatus {
  provider: string;
  kind: ProviderCredentialKind;
  active: boolean;
  last_validated_at: string | null;
  last_used_at: string | null;
}

/**
 * Possible effective GitHub auth modes for the deployment + user pair. This
 * value lives on `/api/auth/status::github_mode` — the per-user credential
 * status returned by `/api/users/me/credentials` does NOT carry it.
 */
export type GithubAuthMode =
  | "app"
  | "app_plus_pat"
  | "pat_only"
  | "pat_required"
  | "missing";

/**
 * Per-user GitHub credential. Matches the wire shape returned by
 * `crates/maestro-web/src/routes/credentials.rs::GithubCredentialStatus`
 * — do NOT add fields without also updating the Rust struct.
 *
 * The presence of a PAT is implied by the parent's `github` field being
 * non-null (the backend wraps this in `Option<...>`). There is no `has_pat`
 * here, and `mode` lives on `/api/auth/status::github_mode`, not here.
 */
export interface UserGithubCredentialStatus {
  login: string;
  scopes: string[];
  /** A3: per-user toggle that sets `GIT_AUTHOR_*` / `GIT_COMMITTER_*` env vars
   *  on the worker. NOT GPG/SSH signing. */
  attribute_commits: boolean;
  last_validated_at: string | null;
}

export interface UserCredentialsStatus {
  /** `null` when the user has not yet captured a provider credential. */
  provider: UserProviderCredentialStatus | null;
  /** `null` when the user has not captured a PAT (App-only / missing mode). */
  github: UserGithubCredentialStatus | null;
}

/** Body for `POST /api/users/me/credentials/{provider}`. Cursor is A1 — the
 *  CLI-state path is dropped, every provider takes a raw API key here. */
export interface SetProviderCredentialRequest {
  api_key: string;
}

/** Body for `POST /api/users/me/github-pat`. A3 rename applied. */
export interface SetGithubPatRequest {
  pat: string;
  attribute_commits?: boolean;
}

/** Body for `PATCH /api/users/me/github` (A3 rename). */
export interface PatchGithubSettingsRequest {
  attribute_commits: boolean;
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

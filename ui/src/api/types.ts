// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// ===========================================================================
// DRIFT-PROTECTION BOUNDARY — read before editing.
//
// This file has TWO halves:
//
// 1. GENERATED (re-exported just below): mirrored from the Rust DTOs by ts-rs
//    into `./generated/*.ts`. These are drift-protected — the `ts-types-drift`
//    CI job regenerates and fails if the committed TS differs from the Rust
//    source of truth. Do NOT hand-edit the generated files or re-declare these
//    shapes here. To change one, edit the Rust DTO (e.g.
//    `crates/takuto-web/src/routes/workflows/dto.rs`) and run
//    `./scripts/generate-ts-types.sh`, then commit the regenerated files.
//      Generated set: WorkflowSummary, RunCommandStatus, TerminalLine,
//      WorkflowCounts, StepLog, StepStatus, GitHubRepo, GitHubIssue,
//      TodoTicket, TicketPreview.
//
// 2. HAND-WRITTEN (everything else below): NOT drift-protected — kept manual on
//    purpose, for one of two reasons:
//    a) Shape can't be mirrored cleanly: `ConfigResponse`, every `*Patch` body,
//       and the `AgentConfig`/provider family use loose `[key: string]: unknown`
//       forward-compat shapes and (for ConfigResponse) mirror a
//       `#[serde(flatten)]` over the whole Rust `Config` tree, which ts-rs
//       cannot reproduce without breaking those consumers. `WorkflowEvent` must
//       stay a FLAT optional-field struct (consumers branch on `event_type` as a
//       plain string) — do NOT let ts-rs emit a tagged union for it without
//       refactoring useWorkflows.handleEvent.
//    b) Generating would break a consumer (stop-on-fallout): `PollingStatus`
//       would gain a required `reason` field, breaking the `{ paused }`
//       optimistic-update literal in usePolling.ts.
//    Other response types (AuthStatus, SystemStatus, MarkDoneOutcome,
//    WorkflowDefinition, Open*Response, Improve/PromptResponse, Workspace,
//    User, credentials) are simply still hand-synced; migrating each is
//    mechanical (a `#[derive(TS)]` + a co-located `ts_bindings` export test +
//    a re-export here) where it maps cleanly.
// ===========================================================================

export type { TerminalLine } from "./generated/TerminalLine";
export type { StepLog } from "./generated/StepLog";
export type { StepStatus } from "./generated/StepStatus";
export type { WorkflowSummary } from "./generated/WorkflowSummary";
export type { RunCommandStatus } from "./generated/RunCommandStatus";
export type { RepoPollingSettingsRow } from "./generated/RepoPollingSettingsRow";
export type { RepoPollingSettings } from "./generated/RepoPollingSettings";
export type { LinkedItemsPromptMode } from "./generated/LinkedItemsPromptMode";

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
  /** Cursor Privacy Mode (ghost mode). Default on; Cursor-specific. */
  privacy_mode?: boolean;
}

export interface AgentCodexConfig extends AgentProviderConfigBase {
  /** Named entry in `~/.codex/config.toml [model_providers]`. */
  provider_name: string;
  base_url: string;
}

export interface AgentOpenCodeConfig extends AgentProviderConfigBase {
  base_url: string;
  /**
   * Self-hosted-only token limits written into `models.<id>.limit` of the
   * synthesised `opencode.json`. `null`/absent = let OpenCode choose.
   */
  context_limit?: number | null;
  output_limit?: number | null;
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
  /** No-progress guardrail: consecutive identical output lines that abort a
   *  step. `0` disables the guardrail. */
  max_repeated_output_lines?: number;
  share_conversation_across_steps?: boolean;
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
  share_conversation_across_steps?: boolean;
  /** Per-step agent timeout (seconds). Server enforces a floor. */
  step_timeout_secs?: number;
  /** Timeout for the "improve description" call (seconds). Server floor. */
  improve_timeout_secs?: number;
  /** No-progress guardrail line count; `0` disables it. Server floor. */
  max_repeated_output_lines?: number;
  providers?: AgentProvidersConfigPatch;
}

/**
 * The `[polling]` section as surfaced by `GET /api/config`. Jira item *types*
 * are not duplicated here — they live at `config.jira.item_types`.
 */
export interface PollingConfig {
  auto_start_flow: string;
  max_parallel_items: number;
  max_parallel_per_user: boolean;
  jira: { summary_keywords: string[] };
  github: { labels: string[]; title_keywords: string[] };
}

/**
 * Patch body for `PUT /api/config/polling`. Every field is optional
 * (replace-on-present; arrays replace wholesale). The top-level `item_types`
 * patches `config.jira.item_types` — the generic `PUT /api/config` allowlist
 * cannot carry it.
 */
export interface ItemPollingConfigPatch {
  auto_start_flow?: string;
  max_parallel_items?: number;
  max_parallel_per_user?: boolean;
  jira?: { summary_keywords?: string[] };
  github?: { labels?: string[]; title_keywords?: string[] };
  item_types?: string[];
  /** Patches [general] poll_interval_secs; server enforces a >= 10 floor. */
  poll_interval_secs?: number;
  /** Enable/disable item polling. Patches [general] auto_polling and flips the
   *  live polling_paused flag immediately. */
  auto_polling?: boolean;
  /** [general] cap on manual starts occupying a slot at once; `0` = unlimited. */
  max_concurrent_manual_workflows?: number;
  /** [general] how often the PR-merge poller checks GitHub (seconds). */
  pr_merge_poll_interval_secs?: number;
  /** [general] default for whether workflows generate an end-of-run report. */
  generate_report?: boolean;
  /** [general] days of work-item logs to retain; `0` = keep forever. */
  work_item_log_retention_days?: number;
}

/**
 * Patch body for `PUT /api/config/git` — the operator-tunable portion of the
 * `[git]` section (the branch work-item branches are cut from and the remote
 * Takuto pushes to). Mirrors `agentConfig.ts` / `jiraConfig.ts`: a single PUT
 * returns the fresh redacted `ConfigResponse` (with `persisted` /
 * `persist_warning`). Both fields optional (replace-on-present); the server
 * rejects an all-empty body.
 */
export interface GitConfigPatch {
  /** Branch new work-item branches are created from (e.g. "main"). */
  base_branch?: string;
  /** Git remote Takuto fetches from and pushes branches to (e.g. "origin"). */
  remote?: string;
}

/** One of the three linked-issue inclusion modes for `linked_items_in_prompt`. */
export type LinkedItemsInPrompt = "full" | "summary_only" | "omit";

/**
 * Patch body for `PUT /api/config/jira` — the deployment-global Jira-context
 * *processing* fields of the `[jira]` section (how linked issues / the ticket
 * description are embedded in agent prompts, and the Mark-as-Done target). The
 * per-repo poll filters (project keys, item types, jql) live in
 * `/api/me/polling-settings`, NOT here. Every field optional (replace-on-present).
 */
export interface JiraConfigPatch {
  /** How linked issues are embedded in `{ticket_context}`. */
  linked_items_in_prompt?: LinkedItemsInPrompt;
  /** Byte cap on the main ticket description in context; `0` = unlimited. */
  ticket_context_max_description_bytes?: number;
  /** Byte cap on each linked-issue description in context; `0` = unlimited. */
  linked_issue_description_max_bytes?: number;
  /** Status the "Mark as Done" transition targets (default "Done"). */
  done_status?: string;
}

export interface ConfigResponse {
  general: {
    dry_mode: boolean;
    max_concurrent_workflows: number;
    max_active_workflows: number;
    max_concurrent_manual_workflows: number;
    poll_interval_secs: number;
    auto_polling: boolean;
    ticketing_system: string;
    pr_merge_poll_interval_secs?: number;
    generate_report?: boolean;
    work_item_log_retention_days?: number;
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
    /** Legacy global project-keys list. Per-user-per-repo keys now live under
     *  `/api/me/jira-project-keys`; this stays optional only for back-compat
     *  with older server responses. */
    project_keys?: string[];
    site: string;
    /** Issue types the Jira poller pulls. Patched via PUT /api/config/polling. */
    item_types?: string[];
    /** Jira-context fields patched via PUT /api/config/jira. */
    linked_items_in_prompt?: LinkedItemsInPrompt;
    ticket_context_max_description_bytes?: number;
    linked_issue_description_max_bytes?: number;
    jql_filter?: string;
    done_status?: string;
    [key: string]: unknown;
  };
  github: {
    app_id: number;
    app_installation_id: number;
    app_name?: string;
    [key: string]: unknown;
  };
  /**
   * The `[git]` section. `base_branch` / `remote` are patched via
   * `PUT /api/config/git` and pre-populate the wizard's Git step. Optional
   * (with optional inner fields) so a pre-feature server omitting them falls
   * back to the "main" / "origin" defaults.
   */
  git?: {
    base_branch?: string;
    remote?: string;
    repo_path?: string;
    [key: string]: unknown;
  };
  /**
   * Admin-tunable polling policy (the `[polling]` section). Optional because a
   * pre-feature server omits it; the Item Polling tab falls back to defaults.
   */
  polling?: PollingConfig;
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
   * Anchored to `crates/takuto-web/src/routes/config.rs::UpdateConfigResponse`
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

export type { WorkflowCounts } from "./generated/WorkflowCounts";

export type { GitHubRepo } from "./generated/GitHubRepo";

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
// Per-user credentials.
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
 * `crates/takuto-web/src/routes/credentials.rs::ProviderCredentialStatus`
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
 * Bundle returned by `GET /api/users/me/credentials` (task #39). For each
 * deployment-active provider a user can hold **two** rows in parallel —
 * one `api_key` row and one `cli_state` row — so the UI can render an
 * independent "Connected" pill per kind. Matches the Rust struct
 * `crates/takuto-web/src/routes/credentials.rs::ProviderCredentialBundle`.
 *
 * Both `api_key` and `cli_state` are independently nullable. The backend
 * elides absent slots from the wire JSON (serde
 * `skip_serializing_if = Option::is_none`), so consumers must treat the
 * keys as truly optional.
 *
 * Only Claude accepts `cli_state` today — for every other provider the
 * `cli_state` slot is always null. See 02_cursor_headless_auth.md
 * amendment A1 in the architecture doc.
 */
export interface UserProviderCredentialBundle {
  /** Provider name the bundle is for ("claude" / "cursor" / …). */
  provider: string;
  /** Present when the user has saved a bearer/API-key row. */
  api_key?: UserProviderCredentialStatus | null;
  /** Present when the user has uploaded a Claude `~/.claude.json` blob. */
  cli_state?: UserProviderCredentialStatus | null;
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
 * `crates/takuto-web/src/routes/credentials.rs::GithubCredentialStatus`
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

/**
 * Per-user Jira credential status. Matches the `jira` object returned by
 * `GET /api/users/me/credentials`. The API token itself is never returned.
 */
export interface UserJiraCredentialStatus {
  /** Normalized Atlassian site URL (e.g. `https://acme.atlassian.net`). */
  site: string;
  /** Atlassian account email the token belongs to. */
  email: string;
  account_id: string;
  account_name: string;
  last_validated_at: string | null;
}

export interface UserCredentialsStatus {
  /**
   * `null` when the user has no credential of any kind for the active
   * provider. When non-null, exposes the per-kind bundle (api_key +
   * cli_state) — at least one of the two inner slots is non-null.
   */
  provider: UserProviderCredentialBundle | null;
  /** `null` when the user has not captured a PAT (App-only / missing mode). */
  github: UserGithubCredentialStatus | null;
  /** `null` when the user has not captured a Jira credential. */
  jira?: UserJiraCredentialStatus | null;
}

/**
 * Body for `POST /api/users/me/credentials/{provider}`.
 *
 * Discriminated by `kind`:
 *   - `kind` omitted / `"api_key"`: `api_key` required, `claude_session_json`
 *     forbidden. The default — pre-task-#39 clients always use this.
 *   - `kind = "cli_state"`: `claude_session_json` required, `api_key`
 *     forbidden. **Only Claude** accepts this kind today; the server
 *     rejects it for every other provider with
 *     `error: "cli_state_only_supported_for_claude"`.
 *
 * Wire-shape note: matches `routes/credentials.rs::ApiKeyBody`. Cursor is
 * still A1 (API-key only at the schema level — the discriminator simply
 * never goes to `cli_state` there).
 */
export interface SetProviderCredentialRequest {
  /** Required when `kind` is omitted or `"api_key"`. */
  api_key?: string;
  /**
   * Required when `kind = "cli_state"`. Full `~/.claude.json` blob; the
   * server validates it parses and contains
   * `oauthAccount.{accountUuid, emailAddress, organizationUuid}`.
   */
  claude_session_json?: string;
  /** Defaults to `"api_key"` on the server when absent. */
  kind?: ProviderCredentialKind;
}

/** Body for `POST /api/users/me/github-pat`. A3 rename applied. */
export interface SetGithubPatRequest {
  pat: string;
  attribute_commits?: boolean;
}

/**
 * The three ticketing modes Takuto supports. Matches the `TicketingSystem`
 * enum in `crates/takuto-core/src/config/general.rs` (serde
 * `rename_all = "lowercase"`).
 */
export type TicketingSystemId = "none" | "jira" | "github";

/**
 * Body for the per-user Jira credential endpoint
 * (`POST /api/users/me/jira-credential`, `deny_unknown_fields`). The
 * credential is validated live against the Atlassian API, then stored
 * encrypted per-user and consumed by the Jira client / poller.
 */
export interface SetJiraCredentialRequest {
  /** Full Atlassian site URL, e.g. `https://acme.atlassian.net` (http(s):// required). */
  site: string;
  /** Atlassian account email the API token belongs to. */
  email: string;
  /** Atlassian API token. */
  token: string;
}

/**
 * 200 response from `POST /api/users/me/jira-credential`. The token is never
 * echoed back; the `account` block is resolved from Atlassian at save time.
 */
export interface JiraCredentialSaved {
  site: string;
  email: string;
  account: {
    account_id: string;
    display_name: string;
  };
}

/**
 * Patch body for the `[general]` portion of `PUT /api/config`. Mirrors the
 * backend `GeneralConcurrencyPatch` — every field optional, replace-on-present.
 */
export interface GeneralConfigPatch {
  ticketing_system?: TicketingSystemId;
  max_concurrent_workflows?: number;
  max_active_workflows?: number;
}

/** Patch body for `PUT /api/config` (the runtime dashboard patch). */
export interface RuntimeConfigPatch {
  general?: GeneralConfigPatch;
}

/** Body for `PATCH /api/users/me/github` (A3 rename). */
export interface PatchGithubSettingsRequest {
  attribute_commits: boolean;
}

export type { TodoTicket } from "./generated/TodoTicket";
export type { TicketPreview } from "./generated/TicketPreview";
export type { GitHubIssue } from "./generated/GitHubIssue";

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

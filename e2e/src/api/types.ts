/**
 * Wire shapes for the Takuto REST endpoints the onboarding e2e suite reads and
 * writes. Field names are confirmed against the working tree in `e2e/CONTRACT.md`
 * (§2 auth, §4 verification endpoints). Only the fields the specs assert on are
 * modelled — the server returns more, and structural typing lets the extra keys
 * ride along untyped.
 */

/** AI provider ids offered by the wizard (`ProviderStep.tsx:7`). */
export type ProviderId = 'claude' | 'cursor' | 'codex' | 'opencode';

/** Ticketing systems offered by the wizard (`TicketingStep.tsx:8`). */
export type TicketingId = 'none' | 'github' | 'jira';

/** Credentials for the first-admin registration / login flow (§2). */
export interface AdminCredentials {
  username: string;
  /** Must be ≥ 12 chars (`register.rs:72`). */
  password: string;
}

/** `GET /api/auth/status` (public) — subset the bootstrap helper reads (§2.1). */
export interface AuthStatus {
  setup_required: boolean;
  dashboard_auth_enabled: boolean;
  multi_user: boolean;
  provider_selected: boolean;
  github_mode: string;
}

/** `POST /api/auth/register` success body — `201 Created` (§2.2). */
export interface RegisterResponse {
  user_id: string;
  username: string;
  role: string;
  recovery_codes: string[];
  redirect_to: string;
}

/** Per-user onboarding block in `GET /api/onboarding/status` (§4). */
export interface UserOnboarding {
  step_1_ticketing: string | null;
  step_2_provider: string | null;
  step_3_github: string | null;
  step_4_credentials: string | null;
  /** Canonical "wizard finished" signal — non-null = done (`onboarding.rs:71-78`). */
  completed_at: string | null;
}

/** `GET /api/onboarding/status` — flattened status + per-user block (§4). */
export interface OnboardingStatus {
  /** Present only with a valid session cookie. */
  user_onboarding?: UserOnboarding;
  jira_credential_present?: boolean;
}

/** A provider sub-table under `agent.providers.<id>` in `GET /api/config` (§4). */
export interface AgentProviderConfig {
  base_url?: string;
  model?: string;
  extra_args?: string[];
}

/** `agent` section of the flattened `GET /api/config` (§4). */
export interface AgentConfig {
  provider: ProviderId;
  step_timeout_secs: number;
  providers: Record<string, AgentProviderConfig>;
}

/** `git` section of the flattened `GET /api/config` (§4). */
export interface GitConfig {
  base_branch: string;
  remote: string;
}

/** `general` section of the flattened `GET /api/config` (§4). */
export interface GeneralConfig {
  ticketing_system: TicketingId;
}

/** `GET /api/config` (auth required) — the subset the specs assert (§4). */
export interface ConfigView {
  git: GitConfig;
  agent: AgentConfig;
  general: GeneralConfig;
  /** Runtime mirror of the persisted ticketing system. */
  ticketing_system: TicketingId;
  github_app_configured: boolean;
  jira_available: boolean;
  config_writable: boolean;
}

/** One credential slot's status (api_key / cli_state) in the bundle. */
export interface CredentialSlotStatus {
  active: boolean;
}

/** Provider credential bundle returned by `GET /api/users/me/credentials` (§4). */
export interface ProviderCredentialBundle {
  provider: string;
  api_key?: CredentialSlotStatus;
  cli_state?: CredentialSlotStatus;
}

/** A stored GitHub PAT's status (no secret) (§4). */
export interface GithubCredentialStatus {
  login: string;
  scopes?: string[];
}

/** A stored Jira credential's status (no secret) (§4). */
export interface JiraCredentialStatus {
  site: string;
  email: string;
}

/** `GET /api/users/me/credentials[?provider=<p>]` — no secrets (§4). */
export interface UserCredentialsStatus {
  provider?: ProviderCredentialBundle | null;
  github?: GithubCredentialStatus | null;
  jira?: JiraCredentialStatus | null;
}

/** `POST /api/users/me/jira-credential` request body (§4; `useTicketingForm.ts:147-151`). */
export interface JiraCredentialRequest {
  site: string;
  email: string;
  /** Omit to keep the stored token. */
  token?: string;
}

/** `POST /api/users/me/credentials/{provider}` request body (§4). */
export interface ProviderCredentialRequest {
  /** The provider API key (shape-validated server-side; no live round-trip). */
  api_key: string;
  /** Credential kind; defaults to `api_key` server-side. */
  kind?: 'api_key';
}

// ---------------------------------------------------------------------------
// Implement-workflow surface (`IMPLEMENT_WORKFLOW_CONTRACT.md`). Only the fields
// the Part-B specs assert on are modelled; the server returns more.
// ---------------------------------------------------------------------------

/** A repository the caller has added (`repositories.ts:RepositoryRow`). */
export interface RepositoryRow {
  id: string;
  name: string;
  repo_url: string | null;
  local_path: string;
  default_branch: string;
}

/**
 * `POST /api/repositories` body. `{ repository_id }` adds an existing registered
 * repo to the caller's dashboard; `{ repo_url }` clones+adds a new one
 * (`repositories.rs`). Idempotent when already associated.
 */
export interface AddRepositoryRequest {
  repository_id?: string;
  repo_url?: string;
}

/** One run-command's name + shell command (`RunCommand`, `worktree_commands`). */
export interface RunCommand {
  name: string;
  command: string;
}

/**
 * `PUT /api/worktree-commands/{workspace}` body (`deny_unknown_fields`;
 * `worktree_commands.rs:96-105`). For the fixture: `init_commands = ["npm ci"]`
 * and one `run_commands` entry `{ name: "dev", command: "npm run dev" }`.
 */
export interface WorktreeCommandsRequest {
  init_commands: string[];
  run_commands: RunCommand[];
  generate_report: boolean;
}

/** A persisted `user_worktree_commands` row (`worktreeCommands.ts`). */
export interface WorktreeCommandsRow {
  workspace_name: string;
  init_commands: string[];
  run_commands: RunCommand[];
  generate_report: boolean;
  updated_at: number;
}

/**
 * `POST /api/workflows/start-manual` body (`manual.rs:18-33`). `ticket_key` may
 * be empty when Jira is off (a synthetic `MANUAL-{ts}` key is generated). With
 * `ticketing_system = none`, pass a non-empty key to drive the branch/worktree.
 */
export interface StartManualWorkflowRequest {
  ticket_key: string;
  ticket_summary: string;
  ticket_description?: string;
  issue_url?: string;
  /** A `repositories` row id the caller owns; omitted → most-recently-added. */
  repository_id?: string;
}

/** `POST /api/workflows/start-manual` response (`manual.rs:36-39`). */
export interface StartManualWorkflowResponse {
  workflow_id: string;
  ticket_key: string;
}

/**
 * One run-command's live status (`RunCommandStatus`, `dto.rs:146-158`).
 * `forwarded_port` is `[container_port, proxy_url]` once a listening port is
 * detected and forwarded (e.g. `[5173, "/s/<token>/"]`).
 */
export interface RunCommandStatus {
  index: number;
  name: string;
  running: boolean;
  forwarded_port: [number, string] | null;
}

/** `POST /api/workflows/{id}/run-commands/{index}/start` response (`run_commands.rs:35-39`). */
export interface StartRunCommandResponse {
  index: number;
  name: string;
}

/** `POST /api/workflows/{id}/open-editor` response (`editor.rs:29-42`). */
export interface OpenEditorResponse {
  /** `/s/<path_token>/?tkn=<connection_token>&folder=<…>` (proxied). */
  url: string;
  connection_token: string;
  vscode_port: number;
  port_mappings: [number, number][];
  path_token: string;
}

/** `POST /api/workflows/{id}/open-terminal` response (`editor.rs:743-754`). */
export interface OpenTerminalResponse {
  /** `/s/<path_token>/<ttyd-token>/` (proxied). */
  url: string;
  credential: string;
  path_token: string;
}

/**
 * Subset of `WorkflowSummary` (`dto.rs:33-`) the Part-B specs poll: enough to
 * observe run-command status, the forwarded ports, and the editor/terminal URLs.
 */
export interface WorkflowSummary {
  id: string;
  ticket_key: string;
  state: string;
  error?: string | null;
  workspace_name: string;
  branch_name: string;
  can_open_editor: boolean;
  editor_url: string | null;
  terminal_url: string | null;
  run_commands: RunCommandStatus[];
  worktree_path: string | null;
  /** Bootstrap + flow step log (`StepLog`, `step.rs:10`); used for diagnostics. */
  steps_log?: WorkflowStepLog[];
  /**
   * Per-definition run state, keyed by the flow slug → display name
   * (`idle` / `running` / `completed` / `error`). Wire field is `definition_runs`
   * (`dto.rs:106`). The terminal-state signal for a flow run.
   */
  definition_runs?: Record<string, string>;
}

/** One step row of a workflow's log (`StepLog`, `step.rs:10`). */
export interface WorkflowStepLog {
  step_name: string;
  status: string;
  error?: string | null;
  bootstrap?: boolean;
}

/**
 * One runnable workflow definition (`DiscoveredWorkflow`, `definitions.rs:74`).
 * `filename` is the flow slug passed to `run-workflow/{def}`.
 */
export interface WorkflowDefinition {
  filename: string;
  name: string;
  valid: boolean;
  error?: string;
}

/** One step of a user flow seeded via `PUT /api/me/flows` (`user_work_item_flows.rs:44`). */
export interface UserFlowStepInput {
  name: string;
  prompt: string;
  skills?: { name: string; args?: string[] }[];
}

/** A user flow definition (`UserFlow`, `user_work_item_flows.rs:57`). */
export interface UserFlowInput {
  name: string;
  depends_on?: string[];
  steps: UserFlowStepInput[];
}

/** `GET`/`PUT /api/me/flows` response (`me_flows.rs:30-32`). */
export interface MyFlowsResponse {
  flows: UserFlowInput[];
  workspace: string;
}

/**
 * A `WorkflowEvent` as serialised onto `GET /ws` (`engine/types.rs:75-129`).
 * `event_type` is the discriminator; `forwarded_port` is `(container_port,
 * host_port)` on the port-forward events.
 */
export interface WorkflowEvent {
  event_type: string;
  workflow_id: string;
  ticket_key: string;
  state: string;
  error?: string | null;
  step_name?: string;
  forwarded_port?: [number, number];
}

/** Result of a `GET /s/{path_token}/…` proxied request via the authed context. */
export interface ProxyResponse {
  status: number;
  body: string;
  contentType: string;
}

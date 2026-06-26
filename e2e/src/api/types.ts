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

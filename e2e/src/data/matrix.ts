import { BACKENDS, type Backend } from '../docker/naming.js';
import type {
  AgentProviderConfig,
  JiraCredentialRequest,
  ProviderId,
  TicketingId,
} from '../api/types.js';

/** Every AI provider the wizard offers (`ProviderStep.tsx:7`). */
export const ALL_PROVIDERS: readonly ProviderId[] = ['claude', 'cursor', 'codex', 'opencode'];

/** Every ticketing system the wizard offers (`TicketingStep.tsx:8`). */
export const ALL_TICKETING: readonly TicketingId[] = ['none', 'github', 'jira'];

export { BACKENDS } from '../docker/naming.js';
export type { Backend } from '../docker/naming.js';

/** Filter a fixed set against a `TAKUTO_E2E_*` comma-separated override. */
function fromEnv<T extends string>(all: readonly T[], raw: string | undefined): T[] {
  if (!raw) {
    return [...all];
  }
  const requested = raw.split(',').map((s) => s.trim().toLowerCase());
  return all.filter((v) => requested.includes(v));
}

/** Providers under test — scope down with `TAKUTO_E2E_PROVIDERS=claude,cursor`. */
export function selectedProviders(): ProviderId[] {
  return fromEnv(ALL_PROVIDERS, process.env.TAKUTO_E2E_PROVIDERS);
}

/** Ticketing systems under test — scope down with `TAKUTO_E2E_TICKETING=none,jira`. */
export function selectedTicketing(): TicketingId[] {
  return fromEnv(ALL_TICKETING, process.env.TAKUTO_E2E_TICKETING);
}

/** Backends under test — scope down with `TAKUTO_E2E_BACKENDS=sqlite,postgres`. */
export function selectedBackends(): Backend[] {
  return fromEnv(BACKENDS, process.env.TAKUTO_E2E_BACKENDS);
}

/** Git settings the wizard step 1 enters — distinct from the seeds to prove a real save. */
export const GIT_INPUT = {
  baseBranch: 'develop',
  remote: 'origin',
} as const;

/** Step-5 timeout the wizard enters — distinct from the 1800 seed. */
export const STEP_TIMEOUT_SECS = 900;

/** Values entered on the provider step for one provider, plus the dummy AI key. */
export interface ProviderInput {
  /** Typed into `#onb-base-url`. Empty = vendor default / disabled (cursor). */
  baseUrl: string;
  /** Typed into `#onb-model`. */
  model: string;
  /** One per line in `#onb-extra-args`. */
  extraArgs: string[];
  /** Dummy provider key pasted into the AI key panel (persistence proof only). */
  apiKey: string;
}

/**
 * Per-provider wizard inputs. They encode the contract's provider rules:
 *  - `cursor` leaves base URL empty (the field is disabled and forced empty).
 *  - `opencode` MUST supply base URL + model (server-side required, §3 step 3).
 *  - `claude` / `codex` leave base URL empty (vendor public API) so the saved
 *    config carries no `base_url` for them.
 */
export const PROVIDER_INPUTS: Record<ProviderId, ProviderInput> = {
  claude: {
    baseUrl: '',
    model: 'claude-sonnet-4-6',
    extraArgs: ['--max-turns', '50'],
    apiKey: 'e2e-dummy-claude-key',
  },
  cursor: {
    baseUrl: '',
    model: 'auto',
    extraArgs: ['--max-turns', '50'],
    apiKey: 'e2e-dummy-cursor-key',
  },
  codex: {
    baseUrl: '',
    model: 'gpt-5-codex',
    extraArgs: ['--max-turns', '50'],
    apiKey: 'e2e-dummy-codex-key',
  },
  opencode: {
    baseUrl: 'http://opencode.local:1234/v1',
    model: 'lmstudio/qwen3-coder',
    extraArgs: ['--max-turns', '50'],
    apiKey: 'e2e-dummy-opencode-bearer',
  },
};

/**
 * Expected `agent.providers.<provider>` sub-table in `GET /api/config` after the
 * wizard saves {@link PROVIDER_INPUTS} for `provider`. Intended for a partial
 * (`toMatchObject`) assertion — the wire carries extra keys (`allow_shared_default`,
 * `version`, opencode limits) that are not modelled here.
 *
 * Wire reality, confirmed in `crates/takuto-core/src/config/agent.rs`:
 *  - `base_url` / `model` / `extra_args` are non-optional with `#[serde(default)]`,
 *    so they are ALWAYS present — an empty base URL serializes as `""`, not omitted.
 *  - `CursorProviderConfig` has NO `base_url` field at all, so the cursor sub-table
 *    never carries that key (the contract's "cursor: no base_url").
 */
export function expectedProviderConfig(provider: ProviderId): AgentProviderConfig {
  const input = PROVIDER_INPUTS[provider];
  const expected: AgentProviderConfig = {
    model: input.model,
    extra_args: input.extraArgs,
  };
  if (provider !== 'cursor') {
    expected.base_url = input.baseUrl;
  }
  return expected;
}

/**
 * Expected values the written `config.toml` carries after completion — the five
 * keys the contract asserts (§6/§7): provider, ticketing system, git base branch
 * + remote, and the step timeout. Read the file with `readConfigTomlViaExec`.
 */
export interface ExpectedConfigToml {
  provider: ProviderId;
  ticketingSystem: TicketingId;
  baseBranch: string;
  remote: string;
  stepTimeoutSecs: number;
}

/** Build the expected `config.toml` key set for an onboarding case. */
export function expectedConfigToml(c: OnboardingCase): ExpectedConfigToml {
  return {
    provider: c.provider,
    ticketingSystem: c.ticketing,
    baseBranch: GIT_INPUT.baseBranch,
    remote: GIT_INPUT.remote,
    stepTimeoutSecs: STEP_TIMEOUT_SECS,
  };
}

/** Jira credential the ticketing step enters when the system is `jira`. */
export const JIRA_INPUT: JiraCredentialRequest = {
  site: 'https://e2e-takuto.atlassian.net',
  email: 'e2e-admin@example.com',
  token: 'e2e-dummy-jira-token',
};

/** One happy-path / persistence combination: a provider crossed with a ticketing system. */
export interface OnboardingCase {
  provider: ProviderId;
  ticketing: TicketingId;
}

/** Human-readable, filesystem-safe label for a case (used in test titles). */
export function caseLabel(c: OnboardingCase): string {
  return `${c.provider}+${c.ticketing}`;
}

/** The selected provider × ticketing cartesian product for the happy/persistence specs. */
export function onboardingCases(): OnboardingCase[] {
  const cases: OnboardingCase[] = [];
  for (const provider of selectedProviders()) {
    for (const ticketing of selectedTicketing()) {
      cases.push({ provider, ticketing });
    }
  }
  return cases;
}

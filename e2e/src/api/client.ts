import { request, type APIRequestContext } from '@playwright/test';
import type {
  AdminCredentials,
  AuthStatus,
  ConfigView,
  JiraCredentialRequest,
  OnboardingStatus,
  ProviderCredentialRequest,
  ProviderId,
  RegisterResponse,
  UserCredentialsStatus,
} from './types.js';
import { parseToml, type TomlTable } from './toml.js';

/**
 * Anything that can run a command inside the Takuto container. The harness
 * `TakutoStack` satisfies this (its `exec(command)` shells into the app
 * container), so `readConfigTomlViaExec` takes the stack directly rather than
 * the lower-level docker CLI — the container name stays an internal harness
 * detail.
 */
export interface ContainerExec {
  exec(command: string[]): Promise<{ stdout: string; stderr: string; exitCode: number }>;
}

/** Path the container CMD writes `config.toml` to (§6, `Dockerfile:444-445`). */
export const CONFIG_TOML_PATH = '/etc/takuto/config.toml';

/** Thrown when a Takuto API call returns an unexpected status. */
export class ApiError extends Error {
  readonly status: number;
  readonly body: string;

  constructor(message: string, status: number, body: string) {
    super(`${message} (status ${status}): ${body}`);
    this.name = 'ApiError';
    this.status = status;
    this.body = body;
  }
}

/**
 * Typed client over the Takuto REST surface the onboarding suite touches.
 *
 * It wraps an `APIRequestContext`. Pass `page.request` so the session cookie set
 * by {@link login} lands in the *browser* context's cookie jar — then
 * `page.goto('/onboarding')` is already authenticated, and read-back GETs in the
 * same test reuse the same session. For a browserless context (e.g. a
 * restart-persistence re-login) use {@link OnboardingApi.create}.
 */
export class OnboardingApi {
  private readonly api: APIRequestContext;
  private readonly baseURL: string;
  private readonly ownsContext: boolean;

  constructor(api: APIRequestContext, baseURL: string, ownsContext = false) {
    this.api = api;
    this.baseURL = baseURL.replace(/\/$/, '');
    this.ownsContext = ownsContext;
  }

  /** Build a client backed by a fresh, browserless request context. */
  static async create(baseURL: string): Promise<OnboardingApi> {
    const ctx = await request.newContext({ baseURL });
    return new OnboardingApi(ctx, baseURL, true);
  }

  private url(path: string): string {
    return `${this.baseURL}${path}`;
  }

  /**
   * Headers for mutating (POST/PUT/DELETE) requests. The server's CSRF
   * middleware rejects any state-changing request whose `Origin` is not in the
   * CORS allowlist; a browser sets `Origin` automatically, but Playwright's
   * `APIRequestContext` sends none, so set it explicitly to this client's own
   * origin (which the stack seeds into the server's allowlist).
   */
  private mutatingHeaders(): Record<string, string> {
    return { Origin: this.baseURL };
  }

  /** `GET /api/auth/status` — public; reports whether first-admin setup is due. */
  async getAuthStatus(): Promise<AuthStatus> {
    const res = await this.api.get(this.url('/api/auth/status'));
    if (!res.ok()) {
      throw new ApiError('GET /api/auth/status failed', res.status(), await res.text());
    }
    return (await res.json()) as AuthStatus;
  }

  /** `POST /api/auth/register` — creates the first admin. Does NOT set a cookie. */
  async registerAdmin(creds: AdminCredentials): Promise<RegisterResponse> {
    const res = await this.api.post(this.url('/api/auth/register'), {
      data: creds,
      headers: this.mutatingHeaders(),
    });
    if (res.status() !== 201) {
      throw new ApiError('POST /api/auth/register failed', res.status(), await res.text());
    }
    return (await res.json()) as RegisterResponse;
  }

  /**
   * `POST /api/auth/login` — `204 No Content` + `Set-Cookie: takuto_session`.
   * The cookie is stored in this client's request context (and, when that
   * context is `page.request`, in the page's browser context too).
   */
  async login(creds: AdminCredentials): Promise<void> {
    // The login endpoint is rate-limited (429). Each spec logs in afresh per
    // test (a new browser context each time), so a fast matrix can trip it;
    // retry on 429 with a short backoff — the server asks for ~1s.
    const maxAttempts = 6;
    for (let attempt = 1; ; attempt += 1) {
      const res = await this.api.post(this.url('/api/auth/login'), {
        data: creds,
        headers: this.mutatingHeaders(),
      });
      if (res.status() === 204) {
        return;
      }
      if (res.status() === 429 && attempt < maxAttempts) {
        await new Promise((resolve) => setTimeout(resolve, 1500));
        continue;
      }
      throw new ApiError('POST /api/auth/login failed', res.status(), await res.text());
    }
  }

  /**
   * Idempotent first-admin bootstrap: register when setup is required, then log
   * in so the session cookie is live. Returns the recovery codes from
   * registration, or an empty array when the admin already existed.
   */
  async bootstrapAdmin(creds: AdminCredentials): Promise<string[]> {
    const status = await this.getAuthStatus();
    let recoveryCodes: string[] = [];
    if (status.setup_required) {
      recoveryCodes = (await this.registerAdmin(creds)).recovery_codes;
    }
    await this.login(creds);
    return recoveryCodes;
  }

  /** `GET /api/onboarding/status` — public; carries `user_onboarding` when authed. */
  async getOnboardingStatus(): Promise<OnboardingStatus> {
    const res = await this.api.get(this.url('/api/onboarding/status'));
    if (!res.ok()) {
      throw new ApiError('GET /api/onboarding/status failed', res.status(), await res.text());
    }
    return (await res.json()) as OnboardingStatus;
  }

  /** True once the wizard has been finished (`completed_at` non-null). */
  async isOnboardingComplete(): Promise<boolean> {
    const status = await this.getOnboardingStatus();
    return status.user_onboarding?.completed_at != null;
  }

  /** `GET /api/config` — flattened config (auth required). The read-back source. */
  async getConfig(): Promise<ConfigView> {
    const res = await this.api.get(this.url('/api/config'));
    if (!res.ok()) {
      throw new ApiError('GET /api/config failed', res.status(), await res.text());
    }
    return (await res.json()) as ConfigView;
  }

  /**
   * `GET /api/users/me/credentials[?provider=<p>]` — per-user credential status
   * (no secrets). Used by restart-persistence specs to prove encrypted rows
   * survive a reboot.
   */
  async getUserCredentials(provider?: ProviderId): Promise<UserCredentialsStatus> {
    const path = provider
      ? `/api/users/me/credentials?provider=${encodeURIComponent(provider)}`
      : '/api/users/me/credentials';
    const res = await this.api.get(this.url(path));
    if (!res.ok()) {
      throw new ApiError('GET /api/users/me/credentials failed', res.status(), await res.text());
    }
    return (await res.json()) as UserCredentialsStatus;
  }

  /**
   * `POST /api/users/me/credentials/{provider}` — store an AI-provider api_key.
   * The server shape-validates the key (no live provider round-trip), so a dummy
   * value seals + persists, giving the restart-persistence spec an encrypted row
   * to prove survives a reboot.
   */
  async setProviderCredential(provider: ProviderId, body: ProviderCredentialRequest): Promise<void> {
    const res = await this.api.post(this.url(`/api/users/me/credentials/${encodeURIComponent(provider)}`), {
      data: body,
      headers: this.mutatingHeaders(),
    });
    if (!res.ok()) {
      throw new ApiError(`POST /api/users/me/credentials/${provider} failed`, res.status(), await res.text());
    }
  }

  /** `POST /api/users/me/jira-credential` — store a Jira credential for the user. */
  async setJiraCredential(body: JiraCredentialRequest): Promise<void> {
    const res = await this.api.post(this.url('/api/users/me/jira-credential'), {
      data: body,
      headers: this.mutatingHeaders(),
    });
    if (!res.ok()) {
      throw new ApiError('POST /api/users/me/jira-credential failed', res.status(), await res.text());
    }
  }

  /** Dispose the request context if this client created it. No-op for `page.request`. */
  async dispose(): Promise<void> {
    if (this.ownsContext) {
      await this.api.dispose();
    }
  }
}

/**
 * Read and parse the `config.toml` the server wrote inside the container, via
 * `docker exec … cat`. `target` is the harness stack (anything with `exec`).
 */
export async function readConfigTomlViaExec(
  target: ContainerExec,
  path: string = CONFIG_TOML_PATH,
): Promise<TomlTable> {
  const result = await target.exec(['cat', path]);
  if (result.exitCode !== 0) {
    throw new Error(`reading ${path} via exec failed (exit ${result.exitCode}): ${result.stderr}`);
  }
  return parseToml(result.stdout);
}

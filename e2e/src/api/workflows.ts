import { request, type APIRequestContext } from '@playwright/test';
import { ApiError } from './client.js';
import type {
  AddRepositoryRequest,
  MyFlowsResponse,
  OpenEditorResponse,
  OpenTerminalResponse,
  ProxyResponse,
  RepositoryRow,
  RunCommandStatus,
  StartManualWorkflowRequest,
  StartManualWorkflowResponse,
  StartRunCommandResponse,
  UserFlowInput,
  WorkflowDefinition,
  WorkflowSummary,
  WorktreeCommandsRequest,
  WorktreeCommandsRow,
} from './types.js';

/**
 * Typed client over the implement-workflow REST surface (Part B,
 * `IMPLEMENT_WORKFLOW_CONTRACT.md §3-4`): repository association, manual /
 * definition workflow start, worktree commands, run-commands, IDE / terminal,
 * and the `/s/{path_token}/…` shared-port proxy.
 *
 * Mirrors {@link OnboardingApi}: wrap an `APIRequestContext`. Pass `page.request`
 * so the session cookie is shared with the browser context (the `/s/` proxy and
 * `GET /ws` both need it), or use {@link WorkflowApi.create} for a browserless
 * context.
 *
 * Workflow ids accept either the workflow id or the `ticket_key`
 * (`workflows/mod.rs:155`); the specs pass the `ticket_key`.
 */
export class WorkflowApi {
  private readonly api: APIRequestContext;
  private readonly baseURL: string;
  private readonly ownsContext: boolean;

  constructor(api: APIRequestContext, baseURL: string, ownsContext = false) {
    this.api = api;
    this.baseURL = baseURL.replace(/\/$/, '');
    this.ownsContext = ownsContext;
  }

  /** Build a client backed by a fresh, browserless request context. */
  static async create(baseURL: string): Promise<WorkflowApi> {
    const ctx = await request.newContext({ baseURL });
    return new WorkflowApi(ctx, baseURL, true);
  }

  private url(path: string): string {
    return `${this.baseURL}${path}`;
  }

  /**
   * The CSRF middleware rejects state-changing requests whose `Origin` is not in
   * the CORS allowlist. Playwright's `APIRequestContext` sends none, so set it to
   * this client's own origin (which the stack seeds into the allowlist).
   */
  private mutatingHeaders(extra: Record<string, string> = {}): Record<string, string> {
    return { Origin: this.baseURL, ...extra };
  }

  // --- Repositories --------------------------------------------------------

  /** `GET /api/repositories` — repos the caller has added. */
  async listRepositories(): Promise<RepositoryRow[]> {
    const res = await this.api.get(this.url('/api/repositories'));
    if (!res.ok()) {
      throw new ApiError('GET /api/repositories failed', res.status(), await res.text());
    }
    return (await res.json()) as RepositoryRow[];
  }

  /**
   * `POST /api/repositories` — clone-if-needed + associate. Pass
   * `{ repository_id }` to add an already-registered repo, or `{ repo_url }` to
   * clone a new one. Idempotent (200 with the existing row) when already added.
   */
  async addRepository(body: AddRepositoryRequest): Promise<RepositoryRow> {
    const res = await this.api.post(this.url('/api/repositories'), {
      data: body,
      headers: this.mutatingHeaders(),
    });
    if (!res.ok()) {
      throw new ApiError('POST /api/repositories failed', res.status(), await res.text());
    }
    return (await res.json()) as RepositoryRow;
  }

  /** Convenience: add an existing registered repo by its `repositories` row id. */
  async addExistingRepository(repositoryId: string): Promise<RepositoryRow> {
    return this.addRepository({ repository_id: repositoryId });
  }

  /**
   * `GET /api/repositories/_available` — registered repos the caller has NOT
   * added yet. The Part-B fixture's react-app row is created by startup
   * reconciliation and surfaces here until associated.
   */
  async listAvailableRepositories(): Promise<RepositoryRow[]> {
    const res = await this.api.get(this.url('/api/repositories/_available'));
    if (!res.ok()) {
      throw new ApiError('GET /api/repositories/_available failed', res.status(), await res.text());
    }
    return (await res.json()) as RepositoryRow[];
  }

  // --- Flow definitions ----------------------------------------------------

  /**
   * `GET /api/workflow-definitions` — the caller's runnable flow definitions for
   * the server's ACTIVE workspace (`git.repo_path`). Note this is scoped to the
   * active workspace, not a per-work-item repository workspace.
   */
  async getWorkflowDefinitions(): Promise<WorkflowDefinition[]> {
    const res = await this.api.get(this.url('/api/workflow-definitions'));
    if (!res.ok()) {
      throw new ApiError('GET /api/workflow-definitions failed', res.status(), await res.text());
    }
    return (await res.json()) as WorkflowDefinition[];
  }

  /**
   * `GET /api/me/flows[?workspace=]` — the caller's flow list for `workspace`
   * (defaults to the active workspace when omitted).
   */
  async getMyFlows(workspace?: string): Promise<MyFlowsResponse> {
    const path = workspace
      ? `/api/me/flows?workspace=${encodeURIComponent(workspace)}`
      : '/api/me/flows';
    const res = await this.api.get(this.url(path));
    if (!res.ok()) {
      throw new ApiError('GET /api/me/flows failed', res.status(), await res.text());
    }
    return (await res.json()) as MyFlowsResponse;
  }

  /**
   * `PUT /api/me/flows[?workspace=]` — replace the caller's flow list for
   * `workspace`. The engine resolves a work item's runnable definitions from the
   * per-(user, workspace) store, so seed under the work item's own workspace.
   */
  async putMyFlows(flows: UserFlowInput[], workspace?: string): Promise<MyFlowsResponse> {
    const path = workspace
      ? `/api/me/flows?workspace=${encodeURIComponent(workspace)}`
      : '/api/me/flows';
    const res = await this.api.put(this.url(path), {
      data: { flows },
      headers: this.mutatingHeaders(),
    });
    if (!res.ok()) {
      throw new ApiError('PUT /api/me/flows failed', res.status(), await res.text());
    }
    return (await res.json()) as MyFlowsResponse;
  }

  // --- Worktree commands ---------------------------------------------------

  /**
   * `PUT /api/worktree-commands/{workspace}` — upsert init + run commands in one
   * round-trip. `workspace` is the last path component of `git.repo_path`.
   */
  async putWorktreeCommands(
    workspace: string,
    body: WorktreeCommandsRequest,
  ): Promise<WorktreeCommandsRow> {
    const res = await this.api.put(
      this.url(`/api/worktree-commands/${encodeURIComponent(workspace)}`),
      { data: body, headers: this.mutatingHeaders() },
    );
    if (!res.ok()) {
      throw new ApiError(
        `PUT /api/worktree-commands/${workspace} failed`,
        res.status(),
        await res.text(),
      );
    }
    return (await res.json()) as WorktreeCommandsRow;
  }

  /** `GET /api/worktree-commands/{workspace}` — the caller's row, or null (404). */
  async getWorktreeCommands(workspace: string): Promise<WorktreeCommandsRow | null> {
    const res = await this.api.get(
      this.url(`/api/worktree-commands/${encodeURIComponent(workspace)}`),
    );
    if (res.status() === 404) {
      return null;
    }
    if (!res.ok()) {
      throw new ApiError(
        `GET /api/worktree-commands/${workspace} failed`,
        res.status(),
        await res.text(),
      );
    }
    return (await res.json()) as WorktreeCommandsRow;
  }

  // --- Workflow start ------------------------------------------------------

  /**
   * `POST /api/workflows/start-manual` — start a ticket workflow from the
   * dashboard (same pipeline as the poller). Equivalent to the paste-description
   * modal submit. Returns the created `workflow_id` + resolved `ticket_key`.
   */
  async startManualWorkflow(
    body: StartManualWorkflowRequest,
  ): Promise<StartManualWorkflowResponse> {
    const res = await this.api.post(this.url('/api/workflows/start-manual'), {
      data: body,
      headers: this.mutatingHeaders(),
    });
    if (!res.ok()) {
      throw new ApiError('POST /api/workflows/start-manual failed', res.status(), await res.text());
    }
    return (await res.json()) as StartManualWorkflowResponse;
  }

  /**
   * `POST /api/workflows/{id}/run-workflow/{def}` — run a flow definition on a
   * card (bootstraps the worktree + init commands on first run). `202 Accepted`,
   * no body; `409` on failure. `def` is the kebab-cased flow slug.
   */
  async runWorkflowDef(id: string, def: string): Promise<void> {
    const res = await this.api.post(
      this.url(
        `/api/workflows/${encodeURIComponent(id)}/run-workflow/${encodeURIComponent(def)}`,
      ),
      { headers: this.mutatingHeaders() },
    );
    if (res.status() !== 202) {
      throw new ApiError(
        `POST /api/workflows/${id}/run-workflow/${def} failed`,
        res.status(),
        await res.text(),
      );
    }
  }

  // --- Run-commands (dev servers) ------------------------------------------

  /** `GET /api/workflows/{id}/run-commands` — status of every configured run-command. */
  async getRunCommands(id: string): Promise<RunCommandStatus[]> {
    const res = await this.api.get(
      this.url(`/api/workflows/${encodeURIComponent(id)}/run-commands`),
    );
    if (!res.ok()) {
      throw new ApiError(`GET /api/workflows/${id}/run-commands failed`, res.status(), await res.text());
    }
    const body = (await res.json()) as { commands: RunCommandStatus[] };
    return body.commands;
  }

  /** `POST /api/workflows/{id}/run-commands/{index}/start` — start a dev server. */
  async startRunCommand(id: string, index: number): Promise<StartRunCommandResponse> {
    const res = await this.api.post(
      this.url(`/api/workflows/${encodeURIComponent(id)}/run-commands/${index}/start`),
      { headers: this.mutatingHeaders() },
    );
    if (!res.ok()) {
      throw new ApiError(
        `POST /api/workflows/${id}/run-commands/${index}/start failed`,
        res.status(),
        await res.text(),
      );
    }
    return (await res.json()) as StartRunCommandResponse;
  }

  /** `POST /api/workflows/{id}/run-commands/{index}/stop` — stop a dev server (200, no body). */
  async stopRunCommand(id: string, index: number): Promise<void> {
    const res = await this.api.post(
      this.url(`/api/workflows/${encodeURIComponent(id)}/run-commands/${index}/stop`),
      { headers: this.mutatingHeaders() },
    );
    if (!res.ok()) {
      throw new ApiError(
        `POST /api/workflows/${id}/run-commands/${index}/stop failed`,
        res.status(),
        await res.text(),
      );
    }
  }

  /**
   * `POST /api/workflows/{id}/stop` — cancel the workflow and park it in the
   * `Stopped` (terminal, non-active) state, keeping the worktree on disk. Used to
   * make the run-command endpoints (which reject an active workflow) usable after
   * a custom flow run that does not finalize the main state.
   */
  async stopWorkflow(id: string): Promise<void> {
    const res = await this.api.post(
      this.url(`/api/workflows/${encodeURIComponent(id)}/stop`),
      { headers: this.mutatingHeaders() },
    );
    if (!res.ok()) {
      throw new ApiError(`POST /api/workflows/${id}/stop failed`, res.status(), await res.text());
    }
  }

  // --- IDE / terminal ------------------------------------------------------

  /** `POST /api/workflows/{id}/open-editor` — start a browser VS Code container. */
  async openEditor(id: string): Promise<OpenEditorResponse> {
    const res = await this.api.post(
      this.url(`/api/workflows/${encodeURIComponent(id)}/open-editor`),
      { headers: this.mutatingHeaders() },
    );
    if (!res.ok()) {
      throw new ApiError(`POST /api/workflows/${id}/open-editor failed`, res.status(), await res.text());
    }
    return (await res.json()) as OpenEditorResponse;
  }

  /** `POST /api/workflows/{id}/open-terminal` — start a browser ttyd terminal. */
  async openTerminal(id: string): Promise<OpenTerminalResponse> {
    const res = await this.api.post(
      this.url(`/api/workflows/${encodeURIComponent(id)}/open-terminal`),
      { headers: this.mutatingHeaders() },
    );
    if (!res.ok()) {
      throw new ApiError(`POST /api/workflows/${id}/open-terminal failed`, res.status(), await res.text());
    }
    return (await res.json()) as OpenTerminalResponse;
  }

  // --- Workflow read -------------------------------------------------------

  /** `GET /api/workflows/{id}` — the full work-item summary (subset typed). */
  async getWorkflow(id: string): Promise<WorkflowSummary> {
    const res = await this.api.get(this.url(`/api/workflows/${encodeURIComponent(id)}`));
    if (!res.ok()) {
      throw new ApiError(`GET /api/workflows/${id} failed`, res.status(), await res.text());
    }
    return (await res.json()) as WorkflowSummary;
  }

  /** `GET /api/workflows` — every work item visible to the caller. */
  async listWorkflows(): Promise<WorkflowSummary[]> {
    const res = await this.api.get(this.url('/api/workflows'));
    if (!res.ok()) {
      throw new ApiError('GET /api/workflows failed', res.status(), await res.text());
    }
    return (await res.json()) as WorkflowSummary[];
  }

  // --- `/s/{path_token}/…` shared-port proxy -------------------------------

  /**
   * `GET` a proxied `/s/{path_token}/…` URL using THIS client's authenticated
   * context — the proxy requires a valid `takuto_session` cookie before the
   * token lookup (`sessions/mod.rs:187-202`). Accepts either an absolute URL or
   * a path (resolved against `baseURL`); the `url` fields from open-editor /
   * open-terminal / forwarded run-command ports are paths.
   */
  async fetchProxied(urlOrPath: string): Promise<ProxyResponse> {
    const target = urlOrPath.startsWith('http') ? urlOrPath : this.url(urlOrPath);
    const res = await this.api.get(target);
    return {
      status: res.status(),
      body: await res.text(),
      contentType: res.headers()['content-type'] ?? '',
    };
  }

  /** Dispose the request context if this client created it. No-op for `page.request`. */
  async dispose(): Promise<void> {
    if (this.ownsContext) {
      await this.api.dispose();
    }
  }
}

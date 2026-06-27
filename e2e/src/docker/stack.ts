import { createServer } from 'node:net';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import * as docker from './cli.js';
import {
  type Backend,
  newStackId,
  resourceName,
  stackLabels,
  LABEL_CACHE,
  LABEL_STACK,
} from './naming.js';
import { ensureImage } from './image.js';
import {
  MOCK_PORT,
  dindDocker,
  loadWorkerImageIntoDind,
  seedFixtureRepo,
  startDindMock,
} from './dind.js';

/**
 * A live, ephemeral Takuto deployment a test can talk to. One instance per
 * Playwright worker. Page Objects and specs depend on this interface — import
 * it from `src/fixtures/stack.fixture.ts`.
 */
export interface TakutoStack {
  /** Database backend this stack runs against. */
  readonly backend: Backend;
  /** Base URL of the running Takuto server, e.g. `http://127.0.0.1:54321`. */
  readonly baseURL: string;
  /**
   * Restart the Takuto container in place — same data volume, same database,
   * same master key — and wait until it is healthy again. The strongest proof
   * that persisted settings and encrypted credentials survive a reboot. The
   * `baseURL` is unchanged across a restart.
   */
  restart(): Promise<void>;
  /** Run a command inside the Takuto container (e.g. read `config.toml`). */
  exec(command: string[]): Promise<{ stdout: string; stderr: string; exitCode: number }>;
  /**
   * Resolve once the runtime agent/CLI install has finished (`phase == ready`).
   * No-op (resolves immediately) for stacks created without `installAgents`,
   * which never start an install. Idempotent — safe to await repeatedly.
   */
  waitAgentsInstalled(): Promise<void>;
  /**
   * Part-B (DinD) facts a workflow spec needs, or `null` for a non-DinD stack:
   * the DinD container name, the in-DinD mock LM Studio IP/port, the registered
   * fixture repo's in-container path + workspace name, and a helper to run a
   * `docker` subcommand against the DinD daemon (e.g. read the mock's logs).
   */
  readonly dind: DindInfo | null;
}

/** Part-B handles exposed to workflow specs. */
export interface DindInfo {
  readonly containerName: string;
  /** Image tag loaded into the DinD daemon (workers + mock run from this). */
  readonly workerImage: string;
  /** Mock LM Studio container name inside the DinD daemon (read its logs for
   * `MOCK_HIT …` lines via `exec(['logs', mockName])` to assert worker calls). */
  readonly mockName: string;
  readonly mockIp: string;
  readonly mockPort: number;
  /** OpenAI `/v1` root opencode is pointed at (`http://<mockIp>:<port>/v1`). */
  readonly mockBaseUrl: string;
  /** In-container path of the seeded fixture repo (`/workspaces/<name>`). */
  readonly repoPath: string;
  /** Last path component of `repoPath` — scopes worktree-commands + workflows. */
  readonly workspaceName: string;
  /** Slug of the seeded flow for `POST /api/workflows/{id}/run-workflow/{flowSlug}`. */
  readonly flowSlug: string;
  /** Run a `docker …` subcommand inside the DinD daemon. */
  exec(args: string[]): Promise<{ stdout: string; exitCode: number }>;
}

/** Per-stack knobs. */
export interface StackOptions {
  /**
   * Install the four agent CLIs (claude/cursor/codex/opencode) + acli at boot
   * into a persistent, shared tools-cache volume, so tests can exec the
   * binaries. When false (default), the tools dir is pointed at a non-existent
   * path so the server SKIPS the install (onboarding suites don't need it).
   */
  installAgents?: boolean;
  /**
   * Part B: bring up a Docker-in-Docker sidecar, load the worker image into it,
   * run a mock LM Studio inside it, seed + register a local React fixture repo,
   * and configure the active provider as `opencode` pointed at the mock. Implies
   * {@link installAgents} (workers need the opencode CLI). Single backend only.
   */
  dind?: boolean;
}

/**
 * Pinned deployment master key (64 hex chars = 32 bytes, the format
 * `TAKUTO_SECRET_KEY` requires). Pinning it lets encrypted credential rows
 * decrypt after a container restart. Ephemeral test value only — overridable
 * via `TAKUTO_E2E_SECRET_KEY`; never a real deployment secret.
 */
const SECRET_KEY =
  process.env.TAKUTO_E2E_SECRET_KEY ||
  '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';

const DATA_DIR = '/home/takuto/.takuto';
const APP_PORT = 8080;
const DB_ALIAS = 'db';

/**
 * Real default tools dir `agent_install` writes into (`<dir>/bin` lands on the
 * worker/workspace PATH). Setting `TAKUTO_TOOLS_DIR` to a path that EXISTS is
 * what makes the server run the boot-time agent install (it skips when the dir
 * is absent). The Dockerfile creates `/opt/takuto-tools/bin` owned by `takuto`.
 */
const TOOLS_DIR = '/opt/takuto-tools';

const AGENT_INSTALL_TIMEOUT_MS = Number(
  process.env.TAKUTO_E2E_AGENT_INSTALL_TIMEOUT_MS ?? 300_000,
);

/**
 * Pinned Cursor Agent version. The server's "latest" resolver parses the
 * Cursor version pin for the e2e stack. **Empty by default** so the suite
 * exercises Takuto's real UNPINNED install path — which defers to Cursor's
 * official installer (`curl https://cursor.com/install | bash`). Leaving it
 * unpinned is what end-to-end verifies the F1 fix (the four startup binaries all
 * install via their default/latest path). Set `TAKUTO_E2E_CURSOR_VERSION` to pin
 * a specific build if ever needed.
 */
const CURSOR_VERSION = process.env.TAKUTO_E2E_CURSOR_VERSION ?? '';

/** In-container path the server reads/writes `config.toml` (Dockerfile CMD). */
const CONFIG_TOML_PATH = '/etc/takuto/config.toml';
/** Parent dir of `config.toml`; bind-mounted (not the bare file) in DinD mode. */
const CONFIG_DIR = '/etc/takuto';

// ── Part-B (DinD) constants — mirror docker-compose.dind.yml ──────────────
/** Runtime uid the image runs as (`Dockerfile` `TAKUTO_UID`); owns the fixture. */
const TAKUTO_UID = 999;
/** DinD-side prefix the takuto-data volume mounts at (path translation target). */
const DIND_DATA_PREFIX = '/shared-auth/takuto-data';
/** Workspaces dir startup reconciliation scans (`snapshot::WORKSPACES_DIR`). */
const WORKSPACES_DIR = '/workspaces';
/** Fixture repo directory name under `WORKSPACES_DIR`. */
const FIXTURE_REPO_NAME = 'react-app';
/** opencode model id seeded for the mock (any non-empty value satisfies it). */
const MOCK_MODEL = 'mock-model';
/**
 * Slug of the seeded flow definition (`{def}` in
 * `POST /api/workflows/{id}/run-workflow/{def}`). The server slugifies the flow
 * `name`; "Implement" → "implement". The flow has a single opencode agent step,
 * so a manual run exercises the mock with no push/PR/GitHub step.
 */
const FLOW_SLUG = 'implement';
const FLOW_DEFINITION_TOML = `name = "Implement"

[[steps]]
name = "Implement with opencode"
prompt = "Make a small, safe change to the project and briefly describe what you did."
`;
/** Host path of the committed React fixture (`e2e/fixtures/react-app`). */
const FIXTURE_HOST_PATH = resolve(
  dirname(fileURLToPath(import.meta.url)),
  '../../fixtures/react-app',
);

const DIND_TIMEOUT_MS = Number(process.env.TAKUTO_E2E_DIND_TIMEOUT_MS ?? 120_000);

const HEALTH_TIMEOUT_MS = Number(process.env.TAKUTO_E2E_HEALTH_TIMEOUT_MS ?? 180_000);
const DB_TIMEOUT_MS = Number(process.env.TAKUTO_E2E_DB_TIMEOUT_MS ?? 90_000);

/** Random alphanumeric token safe for use unencoded in a connection URL. */
function urlSafeToken(len: number): string {
  const alphabet = 'abcdefghijklmnopqrstuvwxyz0123456789';
  let out = '';
  for (let i = 0; i < len; i += 1) {
    out += alphabet[Math.floor(Math.random() * alphabet.length)];
  }
  return out;
}

async function sleep(ms: number): Promise<void> {
  await new Promise((r) => setTimeout(r, ms));
}

/**
 * Reserve a free TCP port on the loopback interface and return it. The host
 * port is bound EXPLICITLY (not Docker's ephemeral `::8080`) so a container
 * stop→start re-publishes the same port — Docker Desktop re-allocates an
 * ephemeral host port on restart, which would orphan the original `baseURL`.
 */
function findFreePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (address === null || typeof address === 'string') {
        server.close(() => reject(new Error('could not determine a free port')));
        return;
      }
      const { port } = address;
      server.close(() => resolve(port));
    });
  });
}

async function waitFor(
  label: string,
  check: () => Promise<boolean>,
  timeoutMs: number,
  intervalMs = 1000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let lastError: unknown;
  for (;;) {
    try {
      if (await check()) {
        return;
      }
    } catch (err) {
      lastError = err;
    }
    if (Date.now() >= deadline) {
      throw new Error(`timed out waiting for ${label} after ${timeoutMs}ms${lastError ? `: ${String(lastError)}` : ''}`);
    }
    await sleep(intervalMs);
  }
}

interface DbSpec {
  image: string;
  env: Record<string, string>;
  /** Connection URL takuto uses, addressing the db by its network alias. */
  connection: string;
  /** Command proving the server is accepting connections. */
  healthCmd: string[];
}

/** Build the DB container spec for a backend (null for sqlite). */
function dbSpecFor(backend: Backend): DbSpec | null {
  if (backend === 'sqlite') {
    return null;
  }
  const user = `u${urlSafeToken(8)}`;
  const password = urlSafeToken(24);
  const name = `d${urlSafeToken(8)}`;
  if (backend === 'postgres') {
    return {
      image: 'postgres:16',
      env: { POSTGRES_USER: user, POSTGRES_PASSWORD: password, POSTGRES_DB: name },
      connection: `postgres://${user}:${password}@${DB_ALIAS}:5432/${name}`,
      healthCmd: ['pg_isready', '-U', user, '-d', name],
    };
  }
  return {
    image: 'mariadb:11',
    env: {
      MARIADB_USER: user,
      MARIADB_PASSWORD: password,
      MARIADB_DATABASE: name,
      MARIADB_ROOT_PASSWORD: urlSafeToken(24),
    },
    connection: `mysql://${user}:${password}@${DB_ALIAS}:3306/${name}`,
    healthCmd: ['healthcheck.sh', '--connect', '--innodb_initialized'],
  };
}

class DockerStack implements TakutoStack {
  readonly backend: Backend;
  baseURL = '';

  private readonly stackId: string;
  private readonly labels: Record<string, string>;
  private readonly netName: string;
  private readonly appName: string;
  private readonly dataVolume: string;
  private readonly dbVolume: string;
  private readonly installAgents: boolean;
  /**
   * Persistent, shared tools-cache volume. Its name is stable per-backend (NOT
   * per-stack), so npm/cursor/acli downloads cache across runs. It is labelled
   * with `LABEL_CACHE` ONLY — deliberately NOT `LABEL_E2E` nor the run/stack
   * labels — so neither per-stack {@link teardown} (`LABEL_STACK=…`) nor the
   * global per-run sweep (`LABEL_RUN=…`) deletes it, AND it never registers as
   * a leftover in the zero-residual checks that filter on `LABEL_E2E=true`.
   * Purge manually with `docker volume ls --filter label=com.takuto.e2e.cache`.
   * The per-backend name also avoids two backend projects writing the same
   * volume concurrently (within a backend the suite runs serially).
   */
  private readonly toolsVolume: string;
  private readonly db: DbSpec | null;
  private dbName = '';
  private image = '';
  private configTmpDir = '';

  // ── Part-B (DinD) state ──────────────────────────────────────────────────
  private readonly useDind: boolean;
  private readonly dindName: string;
  private readonly workspaceVolume: string;
  private readonly workspacesVolume: string;
  private readonly mockName: string;
  private mockIp = '';
  dind: DindInfo | null = null;

  constructor(backend: Backend, options: StackOptions = {}) {
    this.backend = backend;
    this.stackId = newStackId(backend);
    this.labels = stackLabels(this.stackId);
    this.netName = resourceName(this.stackId, 'net');
    this.appName = resourceName(this.stackId, 'app');
    this.dataVolume = resourceName(this.stackId, 'data');
    this.dbVolume = resourceName(this.stackId, 'dbdata');
    this.useDind = options.dind ?? false;
    // DinD workers need the opencode CLI from the tools volume, so DinD implies
    // the agent install.
    this.installAgents = (options.installAgents ?? false) || this.useDind;
    this.toolsVolume = `takuto-e2e-tools-${backend}`;
    this.dindName = resourceName(this.stackId, 'dind');
    this.workspaceVolume = resourceName(this.stackId, 'ws');
    this.workspacesVolume = resourceName(this.stackId, 'wss');
    this.mockName = `takuto-e2e-mock-${this.stackId}`;
    this.db = dbSpecFor(backend);
    if (this.db) {
      this.dbName = resourceName(this.stackId, 'db');
    }
  }

  async up(log: (msg: string) => void = () => {}): Promise<void> {
    this.image = await ensureImage(log);
    await docker.network.create(this.netName, this.labels);
    await docker.volume.create(this.dataVolume, this.labels);

    if (this.installAgents) {
      // Shared cache: LABEL_CACHE only (no e2e/run/stack labels) so it survives
      // both sweeps and the zero-residual checks. `docker volume create` is
      // idempotent for an existing same-labelled name, so reuse is fine.
      await docker.volume.create(this.toolsVolume, { [LABEL_CACHE]: 'true' });
    }

    if (this.db) {
      await docker.volume.create(this.dbVolume, this.labels);
      await this.startDb(log);
    }

    if (this.useDind) {
      await this.startDindSidecar(log);
    }

    await this.startApp(log);
    await this.waitHealthy();
    if (this.installAgents) {
      log(`Waiting for agent CLI install in ${this.stackId}…`);
      await this.waitAgentsInstalled();
      log(`Agent CLIs installed for ${this.stackId}`);
    }
    log(`Stack ${this.stackId} ready at ${this.baseURL}`);
  }

  /**
   * Bring up the DinD sidecar + Part-B infra, in order: create the shared
   * workspace volumes, seed the fixture repo, start `docker:dind` (privileged,
   * on the stack net), wait for `docker info`, load the worker image into it,
   * and start the mock LM Studio inside it (capturing its bridge IP). The
   * takuto container is started later by {@link startApp} sharing this DinD
   * container's network namespace.
   */
  private async startDindSidecar(log: (msg: string) => void): Promise<void> {
    await docker.volume.create(this.workspaceVolume, this.labels);
    await docker.volume.create(this.workspacesVolume, this.labels);

    const repoPath = await seedFixtureRepo({
      image: this.image,
      workspacesVolume: this.workspacesVolume,
      fixtureHostPath: FIXTURE_HOST_PATH,
      repoName: FIXTURE_REPO_NAME,
      takutoUid: TAKUTO_UID,
      labels: this.labels,
      log,
    });

    // The host config dir is bind-mounted into BOTH dind and takuto at
    // /etc/takuto so worker egress rules read the same config.toml (with the
    // mock IP). config.toml itself is written later (after the mock IP is known)
    // in writeSeedConfig — bind mounts reflect that host write live.
    this.configTmpDir = await mkdtemp(join(tmpdir(), `takuto-e2e-${this.stackId}-`));
    // Seed a flow definition into the default workflow_definitions_dir
    // (`<config dir>/workflows` → /etc/takuto/workflows) so a manual work item
    // has a `{def}` to run. The server discovers it; no built-in flows exist.
    const flowsDir = join(this.configTmpDir, 'workflows');
    await mkdir(flowsDir, { recursive: true });
    await writeFile(join(flowsDir, `${FLOW_SLUG}.toml`), FLOW_DEFINITION_TOML, { mode: 0o666 });

    log(`Starting DinD sidecar ${this.dindName}…`);
    await docker.run({
      image: 'docker:27-dind',
      name: this.dindName,
      labels: this.labels,
      env: { DOCKER_TLS_CERTDIR: '' },
      network: this.netName,
      networkAlias: 'dind',
      extraArgs: ['--privileged'],
      // App port is published HERE: takuto shares this netns (no own interfaces).
      publish: [{ container: APP_PORT, host: await this.appHostPort(), bindIp: '127.0.0.1' }],
      volumes: {
        [this.workspaceVolume]: '/workspace',
        [this.workspacesVolume]: WORKSPACES_DIR,
        [this.dataVolume]: DIND_DATA_PREFIX,
        [this.toolsVolume]: TOOLS_DIR,
        [this.configTmpDir]: CONFIG_DIR,
      },
    });
    await waitFor(
      `DinD daemon ${this.dindName} ready`,
      async () => (await dindDocker(this.dindName, ['info'])).exitCode === 0,
      DIND_TIMEOUT_MS,
    );
    log(`DinD ${this.dindName} healthy`);

    await loadWorkerImageIntoDind(this.dindName, this.image, log);
    this.mockIp = await startDindMock(this.dindName, this.mockName, this.image, log);

    this.dind = {
      containerName: this.dindName,
      workerImage: this.image,
      mockName: this.mockName,
      mockIp: this.mockIp,
      mockPort: MOCK_PORT,
      mockBaseUrl: `http://${this.mockIp}:${MOCK_PORT}/v1`,
      repoPath,
      workspaceName: FIXTURE_REPO_NAME,
      flowSlug: FLOW_SLUG,
      exec: (args: string[]) => dindDocker(this.dindName, args),
    };
  }

  /**
   * Reserve (once) the host port the app is published on. In DinD mode this is
   * published on the DinD container; otherwise on the takuto container.
   */
  private async appHostPort(): Promise<number> {
    if (this.reservedHostPort === 0) {
      this.reservedHostPort = await findFreePort();
      this.baseURL = `http://127.0.0.1:${this.reservedHostPort}`;
    }
    return this.reservedHostPort;
  }
  private reservedHostPort = 0;

  private async startDb(log: (msg: string) => void): Promise<void> {
    const db = this.db;
    if (!db) {
      return;
    }
    const mount = this.backend === 'postgres' ? '/var/lib/postgresql/data' : '/var/lib/mysql';
    log(`Starting ${db.image} for ${this.stackId}…`);
    await docker.run({
      image: db.image,
      name: this.dbName,
      labels: this.labels,
      env: db.env,
      network: this.netName,
      networkAlias: DB_ALIAS,
      volumes: { [this.dbVolume]: mount },
    });
    await waitFor(
      `${this.backend} accepting connections`,
      async () => (await docker.exec(this.dbName, db.healthCmd)).exitCode === 0,
      DB_TIMEOUT_MS,
    );
    log(`${this.backend} healthy for ${this.stackId}`);
  }

  private async startApp(log: (msg: string) => void): Promise<void> {
    const hostPort = await this.appHostPort();
    const env: Record<string, string> = {
      TAKUTO_SECRET_KEY: SECRET_KEY,
      TAKUTO_DATA_DIR: DATA_DIR,
      // Default: point the tools dir at a non-existent path so the server skips
      // the runtime agent-CLI install at boot. Otherwise the SPA shows a
      // full-screen "Installing dependencies" overlay that blocks every wizard
      // interaction. The onboarding suite verifies config/credential
      // persistence, not live agent execution, so installed CLIs are irrelevant.
      // With `installAgents`, the dir points at the real default (which EXISTS,
      // via the mounted cache volume) so the install runs.
      TAKUTO_TOOLS_DIR: this.installAgents ? TOOLS_DIR : '/nonexistent/takuto-tools-e2e',
    };
    if (this.db) {
      env.TAKUTO_DATABASE_CONNECTION = this.db.connection;
    }

    const volumes: Record<string, string> = { [this.dataVolume]: DATA_DIR };
    if (this.installAgents) {
      // Mount the cache volume at the PARENT dir; `agent_install` writes
      // `<dir>/bin`. The entrypoint chowns `/opt/takuto-tools` to `takuto`.
      volumes[this.toolsVolume] = TOOLS_DIR;
    }

    await this.writeSeedConfig(hostPort);

    if (this.useDind) {
      // Share the DinD container's netns so the reverse proxy reaches
      // docker-proxy ports via localhost (mirrors docker-compose.dind.yml). The
      // app port is published on the DinD container, so takuto gets NO publish
      // and NO own network. Worker spawns go to the DinD daemon over loopback.
      env.DOCKER_HOST = 'tcp://127.0.0.1:2375';
      env.TAKUTO_DIND_DATA_PREFIX = DIND_DATA_PREFIX;
      // Worker-image discovery falls through to this when the $HOSTNAME-inspect
      // and `takuto:latest` lookups miss (both do here); the tag was loaded into
      // the DinD daemon by startDindSidecar.
      env.TAKUTO_REGISTRY_IMAGE = this.image;
      volumes[this.workspaceVolume] = '/workspace';
      volumes[this.workspacesVolume] = WORKSPACES_DIR;
      volumes[this.configTmpDir] = CONFIG_DIR;
      log(`Starting takuto (${this.backend}) for ${this.stackId} sharing DinD netns…`);
      await docker.run({
        image: this.image,
        name: this.appName,
        labels: this.labels,
        env,
        extraArgs: ['--network', `container:${this.dindName}`],
        volumes,
      });
      return;
    }

    // Bind-mount the seed config.toml so the server's CSRF allowlist accepts
    // the published host-port origin; the data volume holds the DB + secret.
    volumes[join(this.configTmpDir, 'config.toml')] = CONFIG_TOML_PATH;
    log(`Starting takuto (${this.backend}) for ${this.stackId} on host port ${hostPort}…`);
    await docker.run({
      image: this.image,
      name: this.appName,
      labels: this.labels,
      env,
      network: this.netName,
      volumes,
      publish: [{ container: APP_PORT, host: hostPort, bindIp: '127.0.0.1' }],
    });
  }

  /**
   * Write a minimal seed `config.toml` whose `[web] cors_origins` lists the
   * published host-port origin. The server boots without a config.toml on
   * `Config::default()`, which auto-computes the CORS allowlist from the
   * CONTAINER's internal port (`:8080`) — but the stack is reached on a random
   * HOST port, so every mutating request (the browser wizard saves and the API
   * client) would otherwise be rejected by the Origin/Referer CSRF check. There
   * is no env override for `cors_origins`, so it is seeded via the file the CMD
   * already reads. Mode 0666 keeps it writable by any container uid, so the
   * completion-time `ConfigWriter` rewrite (which preserves the `[web]` table)
   * still lands. The file is removed in {@link teardown}.
   *
   * In DinD mode the dir already exists (created by {@link startDindSidecar} and
   * bind-mounted into both dind and takuto), and the body additionally makes
   * `opencode` the ACTIVE provider pointed at the in-DinD mock's IP, with
   * ticketing disabled — so a manual workflow runs a real `opencode` against the
   * mock.
   */
  private async writeSeedConfig(hostPort: number): Promise<void> {
    if (!this.configTmpDir) {
      this.configTmpDir = await mkdtemp(join(tmpdir(), `takuto-e2e-${this.stackId}-`));
    }
    const file = join(this.configTmpDir, 'config.toml');
    const origins = `["http://127.0.0.1:${hostPort}", "http://localhost:${hostPort}"]`;
    let body = `[web]\ncors_origins = ${origins}\n`;
    if (this.useDind) {
      // Manual-only: no ticketing poller. The fixture repo's `repositories`
      // row is auto-created by startup reconciliation (it scans WORKSPACES_DIR);
      // a spec associates the admin user via `POST /api/repositories`.
      body += '\n[general]\nticketing_system = "none"\n';
    }
    if (this.installAgents) {
      // `agent_install` derives its install set from `available_providers`
      // (+ acli, always), so listing all four installs all four.
      body += '\n[agent]\navailable_providers = ["claude", "cursor", "codex", "opencode"]\n';
      if (this.useDind) {
        // Active provider = opencode, pointed at the mock's `/v1` root by IP
        // (workers reach the in-DinD mock only by IP, not hostname). `model` is
        // required and arbitrary against the mock. mock_agent stays OFF so the
        // real `opencode` binary runs.
        body += `provider = "opencode"\n`;
        body += `\n[agent.providers.opencode]\nbase_url = "${this.dind?.mockBaseUrl ?? `http://${this.mockIp}:${MOCK_PORT}/v1`}"\nmodel = "${MOCK_MODEL}"\n`;
      }
      // Leave Cursor UNPINNED by default so the install exercises the official
      // installer path (and verifies the F1 fix); pin only when
      // TAKUTO_E2E_CURSOR_VERSION is set.
      if (CURSOR_VERSION) {
        body += `\n[agent.providers.cursor]\nversion = "${CURSOR_VERSION}"\n`;
      }
    }
    await writeFile(file, body, { mode: 0o666 });
  }

  private async waitHealthy(): Promise<void> {
    try {
      await waitFor(
        `takuto health at ${this.baseURL}/api/health`,
        async () => {
          const res = await fetch(`${this.baseURL}/api/health`);
          if (!res.ok) {
            return false;
          }
          const body = (await res.text()).trim().toLowerCase();
          return body === 'ok';
        },
        HEALTH_TIMEOUT_MS,
      );
    } catch (err) {
      const appLogs = await docker.logs(this.appName);
      let dbLogs = '';
      if (this.db) {
        dbLogs = `\n----- ${this.dbName} logs -----\n${await docker.logs(this.dbName)}`;
      }
      throw new Error(
        `${String(err)}\n----- ${this.appName} logs -----\n${appLogs}${dbLogs}`,
      );
    }
  }

  async restart(): Promise<void> {
    await docker.stop(this.appName);
    await docker.start(this.appName);
    // Published port mapping is preserved across stop/start, so baseURL holds.
    await this.waitHealthy();
  }

  async exec(command: string[]): Promise<{ stdout: string; stderr: string; exitCode: number }> {
    const result = await docker.exec(this.appName, command);
    return {
      stdout: result.stdout?.toString() ?? '',
      stderr: result.stderr?.toString() ?? '',
      exitCode: result.exitCode ?? -1,
    };
  }

  async waitAgentsInstalled(): Promise<void> {
    if (!this.installAgents) {
      return;
    }
    let lastStep = '';
    await waitFor(
      `agent CLI install (${this.baseURL}/api/system/dependencies)`,
      async () => {
        const res = await fetch(`${this.baseURL}/api/system/dependencies`);
        if (!res.ok) {
          return false;
        }
        const status = (await res.json()) as {
          phase: string;
          current_step?: string;
          error?: string;
        };
        if (status.phase === 'error') {
          throw new Error(`agent install failed: ${status.error ?? 'unknown error'}`);
        }
        if (status.current_step && status.current_step !== lastStep) {
          lastStep = status.current_step;
        }
        return status.phase === 'ready';
      },
      AGENT_INSTALL_TIMEOUT_MS,
      2000,
    );
  }

  async teardown(): Promise<void> {
    await docker.removeByLabel(`${LABEL_STACK}=${this.stackId}`);
    if (this.configTmpDir) {
      await rm(this.configTmpDir, { recursive: true, force: true }).catch(() => {});
    }
  }
}

/** Bring up one ephemeral stack for `backend`. Caller must `teardown()` it. */
export async function createStack(
  backend: Backend,
  log: (msg: string) => void = () => {},
  options: StackOptions = {},
): Promise<DockerStack> {
  const stack = new DockerStack(backend, options);
  try {
    await stack.up(log);
  } catch (err) {
    await stack.teardown();
    throw err;
  }
  return stack;
}

export type { DockerStack };

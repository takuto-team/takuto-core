import { createServer } from 'node:net';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import * as docker from './cli.js';
import {
  type Backend,
  newStackId,
  resourceName,
  stackLabels,
  LABEL_STACK,
} from './naming.js';
import { ensureImage } from './image.js';

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

/** In-container path the server reads/writes `config.toml` (Dockerfile CMD). */
const CONFIG_TOML_PATH = '/etc/takuto/config.toml';

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
  private readonly db: DbSpec | null;
  private dbName = '';
  private image = '';
  private configTmpDir = '';

  constructor(backend: Backend) {
    this.backend = backend;
    this.stackId = newStackId(backend);
    this.labels = stackLabels(this.stackId);
    this.netName = resourceName(this.stackId, 'net');
    this.appName = resourceName(this.stackId, 'app');
    this.dataVolume = resourceName(this.stackId, 'data');
    this.dbVolume = resourceName(this.stackId, 'dbdata');
    this.db = dbSpecFor(backend);
    if (this.db) {
      this.dbName = resourceName(this.stackId, 'db');
    }
  }

  async up(log: (msg: string) => void = () => {}): Promise<void> {
    this.image = await ensureImage(log);
    await docker.network.create(this.netName, this.labels);
    await docker.volume.create(this.dataVolume, this.labels);

    if (this.db) {
      await docker.volume.create(this.dbVolume, this.labels);
      await this.startDb(log);
    }
    await this.startApp(log);
    await this.waitHealthy();
    log(`Stack ${this.stackId} ready at ${this.baseURL}`);
  }

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
    const env: Record<string, string> = {
      TAKUTO_SECRET_KEY: SECRET_KEY,
      TAKUTO_DATA_DIR: DATA_DIR,
      // Point the tools dir at a non-existent path so the server skips the
      // runtime agent-CLI install at boot. Otherwise the SPA shows a
      // full-screen "Installing dependencies" overlay (the install reaches for
      // the network the ephemeral stack does not have) that blocks every wizard
      // interaction. The onboarding suite verifies config/credential
      // persistence, not live agent execution, so installed CLIs are irrelevant.
      TAKUTO_TOOLS_DIR: '/nonexistent/takuto-tools-e2e',
    };
    if (this.db) {
      env.TAKUTO_DATABASE_CONNECTION = this.db.connection;
    }
    const hostPort = await findFreePort();
    this.baseURL = `http://127.0.0.1:${hostPort}`;
    const configHostPath = await this.writeSeedConfig(hostPort);
    log(`Starting takuto (${this.backend}) for ${this.stackId} on host port ${hostPort}…`);
    await docker.run({
      image: this.image,
      name: this.appName,
      labels: this.labels,
      env,
      network: this.netName,
      // Bind-mount the seed config.toml so the server's CSRF allowlist accepts
      // the published host-port origin; the data volume holds the DB + secret.
      volumes: { [this.dataVolume]: DATA_DIR, [configHostPath]: CONFIG_TOML_PATH },
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
   */
  private async writeSeedConfig(hostPort: number): Promise<string> {
    this.configTmpDir = await mkdtemp(join(tmpdir(), `takuto-e2e-${this.stackId}-`));
    const file = join(this.configTmpDir, 'config.toml');
    const origins = `["http://127.0.0.1:${hostPort}", "http://localhost:${hostPort}"]`;
    await writeFile(file, `[web]\ncors_origins = ${origins}\n`, { mode: 0o666 });
    return file;
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
): Promise<DockerStack> {
  const stack = new DockerStack(backend);
  try {
    await stack.up(log);
  } catch (err) {
    await stack.teardown();
    throw err;
  }
  return stack;
}

export type { DockerStack };

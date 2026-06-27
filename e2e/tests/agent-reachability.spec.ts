import { test, expect } from '../src/fixtures/stack.fixture.js';
import type { TakutoStack } from '../src/fixtures/stack.fixture.js';

/**
 * Part A — agent-CLI install & reachability.
 *
 * Boots a stack with the runtime agent-CLI install enabled (the four providers
 * land in `/opt/takuto-tools/bin`, first on the container PATH) and proves, for
 * each of claude / cursor / opencode / codex, that:
 *   1. `<bin> --version` exits 0 and prints a parseable version token — the
 *      binary is installed and runnable.
 *   2. a MINIMAL real invocation (the same shape the session runners use), run
 *      with NO token configured, gets PAST "binary works" into an
 *      auth/connectivity/model failure — i.e. the CLI actually starts and tries
 *      to reach a model, rather than failing as a missing binary or a usage
 *      error. A non-zero exit is EXPECTED here (no credentials are present); the
 *      assertion only rejects failure shapes that mean the binary itself is
 *      broken or absent.
 */
test.use({ installAgents: true });

/** Generous wall-clock per case: a real invocation waits on a network attempt. */
const CASE_TIMEOUT_MS = 120_000;

/**
 * In-container wall-clock cap for the minimal real invocation. cursor-agent in
 * particular has a known habit of not exiting its headless `-p` mode; a hard
 * `timeout` keeps a stuck CLI from eating the whole test budget. A kill after a
 * short grace guarantees the exec returns. Exit 124 (timeout) still counts as
 * "the binary ran and tried" — the failure we reject is missing-binary / usage.
 */
const RUN_CAP_SECS = 60;

/** A digit-leading version token: `2.1.178`, `0.3.4`, `2026.06.26-7079533`. */
const VERSION_RE = /\d+\.\d+/;

/**
 * Output that proves the CLI started and reached its model/transport layer with
 * no credentials — auth, connectivity, or model/provider configuration errors.
 */
const REACHED_MODEL_RE =
  /(auth|unauthenticat|unauthor|api[\s_-]?key|log[\s]?in|sign[\s]?in|credential|token|forbidden|401|403|connect|connection|econnrefused|network|enotfound|getaddrinfo|dns|fetch failed|timed?\s?out|model|provider|base[\s_]?url|no\s+model|not\s+configured|configure)/i;

/**
 * Output that proves the binary is absent or the invocation never reached the
 * model layer (a shell could not find/start it, or the CLI rejected the argv).
 */
const BROKEN_BINARY_RE =
  /(command not found|no such file|not an executable|exec format error|cannot execute)/i;

interface AgentCli {
  /** Provider key, used as the test label and in failure reports. */
  provider: string;
  /** Binaries that must each resolve on PATH and report a version. */
  versionBins: string[];
  /** Minimal real invocation (no token) — the session-runner shape. */
  run: string[];
}

const CLIS: AgentCli[] = [
  {
    provider: 'claude',
    versionBins: ['claude'],
    run: ['claude', '--print', 'ping'],
  },
  {
    // Cursor installs as `agent` with a `cursor-agent` symlink; both resolve.
    provider: 'cursor',
    versionBins: ['cursor-agent', 'agent'],
    run: ['cursor-agent', '-p', 'ping'],
  },
  {
    provider: 'opencode',
    versionBins: ['opencode'],
    run: ['opencode', 'run', '--format', 'json', '--dangerously-skip-permissions', 'ping'],
  },
  {
    provider: 'codex',
    versionBins: ['codex'],
    run: [
      'codex',
      'exec',
      '--json',
      '--skip-git-repo-check',
      '--dangerously-bypass-approvals-and-sandbox',
      '--ignore-user-config',
      '--cd',
      '/tmp',
      'ping',
    ],
  },
];

async function versionOf(
  stack: TakutoStack,
  bin: string,
): Promise<{ exitCode: number; out: string }> {
  const res = await stack.exec([bin, '--version']);
  return { exitCode: res.exitCode, out: `${res.stdout}\n${res.stderr}`.trim() };
}

test.describe('agent CLI reachability', () => {
  // Dedicated regression guard for the startup dependency install (F1 + any
  // future breakage of claude/cursor/codex/opencode/acli). The dashboard
  // surfaces a `phase: error` install as the "Could not install dependencies"
  // overlay; this asserts the install reaches `ready` with NO error. Cursor is
  // UNPINNED by default here (no `TAKUTO_E2E_CURSOR_VERSION`), so it exercises
  // the real `cursor.com/install` path — the exact path F1 broke. If any startup
  // installer regresses, this test (and `waitAgentsInstalled`, which throws on
  // `phase: error`) fails loudly.
  test('startup dependency install completes without error (all startup binaries)', async ({
    stack,
  }) => {
    test.setTimeout(CASE_TIMEOUT_MS);
    await stack.waitAgentsInstalled();
    const res = await fetch(`${stack.baseURL}/api/system/dependencies`);
    expect(res.ok, 'GET /api/system/dependencies should succeed').toBeTruthy();
    const status = (await res.json()) as { phase: string; error?: string | null };
    expect(status.error ?? '', `install reported an error: ${status.error ?? ''}`).toBe('');
    expect(status.phase, 'install did not reach the ready phase').toBe('ready');
  });

  for (const cli of CLIS) {
    test(`${cli.provider}: installed, runnable, and reaches its model layer`, async ({
      stack,
    }) => {
      test.setTimeout(CASE_TIMEOUT_MS);

      // Install is awaited by the fixture at boot; awaiting again is idempotent
      // and makes the dependency explicit for this spec.
      await stack.waitAgentsInstalled();

      // (1) Every binary for this provider resolves on PATH and reports a
      // version. No exact-version assertion — only "exit 0 + a version token".
      for (const bin of cli.versionBins) {
        const { exitCode, out } = await versionOf(stack, bin);
        expect(exitCode, `${bin} --version exit code (stdout/stderr: ${out})`).toBe(0);
        expect(out.length, `${bin} --version produced no output`).toBeGreaterThan(0);
        expect(out, `${bin} --version output had no version token`).toMatch(VERSION_RE);
      }

      // (2) Minimal real invocation with NO token. A non-zero exit is EXPECTED:
      // there are no credentials, so the CLI should fail at the auth/network/
      // model stage — which proves it ran rather than being a missing binary.
      // Hard-cap the in-container time so a hung CLI cannot stall the test.
      const runRes = await stack.exec(['timeout', '-k', '5', String(RUN_CAP_SECS), ...cli.run]);
      const runOut = `${runRes.stdout}\n${runRes.stderr}`.trim();
      const report = `exit=${runRes.exitCode} output=${runOut.slice(0, 600)}`;

      // It must NOT be a missing/un-runnable binary.
      expect(runRes.exitCode, `${cli.provider} run was command-not-found (${report})`).not.toBe(
        127,
      );
      expect(runOut, `${cli.provider} run looks like a broken binary (${report})`).not.toMatch(
        BROKEN_BINARY_RE,
      );

      // It must have gotten PAST "binary works": either it surfaced an
      // auth/network/model error, or it was still working (timeout exit 124)
      // when the cap fired — both mean the CLI started and tried to respond.
      const reachedModel = REACHED_MODEL_RE.test(runOut);
      const stillRunningAtCap = runRes.exitCode === 124;
      expect(
        reachedModel || stillRunningAtCap,
        `${cli.provider} did not reach an auth/network/model failure (${report})`,
      ).toBeTruthy();
    });
  }
});

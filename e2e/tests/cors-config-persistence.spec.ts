import { test, expect } from '../src/fixtures/stack.fixture.js';
import { request } from '@playwright/test';
import { parseToml, type TomlTable } from '../src/api/toml.js';

/**
 * Proves the onboarding-completion `ConfigWriter` rewrite PRESERVES
 * `[web].cors_origins`. The stack seeds a config.toml whose allowlist carries
 * the published host-port origin (the server's default allowlist only knows the
 * container's internal :8080). `POST /api/onboarding/complete` does a FULL
 * serialize of the live in-memory config back to that bind-mounted file; if the
 * rewrite dropped or defaulted `[web]`, the post-restart server would 403 every
 * mutating request again. CORS handling is backend-independent, so this runs
 * once (sqlite).
 */
function corsOrigins(toml: TomlTable): string[] {
  const web = toml.web;
  if (typeof web !== 'object' || web === null || Array.isArray(web)) {
    return [];
  }
  const cors = (web as TomlTable).cors_origins;
  if (!Array.isArray(cors)) {
    return [];
  }
  return cors.filter((v): v is string => typeof v === 'string');
}

test('ConfigWriter preserves [web].cors_origins across onboarding complete + restart', async ({
  stack,
  backend,
}) => {
  test.skip(backend !== 'sqlite', 'CORS allowlist handling is backend-independent');
  test.setTimeout(120_000);

  const origin = stack.baseURL;
  const creds = { username: 'e2e-admin', password: 'e2e-admin-pw-123' };
  const headers = { Origin: origin };

  // 1) Seed config.toml on disk already allowlists the host-port origin.
  const seed = await stack.exec(['cat', '/etc/takuto/config.toml']);
  expect(seed.exitCode).toBe(0);
  expect(corsOrigins(parseToml(seed.stdout))).toContain(origin);

  // 2) Bootstrap admin and finish onboarding — this is the ConfigWriter rewrite.
  const ctx = await request.newContext({ baseURL: stack.baseURL });
  const status = await (await ctx.get('/api/auth/status')).json();
  if (status.setup_required) {
    expect((await ctx.post('/api/auth/register', { data: creds, headers })).status()).toBe(201);
  }
  expect((await ctx.post('/api/auth/login', { data: creds, headers })).status()).toBe(204);

  const complete = await ctx.post('/api/onboarding/complete', { headers });
  // A 200 here also proves the CSRF allowlist already accepts the host-port origin.
  expect(complete.ok()).toBeTruthy();

  // 3) The file was fully rewritten (now carries the other sections) AND the
  //    host-port origin survived the rewrite.
  const after = await stack.exec(['cat', '/etc/takuto/config.toml']);
  expect(after.exitCode).toBe(0);
  expect(after.stdout).toContain('[agent]');
  expect(after.stdout).toContain('[database]');
  expect(corsOrigins(parseToml(after.stdout))).toContain(origin);

  // 4) After a restart the server reloads the rewritten file; a mutating POST
  //    must still be accepted (204), not 403 — the strongest persistence proof.
  await stack.restart();
  const ctx2 = await request.newContext({ baseURL: stack.baseURL });
  expect((await ctx2.post('/api/auth/login', { data: creds, headers })).status()).toBe(204);

  await ctx.dispose();
  await ctx2.dispose();
});

import { defineConfig, devices } from '@playwright/test';
import type { Backend, StackWorkerOptions } from './src/fixtures/stack.fixture.js';
import { BACKENDS } from './src/docker/naming.js';

/**
 * Backends under test. Scope down locally with e.g.
 * `TAKUTO_E2E_BACKENDS=sqlite` or `TAKUTO_E2E_BACKENDS=sqlite,postgres`.
 */
function selectedBackends(): Backend[] {
  const raw = process.env.TAKUTO_E2E_BACKENDS;
  if (!raw) {
    return [...BACKENDS];
  }
  const requested = raw.split(',').map((s) => s.trim().toLowerCase());
  return BACKENDS.filter((b) => requested.includes(b));
}

const backends = selectedBackends();
const workers = Number(process.env.TAKUTO_E2E_WORKERS ?? 2);

export default defineConfig<object, StackWorkerOptions>({
  testDir: './tests',
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 0,
  // Container boots dominate wall-clock; cap concurrency to spare Docker.
  workers,
  reporter: [['html', { open: 'never' }], ['list']],
  globalSetup: './global-setup.ts',
  globalTeardown: './global-teardown.ts',
  // Generous per-test budget: a test may complete the wizard and restart the
  // container (stop → start → re-migrate → health) within it.
  timeout: 120_000,
  expect: { timeout: 15_000 },
  use: {
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    actionTimeout: 15_000,
    navigationTimeout: 30_000,
  },
  projects: backends.map((backend) => ({
    name: backend,
    use: { ...devices['Desktop Chrome'], backend },
  })),
});

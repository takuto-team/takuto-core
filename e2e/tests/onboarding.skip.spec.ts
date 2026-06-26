import { test, expect } from '../src/fixtures/admin.fixture.js';
import { readConfigTomlViaExec } from '../src/api/client.js';
import { ALL_PROVIDERS, ALL_TICKETING } from '../src/data/matrix.js';
import { finishWithDefaults } from './support/walk.js';

/**
 * Skip paths — completion must work even when the operator enters nothing. These
 * are provider/ticketing-invariant, so they run once per backend project.
 *
 * Assertions are order-independent (the stack is shared across a worker's tests):
 * we prove the skip → complete path writes a structurally valid `config.toml`
 * and flips the completion flag, without pinning the values to pristine defaults
 * a previous test on the same stack may have overwritten.
 */
test.describe('onboarding wizard — skip paths', () => {
  test('completing on defaults still finishes and writes a valid config.toml', async ({
    page,
    stack,
    admin,
  }) => {
    await finishWithDefaults(page);

    // Completion still succeeds with nothing entered.
    expect(await admin.api.isOnboardingComplete()).toBe(true);

    // The server wrote a structurally valid config.toml: a known provider, a
    // ticketing system, non-empty git settings, and a positive step timeout.
    const toml = await readConfigTomlViaExec(stack);
    const agent = toml.agent as Record<string, unknown>;
    const general = toml.general as Record<string, unknown>;
    const git = toml.git as Record<string, unknown>;

    expect(ALL_PROVIDERS).toContain(agent.provider);
    expect(ALL_TICKETING).toContain(general.ticketing_system);
    expect(typeof git.base_branch).toBe('string');
    expect((git.base_branch as string).length).toBeGreaterThan(0);
    expect(typeof git.remote).toBe('string');
    expect((git.remote as string).length).toBeGreaterThan(0);
    expect(typeof agent.step_timeout_secs).toBe('number');
    expect(agent.step_timeout_secs as number).toBeGreaterThanOrEqual(1);
  });

  test('the header "Skip setup" link leaves the wizard', async ({ page, admin }) => {
    await admin.wizard.skipSetup();

    await page.waitForURL((url) => !url.pathname.startsWith('/onboarding'));
    expect(new URL(page.url()).pathname).not.toBe('/onboarding');
  });
});

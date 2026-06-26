import { test, expect } from '../src/fixtures/admin.fixture.js';
import { readConfigTomlViaExec } from '../src/api/client.js';
import {
  caseLabel,
  expectedConfigToml,
  expectedProviderConfig,
  onboardingCases,
  GIT_INPUT,
  STEP_TIMEOUT_SECS,
} from '../src/data/matrix.js';
import { completeWizard } from './support/walk.js';

/**
 * Happy-path completion across the provider × ticketing cartesian (one Playwright
 * project per backend). For each case: the `admin` fixture registers + logs in
 * the first admin and lands on `/onboarding`; we walk all five steps with the
 * case's provider/ticketing inputs, Finish, then assert the settings landed in
 * all three places a completed wizard writes them — the onboarding state, the
 * live config read-back, and the on-disk `config.toml` inside the container.
 */
test.describe('onboarding wizard — happy path', () => {
  for (const c of onboardingCases()) {
    test(`completes and persists (${caseLabel(c)})`, async ({ page, stack, admin }) => {
      await completeWizard(page, c);

      // 1. Completion signal — `completed_at` non-null.
      expect(await admin.api.isOnboardingComplete()).toBe(true);

      // 2. Live config read-back (flattened GET /api/config).
      const cfg = await admin.api.getConfig();
      expect(cfg).toMatchObject({
        git: { base_branch: GIT_INPUT.baseBranch, remote: GIT_INPUT.remote },
        agent: {
          provider: c.provider,
          step_timeout_secs: STEP_TIMEOUT_SECS,
          providers: { [c.provider]: expectedProviderConfig(c.provider) },
        },
        general: { ticketing_system: c.ticketing },
        ticketing_system: c.ticketing,
      });

      // 3. The `config.toml` the server wrote on completion (read via exec).
      const expected = expectedConfigToml(c);
      const toml = await readConfigTomlViaExec(stack);
      expect(toml).toMatchObject({
        agent: { provider: expected.provider, step_timeout_secs: expected.stepTimeoutSecs },
        general: { ticketing_system: expected.ticketingSystem },
        git: { base_branch: expected.baseBranch, remote: expected.remote },
      });
    });
  }
});

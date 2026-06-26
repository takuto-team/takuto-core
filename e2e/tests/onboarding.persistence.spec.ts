import { test, expect } from '../src/fixtures/admin.fixture.js';
import { OnboardingApi } from '../src/api/client.js';
import {
  caseLabel,
  expectedProviderConfig,
  onboardingCases,
  GIT_INPUT,
  PROVIDER_INPUTS,
  STEP_TIMEOUT_SECS,
} from '../src/data/matrix.js';
import { completeWizard } from './support/walk.js';

/**
 * Restart-persistence — the strongest proof the settings live in the database,
 * not just process memory. For each case: complete the wizard, restart the
 * Takuto container in place (same data volume, same database, same pinned master
 * key), then re-login through a fresh browserless client and assert the admin
 * user, the onboarding completion, the config, and the encrypted per-user
 * provider credential all survived the reboot.
 *
 * Only the AI api_key credential is exercised here: it is shape-validated
 * server-side, so it stores (and later decrypts) without any live provider
 * round-trip. A dummy GitHub PAT / Jira credential cannot be stored offline
 * because those endpoints validate live against GitHub / Atlassian.
 */
test.describe('onboarding wizard — restart persistence', () => {
  for (const c of onboardingCases()) {
    test(`survives a container restart (${caseLabel(c)})`, async ({ page, stack, admin }) => {
      await completeWizard(page, c);
      expect(await admin.api.isOnboardingComplete()).toBe(true);

      // Seal a dummy provider api_key so there is an encrypted credential row to
      // prove survives the reboot. Stored via the documented endpoint (the
      // server shape-validates the key — no live provider round-trip), which is
      // deterministic; the wizard's masked-when-connected key panel is not.
      await admin.api.setProviderCredential(c.provider, {
        api_key: PROVIDER_INPUTS[c.provider].apiKey,
      });

      // Reboot the app container against the same DB + master key. `baseURL`
      // holds across the restart (the published port is preserved).
      await stack.restart();

      const fresh = await OnboardingApi.create(stack.baseURL);
      try {
        // Admin row + auth machinery survived: re-login succeeds (204).
        await fresh.login(admin.creds);

        // Completion + config survived the reboot.
        expect(await fresh.isOnboardingComplete()).toBe(true);
        const after = await fresh.getConfig();
        expect(after).toMatchObject({
          git: { base_branch: GIT_INPUT.baseBranch, remote: GIT_INPUT.remote },
          agent: {
            provider: c.provider,
            step_timeout_secs: STEP_TIMEOUT_SECS,
            providers: { [c.provider]: expectedProviderConfig(c.provider) },
          },
          general: { ticketing_system: c.ticketing },
        });

        // Encrypted credential row decrypts with the pinned master key after
        // the restart — the DB-persistence + envelope-encryption proof.
        const creds = await fresh.getUserCredentials(c.provider);
        expect(creds.provider?.api_key?.active).toBe(true);
      } finally {
        await fresh.dispose();
      }
    });
  }
});

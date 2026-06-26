import { test as stackTest, expect } from './stack.fixture.js';
import { OnboardingApi } from '../api/client.js';
import { newAdminCredentials } from '../api/credentials.js';
import type { AdminCredentials } from '../api/types.js';
import { OnboardingWizard } from '../pages/OnboardingWizard.js';

/**
 * The result of bootstrapping the first admin: the credentials used, any
 * recovery codes minted at registration, a logged-in API client, and a wizard
 * Page Object pointed at `/onboarding`.
 */
export interface AdminSession {
  creds: AdminCredentials;
  recoveryCodes: string[];
  api: OnboardingApi;
  wizard: OnboardingWizard;
}

/** Test-scoped fixtures layered on top of the worker-scoped stack. */
export interface AdminTestFixtures {
  /**
   * A typed Takuto API client bound to `page.request`, so the session cookie set
   * by login lands in the page's browser context — `page.goto` is then
   * authenticated and read-back GETs reuse the same session.
   */
  api: OnboardingApi;
  /**
   * Registers + logs in the first admin (idempotent on a reused stack) and lands
   * the page on `/onboarding`. Yields the live {@link AdminSession}.
   */
  admin: AdminSession;
}

/** Worker-scoped fixtures: credentials stable for the lifetime of the stack. */
export interface AdminWorkerFixtures {
  /**
   * First-admin credentials, minted once per worker. Stable so the idempotent
   * bootstrap can re-login against a stack a worker has already set up.
   */
  adminCreds: AdminCredentials;
}

export const test = stackTest.extend<AdminTestFixtures, AdminWorkerFixtures>({
  adminCreds: [
    // Playwright requires the first fixture argument to be a destructuring
    // pattern; this worker fixture depends on none.
    // eslint-disable-next-line no-empty-pattern
    async ({}, use) => {
      await use(newAdminCredentials());
    },
    { scope: 'worker' },
  ],

  api: async ({ page, stack }, use) => {
    await use(new OnboardingApi(page.request, stack.baseURL));
  },

  admin: async ({ page, api, adminCreds }, use) => {
    const recoveryCodes = await api.bootstrapAdmin(adminCreds);
    await page.goto('/onboarding');
    const wizard = new OnboardingWizard(page);
    await use({ creds: adminCreds, recoveryCodes, api, wizard });
  },
});

export { expect };

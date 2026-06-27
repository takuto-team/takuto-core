import { expect, type Locator, type Page } from '@playwright/test';
import { OnboardingWizard } from './OnboardingWizard.js';

/**
 * Visible strings / element ids for the per-step onboarding bodies, driven
 * through real form controls. Centralized so specs never hard-code a label
 * (English i18n `onboarding.json` / `credentials.json`).
 *
 * The wizard's 5 steps (see `pages/Onboarding.tsx`):
 *   1. Git & GitHub   2. Repositories   3. AI provider
 *   4. Ticketing      5. Workflows (+ Finish)
 */
export const ONBOARDING_STEP_LABELS = {
  provider: {
    select: '#onb-provider',
    baseUrl: '#onb-base-url',
    model: '#onb-model',
    /** `provider.options.opencode` */
    opencodeOption: 'OpenCode (self-hosted)',
  },
  /** `credentials.json my.ai.bearerLabel` — the opencode key field label. */
  bearerLabel: 'Bearer token (optional)',
  ticketing: {
    select: '#onb-ticketing',
    /** `ticketing.options.none.label` */
    noneOption: 'None',
  },
  /** `config.json repositories.available.add` */
  repoAddButton: 'Add',
  /** `repositories.available.searchPlaceholder` */
  repoSearchPlaceholder: 'Search…',
} as const;

/**
 * Page Object that drives the onboarding wizard end to end via its real form
 * controls (provider select, base-url/model inputs, the inline key field, the
 * repository "Add" button, the ticketing select), reusing {@link OnboardingWizard}
 * for the footer Back / Continue / Finish navigation.
 *
 * Persists each step the way the wizard does: typed credentials and the
 * provider/ticketing selections are flushed by clicking "Save and Continue"
 * (the wizard's `onBeforeNext` saves them), and the repository "Add" persists
 * via its own button on step 2.
 */
export class OnboardingSteps {
  readonly page: Page;
  readonly wizard: OnboardingWizard;

  constructor(page: Page) {
    this.page = page;
    this.wizard = new OnboardingWizard(page);
  }

  async goto(): Promise<void> {
    await this.wizard.goto();
  }

  /** Step 1 (Git & GitHub): accept deployment defaults and continue to step 2. */
  async completeGitStep(): Promise<void> {
    await this.wizard.expectStep(1);
    await this.wizard.saveAndContinue();
    await this.wizard.expectStep(2);
  }

  /**
   * Step 2 (Repositories): confirm the fixture repo is present in "My
   * repositories" and continue.
   *
   * The repo is associated out-of-band via the API: the onboarding/Config
   * "Available repositories" list is sourced from GitHub-accessible repos
   * (added by clone URL), so a locally-reconciled repo with no GitHub backing
   * has no UI affordance to add (see FINDINGS F4). This step therefore only
   * verifies the (API-associated) repo shows up and advances.
   */
  async confirmRepository(name: string): Promise<void> {
    await this.wizard.expectStep(2);
    await expect(this.page.getByText(name, { exact: true }).first()).toBeVisible({
      timeout: 30_000,
    });
    await this.wizard.saveAndContinue();
    await this.wizard.expectStep(3);
  }

  /**
   * Step 3 (AI provider): pick opencode, set base_url + model, type the bearer
   * key into the inline credential field, then continue (the wizard persists
   * both the provider config and the key on "Save and Continue").
   */
  async configureProvider(opts: { baseUrl: string; model: string; bearer: string }): Promise<void> {
    await this.wizard.expectStep(3);
    await this.page
      .locator(ONBOARDING_STEP_LABELS.provider.select)
      .selectOption({ label: ONBOARDING_STEP_LABELS.provider.opencodeOption });
    await this.page.locator(ONBOARDING_STEP_LABELS.provider.baseUrl).fill(opts.baseUrl);
    await this.page.locator(ONBOARDING_STEP_LABELS.provider.model).fill(opts.model);
    // Inline key field renders with hideSave (deferSave) — typing is enough; the
    // wizard's "Save and Continue" flushes it via the panel's saveIfDirty handle.
    await this.page.getByLabel(ONBOARDING_STEP_LABELS.bearerLabel).fill(opts.bearer);
    await this.wizard.saveAndContinue();
    await this.wizard.expectStep(4);
  }

  /** Step 4 (Ticketing): select "None" and continue to step 5. */
  async selectTicketingNone(): Promise<void> {
    await this.wizard.expectStep(4);
    await this.page
      .locator(ONBOARDING_STEP_LABELS.ticketing.select)
      .selectOption({ label: ONBOARDING_STEP_LABELS.ticketing.noneOption });
    await this.wizard.saveAndContinue();
    await this.wizard.expectStep(5);
  }

  /** Step 5 (Workflows): finish setup (flows are configured later in Config UI). */
  async finish(): Promise<void> {
    await this.wizard.expectStep(5);
    await this.wizard.finish();
  }

  /** Drive all five steps with the given provider + repo settings. */
  async runFullWizard(opts: {
    repository: string;
    baseUrl: string;
    model: string;
    bearer: string;
  }): Promise<void> {
    await this.completeGitStep();
    await this.confirmRepository(opts.repository);
    await this.configureProvider(opts);
    await this.selectTicketingNone();
    await this.finish();
  }

  /** The repo's "Added"/present marker locator (for assertions). */
  myRepositoryRow(name: string): Locator {
    return this.page.locator('li').filter({ hasText: name });
  }
}

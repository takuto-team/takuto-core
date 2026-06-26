import { expect, type Locator, type Page } from '@playwright/test';

/**
 * Footer / header / stepper control strings. These controls carry no `#onb-*`
 * ids, so they are targeted by role + visible text — centralized here per the
 * contract (§3) so specs never hard-code a label. Values are the English i18n
 * resources (`onboarding.json` `nav.*` / `header.skip`).
 */
export const WIZARD_LABELS = {
  back: '← Back',
  continue: 'Save and Continue',
  finish: 'Finish setup',
  skip: 'Skip setup →',
} as const;

/** Total number of wizard steps (`ONBOARDING_STEPS`, `Stepper.tsx:7-13`). */
export const STEP_COUNT = 5;

/**
 * Base Page Object for the onboarding wizard shell: navigation, stepper state,
 * and the footer Back / Save-and-Continue / Finish controls. Step bodies are
 * driven by the per-step Page Objects (`GitHubStep`, `ProviderStep`, …), each of
 * which takes the same `page`.
 */
export class OnboardingWizard {
  readonly page: Page;

  constructor(page: Page) {
    this.page = page;
  }

  /** Navigate straight to the wizard (assumes an authenticated session). */
  async goto(): Promise<void> {
    await this.page.goto('/onboarding');
  }

  /** The footer's primary button (Save and Continue on steps 1-4, Finish on 5). */
  primaryButton(): Locator {
    return this.page.getByRole('button', {
      name: new RegExp(`${escapeRegExp(WIZARD_LABELS.continue)}|${escapeRegExp(WIZARD_LABELS.finish)}`),
    });
  }

  /** The footer's Back button (disabled on step 1). */
  backButton(): Locator {
    return this.page.getByRole('button', { name: WIZARD_LABELS.back });
  }

  /** The active stepper item, carrying `aria-current="step"`. */
  activeStepItem(): Locator {
    return this.page.locator('li[aria-current="step"]');
  }

  /** Read the 1-based index of the currently active step from the stepper. */
  async activeStep(): Promise<number> {
    const text = (await this.activeStepItem().textContent()) ?? '';
    const match = /^\s*(\d+)\./.exec(text);
    if (!match) {
      throw new Error(`could not parse active step from stepper text "${text}"`);
    }
    return Number.parseInt(match[1], 10);
  }

  /** Assert the wizard is showing step `n` (waits up to the expect timeout). */
  async expectStep(n: number): Promise<void> {
    await expect(this.activeStepItem()).toHaveText(new RegExp(`^\\s*${n}\\.`));
  }

  /** Click Back (steps 2-5). */
  async back(): Promise<void> {
    await this.backButton().click();
  }

  /**
   * Click "Save and Continue". On success the wizard advances; on a blocked
   * step (validation failure) it stays put and surfaces an inline error. Callers
   * assert the resulting step / error themselves.
   */
  async saveAndContinue(): Promise<void> {
    await this.page.getByRole('button', { name: WIZARD_LABELS.continue }).click();
  }

  /**
   * Click "Finish setup" on step 5. On success the app posts
   * `/api/onboarding/complete` and navigates away from `/onboarding`. Pass
   * `expectNavigation: false` for validation cases where Finish is blocked.
   */
  async finish(opts: { expectNavigation?: boolean } = {}): Promise<void> {
    const expectNavigation = opts.expectNavigation ?? true;
    await this.page.getByRole('button', { name: WIZARD_LABELS.finish }).click();
    if (expectNavigation) {
      await this.page.waitForURL((url) => !url.pathname.startsWith('/onboarding'));
    }
  }

  /** Click the header "Skip setup →" link (jumps to `/`). */
  async skipSetup(): Promise<void> {
    await this.page.getByRole('link', { name: WIZARD_LABELS.skip }).click();
  }

  /** Locator for an inline validation message by its exact text. */
  validationError(message: string): Locator {
    return this.page.getByText(message, { exact: true });
  }

  /** Assert an inline validation message is visible. */
  async expectValidationError(message: string): Promise<void> {
    await expect(this.validationError(message)).toBeVisible();
  }
}

/** Escape a literal string for safe interpolation into a RegExp. */
function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

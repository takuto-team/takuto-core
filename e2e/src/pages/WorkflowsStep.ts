import { expect, type Locator, type Page } from '@playwright/test';

/** Inline message for an invalid step timeout (English `onboarding.json`). */
export const WORKFLOWS_ERRORS = {
  stepTimeoutInvalid: 'Step timeout must be a positive number.',
} as const;

/**
 * Step 5 — Workflows. Wraps the step-timeout input (`#onb-step-timeout`, a
 * `min=1` number). A blank or non-positive value blocks "Finish setup" with an
 * inline error. The flows editor below it has no `#onb-*` ids and is left at its
 * seeded defaults by the acceptance specs.
 */
export class WorkflowsStep {
  readonly page: Page;

  constructor(page: Page) {
    this.page = page;
  }

  stepTimeoutInput(): Locator {
    return this.page.locator('#onb-step-timeout');
  }

  async fillStepTimeout(value: number | string): Promise<void> {
    await this.stepTimeoutInput().fill(String(value));
  }

  async getStepTimeout(): Promise<string> {
    return this.stepTimeoutInput().inputValue();
  }

  async expectStepTimeoutInvalid(): Promise<void> {
    await expect(
      this.page.getByText(WORKFLOWS_ERRORS.stepTimeoutInvalid, { exact: true }),
    ).toBeVisible();
  }
}

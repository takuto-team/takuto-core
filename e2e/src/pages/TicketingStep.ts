import { expect, type Locator, type Page } from '@playwright/test';
import type { TicketingId } from '../api/types.js';

/**
 * Message shown when the Jira form is partially filled. Substring of the
 * `jiraPartial` string in `ui/src/locales/en/config.json` ("Fill in the Jira
 * site, email, and API token …").
 */
export const TICKETING_ERRORS = {
  jiraPartial: /Fill in the Jira site/,
} as const;

/**
 * Step 4 — Ticketing. Wraps the system `<select>` (`#onb-ticketing`) and, when
 * the system is `jira`, the site / email / token inputs (`#onb-jira-*`). The
 * Jira fields render only while `jira` is selected. A partially-filled Jira form
 * (1-2 of the 3 fields, when not already connected) blocks Continue with a toast.
 */
export class TicketingStep {
  readonly page: Page;

  constructor(page: Page) {
    this.page = page;
  }

  systemSelect(): Locator {
    return this.page.locator('#onb-ticketing');
  }

  jiraSiteInput(): Locator {
    return this.page.locator('#onb-jira-site');
  }

  jiraEmailInput(): Locator {
    return this.page.locator('#onb-jira-email');
  }

  jiraTokenInput(): Locator {
    return this.page.locator('#onb-jira-token');
  }

  async selectSystem(system: TicketingId): Promise<void> {
    await this.systemSelect().selectOption(system);
  }

  /** Fill all three Jira fields (assumes `jira` is already selected). */
  async fillJira(values: { site: string; email: string; token?: string }): Promise<void> {
    await this.page.locator('#onb-jira-site').fill(values.site);
    await this.page.locator('#onb-jira-email').fill(values.email);
    if (values.token !== undefined) {
      await this.page.locator('#onb-jira-token').fill(values.token);
    }
  }

  async getSystem(): Promise<string> {
    return this.systemSelect().inputValue();
  }

  /** Whether the system selector is disabled (true for a non-admin caller). */
  async isSystemDisabled(): Promise<boolean> {
    return this.systemSelect().isDisabled();
  }

  /** Assert the partial-Jira-form error is shown (Continue was blocked). */
  async expectJiraPartialError(): Promise<void> {
    await expect(this.page.getByText(TICKETING_ERRORS.jiraPartial)).toBeVisible();
  }
}

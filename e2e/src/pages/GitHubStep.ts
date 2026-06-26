import { expect, type Locator, type Page } from '@playwright/test';

/** Inline validation messages for step 1 (English `onboarding.json` `git.*`). */
export const GIT_ERRORS = {
  baseBranchRequired: 'Base branch is required.',
  remoteRequired: 'Remote is required.',
} as const;

/**
 * Step 1 — Git & GitHub. Exposes the two required git inputs (`#onb-git-*`) plus
 * the per-user PAT panel (no `#onb-*` id — anchored on the GitHub card's
 * `aria-labelledby="gh-card-title"`).
 */
export class GitHubStep {
  readonly page: Page;

  constructor(page: Page) {
    this.page = page;
  }

  baseBranchInput(): Locator {
    return this.page.locator('#onb-git-base-branch');
  }

  remoteInput(): Locator {
    return this.page.locator('#onb-git-remote');
  }

  /** The per-user PAT field inside the GitHub credential card. */
  patInput(): Locator {
    return this.page.locator('section[aria-labelledby="gh-card-title"] input[type="password"]');
  }

  async fillBaseBranch(value: string): Promise<void> {
    await this.baseBranchInput().fill(value);
  }

  async fillRemote(value: string): Promise<void> {
    await this.remoteInput().fill(value);
  }

  /** Enter the deployment git settings in one call. */
  async fill(values: { baseBranch: string; remote: string }): Promise<void> {
    await this.fillBaseBranch(values.baseBranch);
    await this.fillRemote(values.remote);
  }

  /** Paste a personal access token into the PAT panel (persisted on Continue). */
  async fillPat(token: string): Promise<void> {
    await this.patInput().fill(token);
  }

  async getBaseBranch(): Promise<string> {
    return this.baseBranchInput().inputValue();
  }

  async getRemote(): Promise<string> {
    return this.remoteInput().inputValue();
  }

  async expectBaseBranchRequired(): Promise<void> {
    await expect(this.page.getByText(GIT_ERRORS.baseBranchRequired, { exact: true })).toBeVisible();
  }

  async expectRemoteRequired(): Promise<void> {
    await expect(this.page.getByText(GIT_ERRORS.remoteRequired, { exact: true })).toBeVisible();
  }
}

import { expect, type Locator, type Page } from '@playwright/test';

/**
 * Visible strings for the paste-description ("New Work Item") modal. Centralized
 * so specs never hard-code a label (English i18n `modals.json` `paste.*` /
 * `common.cancel`).
 */
export const PASTE_MODAL_LABELS = {
  title: 'New Work Item',
  nameLabel: 'Work item name (optional)',
  descriptionLabel: 'Description',
  submit: 'Add to Dashboard',
  cancel: 'Cancel',
} as const;

/**
 * Page Object for `PasteDescriptionModal` — the manual "add work item" flow used
 * when `ticketing_system = none`. Opened from the dashboard **+** tile
 * ({@link DashboardPage.openAddModal}).
 *
 * Layout: a repository control (a `<select>` when >1 repo, a static label when
 * exactly one), an optional name input, and a required description textarea.
 * Submit posts the manual workflow (`onPasteSubmit`).
 */
export class PasteDescriptionModal {
  readonly page: Page;
  private readonly root: Locator;

  constructor(page: Page) {
    this.page = page;
    this.root = page.locator('.modal-backdrop').filter({
      has: page.getByRole('heading', { name: PASTE_MODAL_LABELS.title }),
    });
  }

  /** Assert the modal is open. */
  async expectOpen(): Promise<void> {
    await expect(this.root.getByRole('heading', { name: PASTE_MODAL_LABELS.title })).toBeVisible();
  }

  private nameInput(): Locator {
    return this.root.getByPlaceholder('e.g. add-user-login');
  }

  private descriptionInput(): Locator {
    return this.root.getByPlaceholder('Paste your ticket description here...');
  }

  private repoSelect(): Locator {
    return this.root.locator('select');
  }

  /** Type the work-item name (slugified into `ticket_key`). */
  async fillName(name: string): Promise<void> {
    await this.nameInput().fill(name);
  }

  /** Type the description (becomes `ticket_description`). */
  async fillDescription(description: string): Promise<void> {
    await this.descriptionInput().fill(description);
  }

  /**
   * Select the repository by its visible name. No-op when the deployment has a
   * single repo (rendered as a static label, auto-selected) — pass nothing in
   * that case.
   */
  async selectRepository(name: string): Promise<void> {
    await this.repoSelect().selectOption({ label: name });
  }

  /** Whether the repo control is the multi-repo `<select>` (vs a single static label). */
  async hasRepoSelect(): Promise<boolean> {
    return (await this.repoSelect().count()) > 0;
  }

  /** Fill name + description (and optionally pick a repo) in one call. */
  async fill(opts: { name?: string; description: string; repository?: string }): Promise<void> {
    if (opts.name !== undefined) {
      await this.fillName(opts.name);
    }
    if (opts.repository !== undefined && (await this.hasRepoSelect())) {
      await this.selectRepository(opts.repository);
    }
    await this.fillDescription(opts.description);
  }

  /** Click "Add to Dashboard" to start the workflow. */
  async submit(): Promise<void> {
    await this.root.getByRole('button', { name: PASTE_MODAL_LABELS.submit }).click();
  }

  /** Click "Cancel" to dismiss without starting. */
  async cancel(): Promise<void> {
    await this.root.getByRole('button', { name: PASTE_MODAL_LABELS.cancel }).click();
  }
}

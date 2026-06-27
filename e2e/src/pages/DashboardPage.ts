import { expect, type Page } from '@playwright/test';
import { PasteDescriptionModal } from './PasteDescriptionModal.js';
import { WorkItemCard } from './WorkItemCard.js';

/** Visible strings for the dashboard add-item controls (English `dashboard.json`). */
export const DASHBOARD_LABELS = {
  /** The "+" tile shown after the grid when items exist. */
  addTile: '+',
  /** The empty-state add button shown when the grid is empty. */
  newItem: '+ New Item',
} as const;

/**
 * Page Object for the dashboard / work-item grid (`WorkflowGrid.tsx`). Owns the
 * add-item entry points and hands out per-card Page Objects ({@link WorkItemCard})
 * and the manual-add modal ({@link PasteDescriptionModal}).
 */
export class DashboardPage {
  readonly page: Page;

  constructor(page: Page) {
    this.page = page;
  }

  /** Navigate to the dashboard root (assumes an authenticated session). */
  async goto(): Promise<void> {
    await this.page.goto('/');
    await this.dismissInfoModal();
  }

  /**
   * Dismiss the "No Ticketing System" info modal shown on the dashboard when
   * `ticketing_system = none`. Its backdrop intercepts clicks on the add-item
   * controls, so close it first. No-op when absent.
   */
  async dismissInfoModal(): Promise<void> {
    await this.page
      .getByRole('button', { name: 'Got it' })
      .click({ timeout: 5_000 })
      .catch(() => undefined);
  }

  /**
   * Open the manual "add work item" modal. Clicks the grid **+** tile when work
   * items already exist, or the empty-state "+ New Item" button otherwise.
   * Returns the modal Page Object (`ticketing_system = none` → paste modal).
   */
  async openAddModal(): Promise<PasteDescriptionModal> {
    const tile = this.page.getByRole('button', { name: DASHBOARD_LABELS.addTile, exact: true });
    const emptyStateButton = this.page.getByRole('button', { name: DASHBOARD_LABELS.newItem });
    if (await tile.count()) {
      await tile.first().click();
    } else {
      await emptyStateButton.click();
    }
    const modal = new PasteDescriptionModal(this.page);
    await modal.expectOpen();
    return modal;
  }

  /**
   * Drive the full paste-description flow: open the modal, fill it, submit. The
   * card appears asynchronously once the workflow is created — await
   * {@link WorkItemCard.waitFor} on the returned card.
   */
  async addWorkItem(opts: {
    name?: string;
    description: string;
    repository?: string;
  }): Promise<void> {
    const modal = await this.openAddModal();
    await modal.fill(opts);
    await modal.submit();
  }

  /** A Page Object scoped to the card with the given `ticket_key`. */
  card(ticketKey: string): WorkItemCard {
    return new WorkItemCard(this.page, ticketKey);
  }

  /** Assert a card with the given `ticket_key` is present. */
  async expectCard(ticketKey: string, timeoutMs = 30_000): Promise<WorkItemCard> {
    const card = this.card(ticketKey);
    await expect(card.locator()).toBeVisible({ timeout: timeoutMs });
    return card;
  }
}

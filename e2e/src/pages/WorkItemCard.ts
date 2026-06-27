import { expect, type Locator, type Page } from '@playwright/test';

/**
 * Visible strings on a work-item card. Centralized so specs never hard-code a
 * label (English i18n `dashboard.json` `runCommands.*` / `editorMenu.*` /
 * `card.*` / `workflowDefButtons.*`).
 */
export const CARD_LABELS = {
  showDetails: 'Show details',
  showConsoleOutput: 'Show console output',
  editor: {
    idleTitle: 'Open editor',
    runningTitle: 'Editor (open)',
    openInBrowser: 'Open in browser',
    stop: 'Stop editor',
  },
  terminal: {
    idleTitle: 'Open terminal',
    runningTitle: 'Terminal (open)',
    openInBrowser: 'Open in browser',
    stop: 'Stop terminal',
  },
  /** `runCommands.run` = "Run {{name}}", `runCommands.stop` = "Stop {{name}}". */
  runCommand: {
    run: (name: string) => `Run ${name}`,
    stop: (name: string) => `Stop ${name}`,
    copy: 'Copy',
    open: 'Open',
  },
} as const;

/**
 * Page Object scoped to a single IssueCard (`.work-item-card`), located by its
 * `ticket_key`. Wraps the interactive surface the Part-B specs drive: the flow
 * buttons (`WorkflowDefButtons`), run-command Run/Stop/Copy/Open
 * (`RunCommands.tsx`), and the editor / terminal menus
 * (`IssueCardFooter` → `EditorTerminalMenu.tsx`).
 *
 * Obtain one via {@link DashboardPage.card}.
 */
export class WorkItemCard {
  readonly page: Page;
  readonly ticketKey: string;
  private readonly root: Locator;

  constructor(page: Page, ticketKey: string) {
    this.page = page;
    this.ticketKey = ticketKey;
    this.root = page.locator('.work-item-card').filter({
      has: page.getByText(ticketKey, { exact: true }),
    });
  }

  /** The card root locator (for bespoke assertions in a spec when unavoidable). */
  locator(): Locator {
    return this.root;
  }

  /** Wait for the card to be visible on the dashboard. */
  async waitFor(timeoutMs = 30_000): Promise<void> {
    await this.root.waitFor({ state: 'visible', timeout: timeoutMs });
  }

  // --- Flow definition buttons ---------------------------------------------

  /**
   * Click a flow button by its display name to run that definition. When the
   * button row overflows it collapses behind a single "Start flow" button; this
   * helper handles only the inline case (the Part-B fixture has one short flow).
   */
  async runFlow(flowName: string): Promise<void> {
    await this.root.getByRole('button', { name: flowName, exact: true }).click();
  }

  /**
   * The success badge a flow definition turns into once its run is `completed`
   * (`WorkflowDefButtons` renders a non-interactive `.wf-btn-success` span with
   * a check + the flow name). Scoped to `:visible` because the component also
   * renders an off-screen (invisible) measurer copy of every button.
   */
  completedFlowBadge(flowName: string): Locator {
    return this.root.locator('.wf-btn-success:visible').filter({ hasText: flowName });
  }

  /** The error badge/button a flow turns into when its run ends in error. */
  erroredFlowBadge(flowName: string): Locator {
    return this.root.locator('.wf-btn-danger:visible').filter({ hasText: flowName });
  }

  // --- Run-commands (dev servers) ------------------------------------------

  /** The Run button for run-command `name` (visible when it is stopped). */
  runCommandButton(name: string): Locator {
    return this.root.getByRole('button', { name: CARD_LABELS.runCommand.run(name) });
  }

  /** The Stop button for run-command `name` (visible while it is running). */
  stopCommandButton(name: string): Locator {
    return this.root.getByRole('button', { name: CARD_LABELS.runCommand.stop(name) });
  }

  /** Click Run for run-command `name`. */
  async runCommand(name: string): Promise<void> {
    await this.runCommandButton(name).click();
  }

  /** Click Stop for run-command `name`. */
  async stopCommand(name: string): Promise<void> {
    await this.stopCommandButton(name).click();
  }

  /** The "Open" link for a running run-command (present once its port forwards). */
  runCommandOpenLink(): Locator {
    return this.root.getByRole('link', { name: CARD_LABELS.runCommand.open });
  }

  /** The "Copy" button for a running run-command's forwarded URL. */
  runCommandCopyButton(): Locator {
    return this.root.getByRole('button', { name: CARD_LABELS.runCommand.copy });
  }

  /** Resolve the `href` of the run-command "Open" link (the proxied `/s/…` URL). */
  async runCommandProxiedUrl(): Promise<string> {
    const link = this.runCommandOpenLink();
    await expect(link).toBeVisible();
    const href = await link.getAttribute('href');
    if (!href) {
      throw new Error('run-command Open link has no href');
    }
    return href;
  }

  // --- Editor / terminal menus ---------------------------------------------

  private editorToggle(): Locator {
    return this.root.getByTitle(
      new RegExp(`${CARD_LABELS.editor.idleTitle}|${escapeRegExp(CARD_LABELS.editor.runningTitle)}`),
    );
  }

  private terminalToggle(): Locator {
    return this.root.getByTitle(
      new RegExp(`${CARD_LABELS.terminal.idleTitle}|${escapeRegExp(CARD_LABELS.terminal.runningTitle)}`),
    );
  }

  /** Click the editor icon — starts the open-editor flow when idle, else toggles the menu. */
  async clickEditor(): Promise<void> {
    await this.editorToggle().click();
  }

  /** Click the terminal icon — starts the open-terminal flow when idle, else toggles the menu. */
  async clickTerminal(): Promise<void> {
    await this.terminalToggle().click();
  }

  /** True once an editor container is running (icon shows the "open" title). */
  async isEditorRunning(): Promise<boolean> {
    return (await this.root.getByTitle(CARD_LABELS.editor.runningTitle).count()) > 0;
  }

  /** True once a terminal container is running. */
  async isTerminalRunning(): Promise<boolean> {
    return (await this.root.getByTitle(CARD_LABELS.terminal.runningTitle).count()) > 0;
  }

  /** Open the running-editor menu and resolve the "Open in browser" `href` (proxied URL). */
  async editorProxiedUrl(): Promise<string> {
    await this.root.getByTitle(CARD_LABELS.editor.runningTitle).click();
    const link = this.root.getByRole('link', { name: CARD_LABELS.editor.openInBrowser });
    await expect(link).toBeVisible();
    const href = await link.getAttribute('href');
    if (!href) {
      throw new Error('editor "Open in browser" link has no href');
    }
    return href;
  }

  /** Open the running-terminal menu and resolve the "Open in browser" `href` (proxied URL). */
  async terminalProxiedUrl(): Promise<string> {
    await this.root.getByTitle(CARD_LABELS.terminal.runningTitle).click();
    const link = this.root.getByRole('link', { name: CARD_LABELS.terminal.openInBrowser });
    await expect(link).toBeVisible();
    const href = await link.getAttribute('href');
    if (!href) {
      throw new Error('terminal "Open in browser" link has no href');
    }
    return href;
  }
}

/** Escape a literal string for safe interpolation into a RegExp. */
function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

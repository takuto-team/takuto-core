import { expect, type Locator, type Page } from '@playwright/test';

/**
 * Visible strings for the Config page (`/config.html`) tabs and the two
 * per-repo settings editors driven by Part B: the Workflows (flows) editor and
 * the Repository Settings (init/run commands) editor. English i18n
 * (`common.json tabs.*`, `config.json`, `modals.json stepEditor.*`).
 */
export const CONFIG_LABELS = {
  tabs: {
    workflows: 'Workflows',
    repositorySettings: 'Repository Settings',
  },
  flows: {
    add: '+ Add workflow',
    untitledName: 'Untitled workflow',
    create: 'Create workflow',
  },
  step: {
    untitledName: 'Untitled step',
    promptPlaceholder: 'Text sent verbatim to the agent for this step.',
  },
  worktree: {
    addInit: '+ Add command',
    initPlaceholder: 'e.g. npm install --legacy-peer-deps',
    addRun: '+ Add run command',
    runNamePlaceholder: 'e.g. Dashboard UI',
    runCommandPlaceholder: 'e.g. cd ui && npm run dev',
    saved: 'Commands saved.',
  },
  /** `SettingsFooter` Save button (`config.json actions.saveChanges`). */
  saveChanges: 'Save changes',
} as const;

/**
 * Page Object for the settings page (`/config.html`). Owns tab navigation, the
 * single page-level Save footer, and helpers to create a one-step flow and to
 * set the per-repo init/run commands — all through the real form controls.
 */
export class ConfigPage {
  readonly page: Page;

  constructor(page: Page) {
    this.page = page;
  }

  async goto(): Promise<void> {
    await this.page.goto('/config.html');
  }

  /** Click a top tab by its visible label. */
  async openTab(label: string): Promise<void> {
    await this.page.getByRole('button', { name: label, exact: true }).click();
  }

  /** Click the page-level "Save changes" footer button and wait for it to settle. */
  async saveChanges(): Promise<void> {
    await this.page.getByRole('button', { name: CONFIG_LABELS.saveChanges }).click();
  }

  /**
   * Enter edit mode on an `EditableName` (a click-to-rename span) and type a
   * value. The component focuses its inline `<input>` on click; we fill the
   * focused input and commit with Enter. Never sends Escape (that reverts).
   */
  private async typeEditableName(span: Locator, value: string): Promise<void> {
    await span.click();
    const input = this.page.locator('input:focus');
    await input.fill(value);
    await input.press('Enter');
  }

  // --- Workflows tab --------------------------------------------------------

  /**
   * Create a single-step flow named `flowName` whose one step has `stepName` +
   * `prompt`, via the Workflows tab editor. Assumes a single repo (the sidebar
   * auto-selects it). Leaves the tab on the saved list.
   */
  async createSingleStepFlow(opts: {
    flowName: string;
    stepName: string;
    prompt: string;
  }): Promise<void> {
    await this.openTab(CONFIG_LABELS.tabs.workflows);
    await this.page.getByRole('button', { name: CONFIG_LABELS.flows.add }).first().click();

    // Draft card: name (EditableName) + one blank StepEditor.
    await this.typeEditableName(
      this.page.getByRole('button', { name: CONFIG_LABELS.flows.untitledName }),
      opts.flowName,
    );
    await this.typeEditableName(
      this.page.getByRole('button', { name: CONFIG_LABELS.step.untitledName }),
      opts.stepName,
    );
    await this.page
      .getByPlaceholder(CONFIG_LABELS.step.promptPlaceholder)
      .fill(opts.prompt);

    const create = this.page.getByRole('button', { name: CONFIG_LABELS.flows.create });
    await expect(create).toBeEnabled();
    await create.click();

    // On success the editor closes (the draft "Create workflow" button is gone)
    // and the saved flow card appears with its name.
    await expect(create).toHaveCount(0, { timeout: 30_000 });
    await expect(
      this.page.getByRole('button', { name: opts.flowName, exact: true }),
    ).toBeVisible();
  }

  // --- Repository Settings tab (init + run commands) ------------------------

  /**
   * Set the per-repo init commands + a single run command, then Save. Assumes a
   * single repo (auto-selected in the sidebar).
   */
  async setWorktreeCommands(opts: {
    initCommand: string;
    runName: string;
    runCommand: string;
  }): Promise<void> {
    await this.openTab(CONFIG_LABELS.tabs.repositorySettings);

    // Init command.
    await this.page.getByRole('button', { name: CONFIG_LABELS.worktree.addInit }).click();
    await this.page
      .getByPlaceholder(CONFIG_LABELS.worktree.initPlaceholder)
      .last()
      .fill(opts.initCommand);

    // Run command (name + command).
    await this.page.getByRole('button', { name: CONFIG_LABELS.worktree.addRun }).click();
    await this.page
      .getByPlaceholder(CONFIG_LABELS.worktree.runNamePlaceholder)
      .last()
      .fill(opts.runName);
    await this.page
      .getByPlaceholder(CONFIG_LABELS.worktree.runCommandPlaceholder)
      .last()
      .fill(opts.runCommand);

    await this.saveChanges();
    await expect(this.page.getByText(CONFIG_LABELS.worktree.saved)).toBeVisible({
      timeout: 30_000,
    });
  }
}

import { test, expect } from '../src/fixtures/admin.fixture.js';
import { GitHubStep } from '../src/pages/GitHubStep.js';
import { ProviderStep } from '../src/pages/ProviderStep.js';
import { TicketingStep } from '../src/pages/TicketingStep.js';
import { WorkflowsStep } from '../src/pages/WorkflowsStep.js';
import { JIRA_INPUT, PROVIDER_INPUTS } from '../src/data/matrix.js';
import { advanceToStep, fillStable } from './support/walk.js';

/**
 * Field-validation rules. These are client/server logic that does not vary by
 * provider or ticketing system, so they run once per backend project rather than
 * across the cartesian. Each case reaches the step under test, supplies the
 * offending input, and asserts the wizard refuses to advance (staying on the
 * step) with the documented inline error or toast.
 */
test.describe('onboarding wizard — validation', () => {
  test('git base branch is required', async ({ page, admin }) => {
    const wizard = admin.wizard;
    const github = new GitHubStep(page);

    await advanceToStep(page, 1);
    await fillStable(github.baseBranchInput(), '');
    await wizard.saveAndContinue();

    await wizard.expectStep(1);
    await github.expectBaseBranchRequired();
  });

  test('git remote is required', async ({ page, admin }) => {
    const wizard = admin.wizard;
    const github = new GitHubStep(page);

    await advanceToStep(page, 1);
    await fillStable(github.remoteInput(), '');
    await wizard.saveAndContinue();

    await wizard.expectStep(1);
    await github.expectRemoteRequired();
  });

  test('cursor disables the base URL field', async ({ page, admin }) => {
    const wizard = admin.wizard;
    const provider = new ProviderStep(page);

    await advanceToStep(page, 3);
    await wizard.expectStep(3);
    await provider.selectProvider('cursor');

    await expect(provider.baseUrlInput()).toBeDisabled();
    await expect(provider.baseUrlInput()).toHaveValue('');
  });

  test('opencode requires base URL and model (blocks Continue)', async ({ page, admin }) => {
    const wizard = admin.wizard;
    const provider = new ProviderStep(page);

    await advanceToStep(page, 3);
    await provider.selectProvider('opencode');
    await provider.fillBaseUrl('');
    await provider.fillModel('');
    await wizard.saveAndContinue();

    // Server rejects with `opencode_base_url_required`; the wizard surfaces the
    // code in an error toast and stays on step 3.
    await expect(page.getByText(/opencode_base_url_required/)).toBeVisible();
    await wizard.expectStep(3);

    // Repair the shared deployment config. The rejected PUT mutates the
    // server's in-memory config (provider→opencode, blank base_url) before
    // validation fails, and that mutation is NOT rolled back — so leave a valid
    // config behind, or every later config write on this worker stack fails
    // whole-config validation. Supplying valid opencode values and advancing
    // re-validates cleanly.
    await provider.fillBaseUrl(PROVIDER_INPUTS.opencode.baseUrl);
    await provider.fillModel(PROVIDER_INPUTS.opencode.model);
    await wizard.saveAndContinue();
    await wizard.expectStep(4);
  });

  test('jira partial form blocks Continue', async ({ page, admin }) => {
    const wizard = admin.wizard;
    const ticketing = new TicketingStep(page);

    await advanceToStep(page, 4);
    await ticketing.selectSystem('jira');
    // Only the site filled (1 of 3) → partial form.
    await ticketing.jiraSiteInput().fill(JIRA_INPUT.site);
    await wizard.saveAndContinue();

    await expect(page.getByText(/Fill in the Jira site/)).toBeVisible();
    await wizard.expectStep(4);
  });

  test('step timeout must be positive (blocks Finish)', async ({ page, admin }) => {
    const wizard = admin.wizard;
    const workflows = new WorkflowsStep(page);

    await advanceToStep(page, 5);
    await workflows.fillStepTimeout('0');
    await wizard.finish({ expectNavigation: false });

    await wizard.expectStep(5);
    await workflows.expectStepTimeoutInvalid();
  });
});

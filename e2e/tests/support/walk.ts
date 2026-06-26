import { expect, type Locator, type Page } from '@playwright/test';
import { OnboardingWizard } from '../../src/pages/OnboardingWizard.js';
import { GitHubStep } from '../../src/pages/GitHubStep.js';
import { ProviderStep } from '../../src/pages/ProviderStep.js';
import { TicketingStep } from '../../src/pages/TicketingStep.js';
import { WorkflowsStep } from '../../src/pages/WorkflowsStep.js';
import {
  GIT_INPUT,
  PROVIDER_INPUTS,
  STEP_TIMEOUT_SECS,
  type OnboardingCase,
} from '../../src/data/matrix.js';

/**
 * Shared wizard-driving routines for the onboarding specs. They orchestrate the
 * delivered Page Objects only — no raw selectors — so the specs stay declarative.
 *
 * This file lives under `tests/` for proximity to its callers but is NOT a spec:
 * Playwright's default `testMatch` only collects `*.spec.ts` / `*.test.ts`, so it
 * is loaded purely as an imported helper module.
 */

/**
 * Fill `locator` with `value` and confirm it sticks.
 *
 * The wizard's form hooks seed their fields from `GET /api/config` in a
 * post-mount effect that fires exactly once. A value typed in the brief window
 * before that effect runs is clobbered by the seed (the field reverts to its
 * persisted/default value). Filling and then asserting the value — retrying via
 * `toPass` — rides out that one-time clobber without an arbitrary sleep: once the
 * seed has run, the field keeps whatever we type. Used for the step-1 git
 * inputs, the first fields the suite touches after the wizard mounts.
 */
export async function fillStable(locator: Locator, value: string): Promise<void> {
  await expect(async () => {
    await locator.fill(value);
    await expect(locator).toHaveValue(value, { timeout: 1500 });
  }).toPass({ timeout: 20_000 });
}

/**
 * Drive the wizard from step 1 to a successful Finish for one provider ×
 * ticketing case, entering only the inputs that persist offline:
 *  - git base branch + remote (required),
 *  - the provider sub-table + a dummy AI api_key (shape-validated server-side,
 *    so it stores without a live provider round-trip),
 *  - the ticketing system selection,
 *  - the step-5 timeout.
 *
 * The GitHub PAT and Jira credential are deliberately left blank: both are
 * validated live (against GitHub / Atlassian) on save, so a dummy value would
 * fail validation and block "Save and Continue". Selecting Jira with an empty
 * credential form still persists `general.ticketing_system = "jira"` and advances.
 */
export async function completeWizard(page: Page, c: OnboardingCase): Promise<void> {
  const wizard = new OnboardingWizard(page);
  const github = new GitHubStep(page);
  const provider = new ProviderStep(page);
  const ticketing = new TicketingStep(page);
  const workflows = new WorkflowsStep(page);
  const input = PROVIDER_INPUTS[c.provider];

  // Step 1 — Git & GitHub
  await wizard.expectStep(1);
  await fillStable(github.baseBranchInput(), GIT_INPUT.baseBranch);
  await fillStable(github.remoteInput(), GIT_INPUT.remote);
  await wizard.saveAndContinue();

  // Step 2 — Repositories (nothing to persist on Continue)
  await wizard.expectStep(2);
  await wizard.saveAndContinue();

  // Step 3 — AI provider
  await wizard.expectStep(3);
  await provider.selectProvider(c.provider);
  if (c.provider !== 'cursor') {
    // Set the base URL explicitly — empty for vendor-default providers — so a
    // value seeded from a previous case's provider can never leak into this one.
    await provider.fillBaseUrl(input.baseUrl);
  }
  await provider.fillModel(input.model);
  await provider.fillExtraArgs(input.extraArgs);
  // The AI key is intentionally NOT entered here. On a worker stack reused
  // across cases a provider may already hold a stored key, which renders the
  // panel "connected" with a masked, non-fillable field — driving it would be
  // flaky and adds nothing to the config assertions. Encrypted-credential
  // persistence is proven directly via the API in the persistence spec.
  await wizard.saveAndContinue();

  // Step 4 — Ticketing (system selection only; live-validated creds left blank)
  await wizard.expectStep(4);
  await ticketing.selectSystem(c.ticketing);
  await wizard.saveAndContinue();

  // Step 5 — Workflows
  await wizard.expectStep(5);
  await workflows.fillStepTimeout(STEP_TIMEOUT_SECS);
  await wizard.finish();
}

/**
 * Advance the wizard to `target` (1-5) entering only the minimum valid inputs to
 * clear each preceding step, without finishing. Used by the validation specs to
 * reach the step under test. Providers/ticketing are left at safe defaults
 * (`claude`, `none`) that never trigger a live credential round-trip. The git
 * fields are always set (seed-stable) so a caller targeting step 1 can then
 * clear one field and assert the required-field validation.
 */
export async function advanceToStep(page: Page, target: number): Promise<void> {
  const wizard = new OnboardingWizard(page);
  const github = new GitHubStep(page);
  const provider = new ProviderStep(page);
  const ticketing = new TicketingStep(page);

  await wizard.expectStep(1);
  await fillStable(github.baseBranchInput(), GIT_INPUT.baseBranch);
  await fillStable(github.remoteInput(), GIT_INPUT.remote);
  if (target <= 1) {
    return;
  }
  await wizard.saveAndContinue();

  await wizard.expectStep(2);
  if (target <= 2) {
    return;
  }
  await wizard.saveAndContinue();

  await wizard.expectStep(3);
  if (target <= 3) {
    return;
  }
  await provider.selectProvider('claude');
  await provider.fillBaseUrl('');
  await provider.fillModel(PROVIDER_INPUTS.claude.model);
  await wizard.saveAndContinue();

  await wizard.expectStep(4);
  if (target <= 4) {
    return;
  }
  await ticketing.selectSystem('none');
  await wizard.saveAndContinue();

  await wizard.expectStep(5);
}

/**
 * Walk all five steps accepting the seeded defaults — entering nothing — then
 * Finish. The skip path: the required fields (git base/remote, step timeout)
 * seed to valid values and every optional panel is left blank, so the wizard
 * completes on defaults alone and the server still writes a valid `config.toml`.
 */
export async function finishWithDefaults(page: Page): Promise<void> {
  const wizard = new OnboardingWizard(page);
  const github = new GitHubStep(page);

  await wizard.expectStep(1);
  // Wait for the step-1 body (and thus the config seed) to render before
  // accepting its defaults, so we never click Continue on an unseeded form.
  await expect(github.baseBranchInput()).toBeVisible();
  await wizard.saveAndContinue();

  await wizard.expectStep(2);
  await wizard.saveAndContinue();

  await wizard.expectStep(3);
  await wizard.saveAndContinue();

  await wizard.expectStep(4);
  await wizard.saveAndContinue();

  await wizard.expectStep(5);
  await wizard.finish();
}

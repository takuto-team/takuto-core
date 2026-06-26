import { type Page } from '@playwright/test';

/**
 * Step 2 — Repositories (`MyRepositoriesTab`). This step exposes no `#onb-*`
 * ids: adds/removes persist via the component's own buttons, and the set of
 * addable repositories is dictated by the deployment's GitHub App / PAT scope
 * (there is no free-form URL entry). Nothing is persisted on "Save and
 * Continue", so the step is fully skippable — every acceptance case advances
 * straight through it via the base wizard's `saveAndContinue()`.
 *
 * The Page Object is intentionally thin: it holds the page for symmetry with the
 * other steps. Driving real repository adds requires live GitHub scope the
 * ephemeral stack does not have, so no fill/add helpers are exposed.
 */
export class RepositoriesStep {
  readonly page: Page;

  constructor(page: Page) {
    this.page = page;
  }
}

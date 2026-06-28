import { test, expect } from '../src/fixtures/admin.fixture.js';
import { ConfigPage } from '../src/pages/ConfigPage.js';

/**
 * The admin "AI Settings" tab shows the Provider Settings dropdown above the
 * per-user "My credentials" card. Selecting a different provider in the
 * dropdown must switch the credential card to that provider **immediately**,
 * before any Save — previously the card stayed on the old provider until the
 * admin saved the new active provider. Pure UI wiring, so it runs once.
 */
test.describe('AI Settings — credential card follows the provider dropdown', () => {
  test('switching the provider dropdown updates the credential card in realtime', async ({
    page,
    admin,
    backend,
  }) => {
    test.skip(backend !== 'sqlite', 'provider/credential UI wiring is backend-independent');
    test.setTimeout(120_000);

    // Finish onboarding so /config.html renders instead of redirecting.
    await admin.api.completeOnboarding();

    const config = new ConfigPage(page);
    await config.goto();
    await config.openTab('AI Settings');

    const select = page.locator('#provider-select');
    await expect(select).toBeVisible();

    // Each dropdown selection flips the "My credentials" card title to that
    // provider with no Save in between. Cycle through providers that differ
    // from each other so a stuck card can't pass by coincidence.
    for (const [value, label] of [
      ['cursor', 'Cursor'],
      ['opencode', 'OpenCode'],
      ['claude', 'Claude'],
    ] as const) {
      await select.selectOption(value);
      await expect(
        page.getByText(new RegExp(`AI provider .* ${label}`)),
      ).toBeVisible();
    }
  });

  test('Claude and Codex are annotated "(not fully tested)" in the dropdown', async ({
    page,
    admin,
    backend,
  }) => {
    test.skip(backend !== 'sqlite', 'dropdown labels are backend-independent');
    test.setTimeout(120_000);

    await admin.api.completeOnboarding();

    const config = new ConfigPage(page);
    await config.goto();
    await config.openTab('AI Settings');

    const select = page.locator('#provider-select');
    await expect(select).toBeVisible();

    // The annotation is scoped to the dropdown options for the two providers
    // we flag, and absent from the others.
    await expect(select.locator('option[value="claude"]')).toHaveText(/not fully tested/);
    await expect(select.locator('option[value="codex"]')).toHaveText(/not fully tested/);
    await expect(select.locator('option[value="cursor"]')).not.toHaveText(/not fully tested/);
    await expect(select.locator('option[value="opencode"]')).not.toHaveText(/not fully tested/);
  });
});

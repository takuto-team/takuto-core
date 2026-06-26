import { test, expect } from '../src/fixtures/stack.fixture.js';

/**
 * Harness smoke check: proves the ephemeral stack boots, serves the onboarding
 * wizard, and survives a container restart. Full wizard coverage lives in the
 * dedicated onboarding specs.
 */
test.describe('stack harness', () => {
  test('boots, serves onboarding, survives restart', async ({ stack, page }) => {
    const health = await page.request.get(`${stack.baseURL}/api/health`);
    expect(health.ok()).toBeTruthy();
    expect((await health.text()).trim().toLowerCase()).toBe('ok');

    const status = await page.request.get(`${stack.baseURL}/api/onboarding/status`);
    expect(status.ok()).toBeTruthy();

    await page.goto('/', { waitUntil: 'domcontentloaded' });
    await expect(page.locator('body')).toBeVisible();

    await stack.restart();

    const afterRestart = await page.request.get(`${stack.baseURL}/api/health`);
    expect(afterRestart.ok()).toBeTruthy();
    expect((await afterRestart.text()).trim().toLowerCase()).toBe('ok');
  });
});

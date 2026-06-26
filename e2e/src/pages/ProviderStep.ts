import { expect, type Locator, type Page } from '@playwright/test';
import type { ProviderId } from '../api/types.js';

/** Server validation code surfaced in a toast when opencode lacks a base URL. */
export const PROVIDER_ERRORS = {
  opencodeBaseUrlRequired: /opencode_base_url_required/,
} as const;

/**
 * Step 3 — AI provider. Wraps the provider `<select>` (`#onb-provider`), the
 * base URL / model / extra-args inputs, and the AI key panel (anchored on
 * `aria-labelledby="ai-card-title"`). The base URL field is disabled and forced
 * empty for `cursor`; `opencode` requires base URL + model (server-validated).
 */
export class ProviderStep {
  readonly page: Page;

  constructor(page: Page) {
    this.page = page;
  }

  providerSelect(): Locator {
    return this.page.locator('#onb-provider');
  }

  baseUrlInput(): Locator {
    return this.page.locator('#onb-base-url');
  }

  modelInput(): Locator {
    return this.page.locator('#onb-model');
  }

  extraArgsInput(): Locator {
    return this.page.locator('#onb-extra-args');
  }

  /** The dummy AI key field inside the provider credential card. */
  apiKeyInput(): Locator {
    return this.page.locator('section[aria-labelledby="ai-card-title"] input[type="password"]');
  }

  async selectProvider(provider: ProviderId): Promise<void> {
    await this.providerSelect().selectOption(provider);
  }

  async fillBaseUrl(value: string): Promise<void> {
    await this.baseUrlInput().fill(value);
  }

  async fillModel(value: string): Promise<void> {
    await this.modelInput().fill(value);
  }

  /** Set the extra-args textarea — one argument per line. */
  async fillExtraArgs(args: string[]): Promise<void> {
    await this.extraArgsInput().fill(args.join('\n'));
  }

  /** Paste a provider key into the AI key panel (persisted on Continue). */
  async fillApiKey(value: string): Promise<void> {
    await this.apiKeyInput().fill(value);
  }

  async getProvider(): Promise<string> {
    return this.providerSelect().inputValue();
  }

  async getBaseUrl(): Promise<string> {
    return this.baseUrlInput().inputValue();
  }

  async getModel(): Promise<string> {
    return this.modelInput().inputValue();
  }

  /** Whether the base URL field is disabled (true for `cursor`). */
  async isBaseUrlDisabled(): Promise<boolean> {
    return this.baseUrlInput().isDisabled();
  }

  /** Assert the opencode-base-URL-required error toast is shown. */
  async expectOpencodeBaseUrlRequiredError(): Promise<void> {
    await expect(this.page.getByText(PROVIDER_ERRORS.opencodeBaseUrlRequired)).toBeVisible();
  }
}

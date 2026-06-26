import { test as base, expect } from '@playwright/test';
import type { Backend } from '../docker/naming.js';
import { createStack, type TakutoStack, type DockerStack } from '../docker/stack.js';

export type { Backend } from '../docker/naming.js';
export type { TakutoStack } from '../docker/stack.js';

/**
 * Worker-scoped option set per Playwright project (one project per backend).
 * Specs and Page Objects never set this directly — it comes from the project.
 */
export interface StackWorkerOptions {
  /** Database backend for this worker, injected by the project's `use`. */
  backend: Backend;
}

/**
 * Worker-scoped fixtures every onboarding spec and Page Object receives.
 *
 * - `stack`  — the live ephemeral deployment (backend, baseURL, restart, exec).
 * - `baseURL` — Playwright's built-in option, overridden to point at `stack`,
 *               so `page.goto('/')` hits the running container with no setup.
 */
export interface StackWorkerFixtures {
  stack: TakutoStack;
}

export const test = base.extend<object, StackWorkerOptions & StackWorkerFixtures>({
  backend: ['sqlite', { option: true, scope: 'worker' }],

  stack: [
    async ({ backend }, use, workerInfo) => {
      const log = (msg: string): void => {
        console.log(`[stack:${backend}:w${workerInfo.workerIndex}] ${msg}`);
      };
      const stack: DockerStack = await createStack(backend, log);
      try {
        await use(stack);
      } finally {
        await stack.teardown();
      }
    },
    { scope: 'worker', timeout: 300_000 },
  ],

  // `baseURL` is a built-in test-scoped option; it may still depend on the
  // worker-scoped `stack`, so `page.goto('/')` targets the running container.
  baseURL: async ({ stack }, use) => {
    await use(stack.baseURL);
  },
});

export { expect };

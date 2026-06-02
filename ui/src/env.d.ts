// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

declare const __APP_VERSION__: string;

interface ImportMetaEnv {
  /**
   * When `"true"` at vite build/dev time, the per-user credential API client
   * routes through the in-memory mock layer (`src/api/mocks.ts`). Storybook
   * stories also flip this on at runtime via `setMocksEnabled(true)` so the
   * mock works regardless of the env var.
   */
  readonly VITE_USE_MOCKS?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

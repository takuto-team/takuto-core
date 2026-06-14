// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * The single fetch-error surfacing helper. Normalises an `unknown` error to a
 * message and routes it onto the fetch-error bus, where `QueryErrorToaster`
 * turns it into a visible toast. Used both by the QueryClient's
 * `QueryCache.onError` (read failures from `useQuery`) and by the handful of
 * raw `apiJson` call sites that aren't queries (modals, one-off fetches) so no
 * server failure is swallowed silently.
 */

import { emitFetchError } from "../api/fetchErrorBus";

export function surfaceError(err: unknown, context?: string): void {
  const message = err instanceof Error ? err.message : String(err);
  emitFetchError(context ? `${context}: ${message}` : message);
}

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Tiny pub/sub bridging non-React error sources (the TanStack Query
 * `QueryCache.onError`, which runs outside the component tree) to a React
 * listener that can surface them via the toast UI. This is how failed
 * `useQuery` reads stop being silent: every query error is emitted here and
 * `QueryErrorToaster` turns it into a visible toast.
 */

export type FetchErrorListener = (message: string) => void;

const listeners = new Set<FetchErrorListener>();

/** Subscribe to fetch-error notifications. Returns an unsubscribe function. */
export function onFetchError(listener: FetchErrorListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Notify all subscribers that a server fetch failed. No-op when none. */
export function emitFetchError(message: string): void {
  for (const listener of listeners) listener(message);
}

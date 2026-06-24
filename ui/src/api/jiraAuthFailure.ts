// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Tiny pub/sub bridging the low-level fetch wrapper (`http.ts`, non-React) to
 * the React global modal host (`JiraAuthFailedModalHost`).
 *
 * When ANY Jira request fails with the per-user credential-invalid code, the
 * wrapper calls `emitJiraAuthFailure()`; the host subscribes via
 * `onJiraAuthFailure` and shows ONE global modal. A module-level listener set
 * (rather than a `window` event) keeps it import-explicit and unit-testable.
 */

/**
 * The structured error `code` returned (HTTP 401, JSON body) when a user's
 * stored Jira credential is expired/revoked. Centralised here so the
 * interceptor and any future caller branch on one constant.
 */
export const JIRA_CREDENTIAL_INVALID_CODE = "jira_credential_invalid";

type Listener = () => void;

const listeners = new Set<Listener>();

/** Notify every subscriber that a Jira credential-invalid response was seen. */
export function emitJiraAuthFailure(): void {
  for (const listener of listeners) {
    listener();
  }
}

/** Subscribe to Jira auth-failure events. Returns an unsubscribe function. */
export function onJiraAuthFailure(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

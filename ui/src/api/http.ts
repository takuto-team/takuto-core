// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Low-level fetch wrappers shared by every per-domain API module under
 * `ui/src/api/`. Lives in its own file (split out of `client.ts`) so the
 * domain modules can import the four primitives without pulling the entire
 * credentials / agent-config / repositories surface.
 */

import { JIRA_CREDENTIAL_INVALID_CODE, emitJiraAuthFailure } from "./jiraAuthFailure";

/**
 * A 401 carrying this code is a per-user JIRA credential failure (expired /
 * revoked token), NOT an unauthenticated session: the session cookie is still
 * valid. We surface a global "update your Jira token" modal instead of bouncing
 * to login. Peeks a CLONE so the original body is still readable by the caller.
 */
async function isJiraCredentialInvalid(res: Response): Promise<boolean> {
  try {
    const body = (await res.clone().json()) as { code?: unknown } | null;
    return !!body && body.code === JIRA_CREDENTIAL_INVALID_CODE;
  } catch {
    // Non-JSON / empty body → not the typed Jira-credential failure.
    return false;
  }
}

/**
 * Fetch wrapper that includes session cookie credentials.
 * On 401, redirects to the login page — except for the per-user Jira
 * credential-invalid 401, which fires a global modal and is left for the caller
 * to handle as a normal error.
 */
export async function api(input: string, init: RequestInit = {}): Promise<Response> {
  const res = await fetch(input, { ...init, credentials: "same-origin" });
  if (res.status === 401) {
    // The Jira token is bad but the session is fine: notify the global modal
    // host and do NOT redirect to login.
    if (await isJiraCredentialInvalid(res)) {
      emitJiraAuthFailure();
      return res;
    }
    // Genuine auth failure: send the user to the login route — but never when
    // we're already there. Without this guard a single stray 401 can ping-pong
    // an already-authenticated user between the dashboard and /login.html (the
    // login route redirects authenticated users straight back).
    if (!window.location.pathname.startsWith("/login")) {
      const ret = encodeURIComponent(window.location.pathname + window.location.search);
      window.location.href = `/login.html?return=${ret}`;
    }
  }
  return res;
}

export async function apiJson<T>(input: string, init: RequestInit = {}): Promise<T> {
  const res = await api(input, init);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}

export async function apiPost(input: string, body?: unknown): Promise<Response> {
  return api(input, {
    method: "POST",
    headers: body ? { "Content-Type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
}

export async function apiPostJson<T>(input: string, body?: unknown): Promise<T> {
  const res = await apiPost(input, body);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}

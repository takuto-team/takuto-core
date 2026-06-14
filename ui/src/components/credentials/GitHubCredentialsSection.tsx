// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Per-user GitHub credentials section — visible to every authenticated user.
 *
 * Lives on its own "GitHub" tab of /config.html. Manages the caller's GitHub
 * PAT and the "Attribute commits to me" toggle. The deployment-level GitHub App
 * connection is reported via `/api/auth/status::github_mode`; the PAT is layered
 * on top per-user so commits/PRs can be attributed to the individual.
 *
 * Hard constraint (A3): the per-user toggle is **"Attribute commits to me"** —
 * NOT "Sign commits". v1 does not do GPG/SSH signing. Regression-guarded in
 * `GitHubCredentialsSection.test.tsx`.
 */

import { useCallback, useEffect, useState } from "react";
import {
  apiJson,
  fetchUserCredentials,
  patchGithubSettings,
  setGithubPat,
  UserCredentialsError,
} from "../../api/client";
import { useToast } from "../../hooks/useToast";
import type {
  AuthStatus,
  GithubAuthMode,
  UserCredentialsStatus,
} from "../../api/types";
import { GitHubCredentialPanel } from "./GitHubCredentialPanel";

export function GitHubCredentialsSection() {
  const { showToast } = useToast();
  const [creds, setCreds] = useState<UserCredentialsStatus | null>(null);
  const [auth, setAuth] = useState<AuthStatus | null>(null);
  // `initialLoading` gates the first paint only; save-triggered refetches keep
  // the panel mounted (see MyCredentialsSection for the full rationale).
  const [initialLoading, setInitialLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    const [c, a] = await Promise.all([
      fetchUserCredentials().catch(() => null),
      apiJson<AuthStatus>("/api/auth/status").catch(() => null),
    ]);
    setCreds(c);
    setAuth(a);
    setLoadError(c ? null : "Could not load your credentials.");
  }, []);

  useEffect(() => {
    let mounted = true;
    refresh().finally(() => {
      if (mounted) setInitialLoading(false);
    });
    return () => {
      mounted = false;
    };
  }, [refresh]);

  const handleSurfaceError = useCallback(
    (e: unknown, fallback: string) => {
      if (e instanceof UserCredentialsError) {
        if (e.code === "sso_authorization_required" && e.orgSsoUrl) {
          showToast(
            `GitHub SSO required. Authorize at ${e.orgSsoUrl} and try again.`,
            "error",
          );
          return;
        }
        showToast(`${e.message} (code: ${e.code})`, "error");
        return;
      }
      showToast(e instanceof Error ? e.message : fallback, "error");
    },
    [showToast],
  );

  return (
    // The GitHubCredentialPanel renders its own "GitHub" header + connection
    // pill, so this tab wrapper carries no duplicate heading — just the
    // load/error gate around the panel.
    <section aria-label="GitHub credentials" className="flex flex-col gap-3">
      {initialLoading && <p className="text-sm text-gray-500">Loading…</p>}
      {!initialLoading && loadError && (
        <p className="text-sm text-red-400">{loadError}</p>
      )}

      {!initialLoading && (
        <GitHubCredentialPanel
          github={creds?.github ?? null}
          authMode={auth?.github_mode as GithubAuthMode | undefined}
          onSavePat={async (pat, attribute) => {
            try {
              const next = await setGithubPat({
                pat,
                attribute_commits: attribute,
              });
              await refresh();
              showToast(
                `GitHub token saved — you're @${next.github?.login ?? "?"}.`,
                "success",
              );
            } catch (e: unknown) {
              handleSurfaceError(e, "Could not save your GitHub token.");
            }
          }}
          onToggleAttributeCommits={async (attribute) => {
            try {
              await patchGithubSettings({ attribute_commits: attribute });
              await refresh();
            } catch (e: unknown) {
              handleSurfaceError(e, "Could not update GitHub settings.");
            }
          }}
        />
      )}
    </section>
  );
}

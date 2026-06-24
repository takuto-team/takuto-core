// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Per-user GitHub credentials section — visible to every authenticated user.
 *
 * Lives on its own "GitHub" tab of /config.html. Manages the caller's GitHub
 * PAT. The deployment-level GitHub App connection is reported via
 * `/api/auth/status::github_mode`; the PAT is layered on top per-user. Commit
 * and PR attribution follows PAT presence: with a PAT, commits and PRs are
 * authored by the user; with the App only, they are authored by the bot.
 */

import { useCallback, useEffect, useRef, useState, type Ref } from "react";
import { useTranslation } from "react-i18next";
import {
  apiJson,
  fetchUserCredentials,
  setGithubPat,
  UserCredentialsError,
} from "../../api/client";
import { useToast } from "../../hooks/useToast";
import type {
  AuthStatus,
  GithubAuthMode,
  UserCredentialsStatus,
} from "../../api/types";
import {
  GitHubCredentialPanel,
  type GitHubCredentialPanelHandle,
} from "./GitHubCredentialPanel";

interface Props {
  /** Optional imperative handle, forwarded to the inner panel so the
   *  onboarding wizard can persist a typed PAT on "Continue". The Config-page
   *  usage omits it and is unaffected. */
  panelRef?: Ref<GitHubCredentialPanelHandle>;
  /** Reports a typed-but-unsaved PAT so a page-level Save can fold it in. */
  onDirtyChange?: (dirty: boolean) => void;
  /** When true, the panel's own Save button is hidden — the PAT is persisted by
   *  a single page-level Save (wizard / settings footer). Defaults to false. */
  deferSave?: boolean;
  /** Registers a save fn (persists a typed PAT) for the settings footer. */
  registerSave?: (save: () => Promise<boolean>) => void;
}

export function GitHubCredentialsSection({ panelRef, onDirtyChange, deferSave, registerSave }: Props = {}) {
  const { t } = useTranslation("credentials");
  const { showToast } = useToast();
  const [creds, setCreds] = useState<UserCredentialsStatus | null>(null);
  const [auth, setAuth] = useState<AuthStatus | null>(null);
  // `initialLoading` gates the first paint only; save-triggered refetches keep
  // the panel mounted (see MyCredentialsSection for the full rationale).
  const [initialLoading, setInitialLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  // Internal handle so the settings footer can persist a typed PAT. The wizard
  // passes its own `panelRef` (and no `registerSave`); the Config tab passes
  // `registerSave` (and no `panelRef`) — they are mutually exclusive, so we use
  // whichever ref is supplied rather than merging (no prop mutation).
  const innerRef = useRef<GitHubCredentialPanelHandle>(null);
  const effectiveRef = panelRef ?? innerRef;
  useEffect(() => {
    registerSave?.(async () => (innerRef.current ? innerRef.current.saveIfDirty() : true));
  }, [registerSave]);

  const refresh = useCallback(async () => {
    const [c, a] = await Promise.all([
      fetchUserCredentials().catch(() => null),
      apiJson<AuthStatus>("/api/auth/status").catch(() => null),
    ]);
    setCreds(c);
    setAuth(a);
    setLoadError(c ? null : t("loadError"));
  }, [t]);

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
            t("github.toast.ssoRequired", { url: e.orgSsoUrl }),
            "error",
          );
          return;
        }
        showToast(
          t("error.withCode", { message: e.message, code: e.code }),
          "error",
        );
        return;
      }
      showToast(e instanceof Error ? e.message : fallback, "error");
    },
    [showToast, t],
  );

  return (
    // The GitHubCredentialPanel renders its own "GitHub" header + connection
    // pill, so this tab wrapper carries no duplicate heading — just the
    // load/error gate around the panel.
    <section aria-label={t("github.sectionAria")} className="flex flex-col gap-3">
      {initialLoading && (
        <p className="text-sm text-gray-500">{t("loading")}</p>
      )}
      {!initialLoading && loadError && (
        <p className="text-sm text-red-400">{loadError}</p>
      )}

      {!initialLoading && (
        <GitHubCredentialPanel
          ref={effectiveRef}
          github={creds?.github ?? null}
          authMode={auth?.github_mode as GithubAuthMode | undefined}
          onDirtyChange={onDirtyChange}
          deferSave={deferSave}
          onSavePat={async (pat) => {
            try {
              const next = await setGithubPat({ pat, attribute_commits: true });
              await refresh();
              showToast(
                t("github.toast.saved", {
                  login: next.github?.login ?? "?",
                }),
                "success",
              );
              return true;
            } catch (e: unknown) {
              handleSurfaceError(e, t("github.toast.saveError"));
              return false;
            }
          }}
        />
      )}
    </section>
  );
}

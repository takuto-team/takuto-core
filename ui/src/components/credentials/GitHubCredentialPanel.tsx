// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * GitHub auth card — PAT paste + a per-case explanation of who commits and PRs
 * are attributed to (you when a PAT is present, the GitHub App bot otherwise).
 * Extracted from `MyCredentialsSection.tsx` (CODING_STANDARDS §3 one component
 * per file).
 */

import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useState,
} from "react";
import { Trans, useTranslation } from "react-i18next";
import { ConnectedStatusPill } from "../ConnectedStatusPill";
import { CredentialPasteField } from "../CredentialPasteField";
import type { GithubAuthMode, UserCredentialsStatus } from "../../api/types";
import { describeMode } from "./helpers";

interface GitHubCredentialPanelProps {
  github: UserCredentialsStatus["github"] | null;
  authMode: GithubAuthMode | undefined;
  /** Persist the entered PAT. Returns `true` on success, `false` on failure
   *  (the caller renders the error toast). */
  onSavePat: (pat: string) => Promise<boolean>;
  /** Reports `true` when a PAT is typed-but-unsaved, so a parent page-level
   *  Save can fold it in and gate its dirty state. */
  onDirtyChange?: (dirty: boolean) => void;
  /** When true, hide the panel's own Save button — the PAT is persisted by a
   *  single page-level Save that calls `saveIfDirty`. Defaults to false. */
  deferSave?: boolean;
}

/**
 * Imperative handle the onboarding wizard drives on "Continue" so the
 * currently-typed PAT is persisted as part of advancing the step, without
 * the user having to click the panel's own Validate & save button.
 */
export interface GitHubCredentialPanelHandle {
  /**
   * Submit the entered PAT if non-blank. A blank field is a no-op that
   * resolves `true` (the user is skipping / running as the shared GitHub App).
   * Resolves `false` only when a non-blank save fails.
   */
  saveIfDirty: () => Promise<boolean>;
}

export const GitHubCredentialPanel = forwardRef<
  GitHubCredentialPanelHandle,
  GitHubCredentialPanelProps
>(function GitHubCredentialPanel(
  { github, authMode, onSavePat, onDirtyChange, deferSave = false }: GitHubCredentialPanelProps,
  ref,
) {
  const { t } = useTranslation("credentials");
  const [pat, setPat] = useState("");
  const [saving, setSaving] = useState(false);

  // Report a typed-but-unsaved PAT so a parent page-level Save can fold it in.
  useEffect(() => {
    onDirtyChange?.(pat.trim() !== "");
  }, [pat, onDirtyChange]);

  // Wire-format note: presence of a PAT is derived from the parent's
  // `github` field being non-null. The backend never returns `has_pat` —
  // see `routes/credentials.rs::GithubCredentialStatus`. The effective mode
  // lives on `/api/auth/status::github_mode`.
  const hasPat = github != null;
  const effectiveMode = authMode ?? "missing";

  const submit = useCallback(async (): Promise<boolean> => {
    setSaving(true);
    try {
      const ok = await onSavePat(pat);
      if (ok) setPat("");
      return ok;
    } finally {
      setSaving(false);
    }
  }, [pat, onSavePat]);

  useImperativeHandle(
    ref,
    () => ({
      saveIfDirty: async () => {
        if (pat.trim() === "") return true;
        return submit();
      },
    }),
    [pat, submit],
  );

  // No "Remove PAT" button — PAT revocation happens on github.com; to wipe the
  // local row the user saves a different token.

  return (
    <section
      aria-labelledby="gh-card-title"
      className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-4"
    >
      <div className="flex items-center justify-between gap-3 flex-wrap">
        <h3 id="gh-card-title" className="text-lg font-semibold text-white">
          {t("github.title")}
        </h3>
        <ConnectedStatusPill
          state={hasPat ? "token" : effectiveMode === "app" ? "connected" : "missing"}
          label={describeMode(effectiveMode)}
        />
      </div>

      {effectiveMode === "app" && !hasPat && (
        <p className="text-sm text-gray-400">{t("github.appModeHint")}</p>
      )}
      {effectiveMode === "pat_only" && !hasPat && (
        <p className="text-sm text-amber-300">{t("github.patOnlyHint")}</p>
      )}
      {hasPat && (
        <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 text-sm text-gray-300">
          <p>
            <Trans
              i18nKey="github.loggedInAs"
              ns="credentials"
              values={{ login: github?.login ?? "?" }}
              components={{ strong: <strong className="text-gray-200" /> }}
            />
            {github?.scopes && github.scopes.length > 0 && (
              <>
                {" · "}
                {t("github.scopes")}: {github.scopes.join(", ")}
              </>
            )}
          </p>
          <p className="text-xs text-gray-500 mt-1">
            {t("github.attributedNote")}
          </p>
        </div>
      )}

      <CredentialPasteField
        label={hasPat ? t("github.patReplaceLabel") : t("github.patLabel")}
        value={pat}
        onChange={setPat}
        onSubmit={submit}
        hideSave={deferSave}
        saving={saving}
        masked={hasPat}
        placeholder={t("github.patPlaceholder")}
        saveLabel={hasPat ? t("actions.replace") : t("github.patSaveLabel")}
        helper={
          hasPat ? (
            t("github.patHelpSet")
          ) : (
          <Trans
            i18nKey="github.patHelp"
            ns="credentials"
            components={{
              code0: <code className="text-gray-400" />,
              code1: <code className="text-gray-400" />,
              ghLink: (
                <a
                  href="https://github.com/settings/tokens"
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-blue-400 hover:text-blue-300"
                  aria-label={t("github.patHelpAria")}
                />
              ),
            }}
          />
          )
        }
      />
    </section>
  );
});

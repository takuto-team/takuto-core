// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Ref } from "react";
import { useTranslation } from "react-i18next";
import { GitHubCredentialsSection } from "../../components/credentials/GitHubCredentialsSection";
import type { GitHubCredentialPanelHandle } from "../../components/credentials/GitHubCredentialPanel";

const GITHUB_APP_DOCS_URL = "https://takuto.io/docs/github-app/";

interface Props {
  githubAppConfigured: boolean;
  baseBranch: string;
  onChangeBaseBranch: (v: string) => void;
  remote: string;
  onChangeRemote: (v: string) => void;
  baseBranchInvalid: boolean;
  remoteInvalid: boolean;
  /** When `false`, the git inputs are read-only (the `[git]` section is
   *  admin-gated server-side). Defaults to `true`. */
  canEditGit?: boolean;
  /** Forwarded to the per-user PAT panel so the wizard persists a typed PAT
   *  when the user clicks Continue. */
  patPanelRef?: Ref<GitHubCredentialPanelHandle>;
  /** Reports a typed-but-unsaved PAT so the wizard gates "Save and Continue". */
  onPatDirtyChange?: (dirty: boolean) => void;
}

const INPUT_BASE = "w-full bg-gray-950 border rounded-lg px-3 py-2 text-sm";

export function GitHubStep({
  githubAppConfigured,
  baseBranch,
  onChangeBaseBranch,
  remote,
  onChangeRemote,
  baseBranchInvalid,
  remoteInvalid,
  canEditGit = true,
  patPanelRef,
  onPatDirtyChange,
}: Props) {
  const { t } = useTranslation("onboarding");
  const inputText = canEditGit ? "text-gray-200" : "text-gray-500 cursor-not-allowed";
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col gap-3">
        <div>
          <h3 className="text-sm font-semibold text-gray-300 mb-1">{t("git.heading")}</h3>
          <p className="text-xs text-gray-500 mb-3">{t("git.intro")}</p>
        </div>

        <div>
          <label htmlFor="onb-git-base-branch" className="block text-xs text-gray-400 mb-1">
            {t("git.baseBranch")}
          </label>
          <input
            id="onb-git-base-branch"
            type="text"
            value={baseBranch}
            onChange={(e) => onChangeBaseBranch(e.target.value)}
            placeholder={t("git.baseBranchPlaceholder")}
            disabled={!canEditGit}
            className={`${INPUT_BASE} ${inputText} ${
              baseBranchInvalid ? "border-red-500" : "border-gray-700"
            }`}
          />
          {baseBranchInvalid ? (
            <p className="text-xs text-red-400 mt-1">{t("git.baseBranchRequired")}</p>
          ) : (
            <p className="text-xs text-gray-500 mt-1">{t("git.baseBranchHint")}</p>
          )}
        </div>

        <div>
          <label htmlFor="onb-git-remote" className="block text-xs text-gray-400 mb-1">
            {t("git.remote")}
          </label>
          <input
            id="onb-git-remote"
            type="text"
            value={remote}
            onChange={(e) => onChangeRemote(e.target.value)}
            placeholder={t("git.remotePlaceholder")}
            disabled={!canEditGit}
            className={`${INPUT_BASE} ${inputText} ${
              remoteInvalid ? "border-red-500" : "border-gray-700"
            }`}
          />
          {remoteInvalid ? (
            <p className="text-xs text-red-400 mt-1">{t("git.remoteRequired")}</p>
          ) : (
            <p className="text-xs text-gray-500 mt-1">{t("git.remoteHint")}</p>
          )}
        </div>

        {!canEditGit && (
          <p className="text-xs text-gray-500">{t("git.adminOnly")}</p>
        )}
      </div>

      <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 text-sm text-gray-300">
        <p>
          {t("git.app.labelPrefix")}{" "}
          <strong>{githubAppConfigured ? t("git.app.configured") : t("git.app.notConfigured")}</strong>
        </p>
        <p className="text-xs text-gray-500 mt-2">{t("git.app.explainer")}</p>
        <a
          href={GITHUB_APP_DOCS_URL}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-block mt-3 text-sm text-blue-400 hover:text-blue-300"
          aria-label={t("git.app.setupLinkAria")}
        >
          {t("git.app.setupLink")}
        </a>
      </div>

      <div>
        <h3 className="text-sm font-semibold text-gray-300 mb-1">{t("git.pat.heading")}</h3>
        <p className="text-xs text-gray-500 mb-3">{t("git.pat.intro")}</p>
        <GitHubCredentialsSection
          panelRef={patPanelRef}
          onDirtyChange={onPatDirtyChange}
          deferSave
        />
      </div>
    </div>
  );
}

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { GitHubCredentialsSection } from "../../components/credentials/GitHubCredentialsSection";

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
}: Props) {
  const inputText = canEditGit ? "text-gray-200" : "text-gray-500 cursor-not-allowed";
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col gap-3">
        <div>
          <h3 className="text-sm font-semibold text-gray-300 mb-1">Git settings</h3>
          <p className="text-xs text-gray-500 mb-3">
            The branch Takuto checks out for each work item, and the git remote
            it pushes to.
          </p>
        </div>

        <div>
          <label htmlFor="onb-git-base-branch" className="block text-xs text-gray-400 mb-1">
            Base branch
          </label>
          <input
            id="onb-git-base-branch"
            type="text"
            value={baseBranch}
            onChange={(e) => onChangeBaseBranch(e.target.value)}
            placeholder="main"
            disabled={!canEditGit}
            className={`${INPUT_BASE} ${inputText} ${
              baseBranchInvalid ? "border-red-500" : "border-gray-700"
            }`}
          />
          {baseBranchInvalid ? (
            <p className="text-xs text-red-400 mt-1">Base branch is required.</p>
          ) : (
            <p className="text-xs text-gray-500 mt-1">
              The branch work-item branches are cut from. Usually "main" or
              "master".
            </p>
          )}
        </div>

        <div>
          <label htmlFor="onb-git-remote" className="block text-xs text-gray-400 mb-1">
            Remote
          </label>
          <input
            id="onb-git-remote"
            type="text"
            value={remote}
            onChange={(e) => onChangeRemote(e.target.value)}
            placeholder="origin"
            disabled={!canEditGit}
            className={`${INPUT_BASE} ${inputText} ${
              remoteInvalid ? "border-red-500" : "border-gray-700"
            }`}
          />
          {remoteInvalid ? (
            <p className="text-xs text-red-400 mt-1">Remote is required.</p>
          ) : (
            <p className="text-xs text-gray-500 mt-1">
              The git remote Takuto fetches from and pushes branches to.
            </p>
          )}
        </div>

        {!canEditGit && (
          <p className="text-xs text-gray-500">
            Only an admin can change the deployment's git settings.
          </p>
        )}
      </div>

      <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 text-sm text-gray-300">
        <p>
          GitHub App:{" "}
          <strong>{githubAppConfigured ? "configured" : "not configured"}</strong>
        </p>
        <p className="text-xs text-gray-500 mt-2">
          A shared GitHub App lets everyone on this deployment talk to GitHub
          without a personal token. Setting it up is optional — each teammate
          can instead bring their own personal access token below.
        </p>
        <a
          href={GITHUB_APP_DOCS_URL}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-block mt-3 text-sm text-blue-400 hover:text-blue-300"
          aria-label="Read the Takuto GitHub App setup guide (opens in a new tab)"
        >
          Set up a GitHub App →
        </a>
      </div>

      <div>
        <h3 className="text-sm font-semibold text-gray-300 mb-1">
          Your personal access token (optional)
        </h3>
        <p className="text-xs text-gray-500 mb-3">
          Add a PAT so commits, PRs, and review comments on your work items are
          attributed to you. Skip it to run as the shared GitHub App.
        </p>
        <GitHubCredentialsSection />
      </div>
    </div>
  );
}

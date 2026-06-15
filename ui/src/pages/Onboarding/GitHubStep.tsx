// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { GitHubCredentialsSection } from "../../components/credentials/GitHubCredentialsSection";

const GITHUB_APP_DOCS_URL = "https://takuto.io/docs/github-app/";

export function GitHubStep({ githubAppConfigured }: { githubAppConfigured: boolean }) {
  return (
    <div className="flex flex-col gap-4">
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

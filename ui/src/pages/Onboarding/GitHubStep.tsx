// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

export function GitHubStep({ githubAppConfigured }: { githubAppConfigured: boolean }) {
  return (
    <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 text-sm text-gray-300">
      <p>
        GitHub App:{" "}
        <strong>{githubAppConfigured ? "configured" : "not configured"}</strong>
      </p>
      <p className="text-xs text-gray-500 mt-2">
        Per-user GitHub personal access token capture is not yet
        available. For now, the wizard records that this step was seen
        but writes nothing.
      </p>
    </div>
  );
}

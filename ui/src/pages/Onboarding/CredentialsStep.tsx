// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { Link } from "react-router-dom";

export function CredentialsStep() {
  return (
    <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 text-sm text-gray-300">
      <p>
        <strong>Your credentials</strong>
      </p>
      <p className="text-xs text-gray-500 mt-2">
        Paste your AI provider key and (optionally) a GitHub personal access
        token from the per-user credential page. Skipping is fine — the
        deployment-default credentials in{" "}
        <code className="text-gray-400">takuto.env</code> stay in effect
        until you connect your own.
      </p>
      <Link
        to="/config.html?tab=ai"
        className="inline-block mt-3 text-sm text-blue-400 hover:text-blue-300"
      >
        Open AI Settings →
      </Link>
    </div>
  );
}

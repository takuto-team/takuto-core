// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * GitHub auth card — PAT paste + "Attribute commits" toggle. Extracted from
 * `MyCredentialsSection.tsx` (CODING_STANDARDS §3 one component per file).
 */

import { useEffect, useState } from "react";
import { ConnectedStatusPill } from "../ConnectedStatusPill";
import { CredentialPasteField } from "../CredentialPasteField";
import type { GithubAuthMode, UserCredentialsStatus } from "../../api/types";
import { describeMode } from "./helpers";

interface GitHubCredentialPanelProps {
  github: UserCredentialsStatus["github"] | null;
  authMode: GithubAuthMode | undefined;
  onSavePat: (pat: string, attributeCommits: boolean) => Promise<void>;
  onToggleAttributeCommits: (next: boolean) => Promise<void>;
}

export function GitHubCredentialPanel({
  github,
  authMode,
  onSavePat,
  onToggleAttributeCommits,
}: GitHubCredentialPanelProps) {
  const [pat, setPat] = useState("");
  const [attribute, setAttribute] = useState(github?.attribute_commits ?? true);
  const [saving, setSaving] = useState(false);
  // Keep local toggle in sync with server state on refresh.
  useEffect(() => {
    setAttribute(github?.attribute_commits ?? true);
  }, [github?.attribute_commits]);

  // Wire-format note: presence of a PAT is derived from the parent's
  // `github` field being non-null. The backend never returns `has_pat` —
  // see `routes/credentials.rs::GithubCredentialStatus`. The effective mode
  // lives on `/api/auth/status::github_mode`.
  const hasPat = github != null;
  const effectiveMode = authMode ?? "missing";

  const submit = async () => {
    setSaving(true);
    try {
      await onSavePat(pat, attribute);
      setPat("");
    } finally {
      setSaving(false);
    }
  };

  // Issue B from #31: no "Remove PAT" button. PAT revocation happens on
  // github.com; to wipe the local row the user saves a different token.

  const toggle = async (next: boolean) => {
    setAttribute(next);
    try {
      await onToggleAttributeCommits(next);
    } catch {
      // Revert on failure — parent surfaces the toast.
      setAttribute((v) => !v);
    }
  };

  return (
    <section
      aria-labelledby="gh-card-title"
      className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-4"
    >
      <div className="flex items-center justify-between gap-3 flex-wrap">
        <h3 id="gh-card-title" className="text-lg font-semibold text-white">
          GitHub
        </h3>
        <ConnectedStatusPill
          state={hasPat ? "connected" : effectiveMode === "app" ? "connected" : "missing"}
          label={describeMode(effectiveMode)}
        />
      </div>

      {effectiveMode === "app" && !hasPat && (
        <p className="text-sm text-gray-400">
          Takuto is using its GitHub App. Workflows run as the bot. Add a
          personal access token below if you want commits and PRs attributed
          to you.
        </p>
      )}
      {effectiveMode === "pat_only" && !hasPat && (
        <p className="text-sm text-amber-300">
          No shared GitHub App is configured. Takuto can only talk to GitHub
          via a personal access token — without one, GitHub-touching workflows
          won't start.
        </p>
      )}
      {hasPat && (
        <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 text-sm text-gray-300">
          <p>
            Logged in as{" "}
            <strong className="text-gray-200">@{github?.login ?? "?"}</strong>
            {github?.scopes && github.scopes.length > 0 && (
              <>
                {" · "}Scopes: {github.scopes.join(", ")}
              </>
            )}
          </p>
          <p className="text-xs text-gray-500 mt-1">
            Your commits, PRs, and PR comments are attributed to you. The
            GitHub App handles read-only API calls.
          </p>
        </div>
      )}

      {/* A3 regression guard: this toggle is **"Attribute commits to me"** —
          NOT "Sign commits". v1 does NOT GPG/SSH-sign. */}
      <div className="flex items-start gap-2 bg-gray-950/40 border border-gray-800 rounded-lg p-3">
        <input
          id="attribute-commits-toggle"
          type="checkbox"
          checked={attribute}
          disabled={!hasPat || saving}
          onChange={(e) => void toggle(e.target.checked)}
          className="mt-1 accent-blue-500"
        />
        <label
          htmlFor="attribute-commits-toggle"
          className="text-sm text-gray-300"
        >
          Attribute commits to me
          <p className="text-xs text-gray-500 mt-0.5">
            Your GitHub username and email will appear as the author on
            commits, PRs, and review comments. Cryptographic signing is a v2
            feature.
          </p>
        </label>
      </div>

      <CredentialPasteField
        label={hasPat ? "Replace personal access token" : "Personal access token"}
        value={pat}
        onChange={setPat}
        onSubmit={submit}
        saving={saving}
        placeholder="ghp_…"
        saveLabel={hasPat ? "Replace" : "Validate & save"}
        helper={
          <>
            Required scopes: <code className="text-gray-400">repo</code>{" "}
            (classic) or{" "}
            <code className="text-gray-400">
              contents:write + pull_requests:write + issues:read
            </code>{" "}
            (fine-grained).{" "}
            <a
              href="https://github.com/settings/tokens"
              target="_blank"
              rel="noopener noreferrer"
              className="text-blue-400 hover:text-blue-300"
              aria-label="Open GitHub PAT creation page (opens in a new tab)"
            >
              Help me create one →
            </a>
          </>
        }
      />
    </section>
  );
}

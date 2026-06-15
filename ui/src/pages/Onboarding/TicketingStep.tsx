// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { TicketingSystemId, UserJiraCredentialStatus } from "../../api/types";

const TICKETING_OPTIONS: { id: TicketingSystemId; label: string; hint: string }[] = [
  {
    id: "none",
    label: "None",
    hint: "No ticketing integration. Start work items manually from the dashboard.",
  },
  {
    id: "github",
    label: "GitHub",
    hint: "Poll open GitHub Issues from the repo's remote. No token here — connect a PAT or GitHub App in the GitHub step.",
  },
  {
    id: "jira",
    label: "Jira",
    hint: "Poll Jira for To Do tickets. Add your Atlassian site, email, and API token below.",
  },
];

interface Props {
  system: TicketingSystemId;
  onChangeSystem: (s: TicketingSystemId) => void;
  site: string;
  onChangeSite: (v: string) => void;
  email: string;
  onChangeEmail: (v: string) => void;
  token: string;
  onChangeToken: (v: string) => void;
  connected: UserJiraCredentialStatus | null;
  /** When `false`, the system selector is read-only (the deployment-wide
   *  ticketing system is admin-gated). Defaults to `true`. */
  canEditSystem?: boolean;
}

export function TicketingStep({
  system,
  onChangeSystem,
  site,
  onChangeSite,
  email,
  onChangeEmail,
  token,
  onChangeToken,
  connected,
  canEditSystem = true,
}: Props) {
  const activeHint = TICKETING_OPTIONS.find((o) => o.id === system)?.hint ?? "";

  return (
    <div className="flex flex-col gap-4">
      <div>
        <label htmlFor="onb-ticketing" className="block text-xs text-gray-400 mb-1">
          Ticketing system
        </label>
        <select
          id="onb-ticketing"
          value={system}
          onChange={(e) => onChangeSystem(e.target.value as TicketingSystemId)}
          disabled={!canEditSystem}
          className={`w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm ${
            canEditSystem ? "text-gray-200" : "text-gray-500 cursor-not-allowed"
          }`}
        >
          {TICKETING_OPTIONS.map((o) => (
            <option key={o.id} value={o.id}>
              {o.label}
            </option>
          ))}
        </select>
        <p className="text-xs text-gray-500 mt-1">{activeHint}</p>
        {!canEditSystem && (
          <p className="text-xs text-gray-500 mt-1">
            Only an admin can change the deployment's ticketing system. You can
            still manage your own Jira credential below.
          </p>
        )}
      </div>

      {system === "jira" && (
        <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 flex flex-col gap-3">
          {connected && (
            <p className="text-sm text-green-400">
              ✓ Connected as <strong>{connected.email}</strong> on{" "}
              <span className="font-mono">{connected.site}</span>. Leave the
              form blank to keep it, or enter a new token to replace it.
            </p>
          )}
          <p className="text-sm text-gray-300">
            Your Jira credentials are stored encrypted and used only for your
            own work items.
          </p>
          <div>
            <label htmlFor="onb-jira-site" className="block text-xs text-gray-400 mb-1">
              Atlassian site
            </label>
            <input
              id="onb-jira-site"
              type="text"
              value={site}
              onChange={(e) => onChangeSite(e.target.value)}
              placeholder="https://your-org.atlassian.net"
              className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
            />
          </div>
          <div>
            <label htmlFor="onb-jira-email" className="block text-xs text-gray-400 mb-1">
              Account email
            </label>
            <input
              id="onb-jira-email"
              type="email"
              value={email}
              onChange={(e) => onChangeEmail(e.target.value)}
              placeholder="you@your-org.com"
              className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
            />
          </div>
          <div>
            <label htmlFor="onb-jira-token" className="block text-xs text-gray-400 mb-1">
              API token
            </label>
            <input
              id="onb-jira-token"
              type="password"
              value={token}
              onChange={(e) => onChangeToken(e.target.value)}
              placeholder="Paste your Atlassian API token"
              autoComplete="off"
              className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
            />
            <p className="text-xs text-gray-500 mt-1">
              Create one at{" "}
              <a
                href="https://id.atlassian.com/manage-profile/security/api-tokens"
                target="_blank"
                rel="noopener noreferrer"
                className="text-blue-400 hover:text-blue-300"
                aria-label="Open the Atlassian API token page (opens in a new tab)"
              >
                id.atlassian.com → API tokens →
              </a>
            </p>
          </div>
        </div>
      )}
    </div>
  );
}

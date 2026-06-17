// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect } from "react";
import { apiJson } from "../../api/client";
import type { TodoTicket, GitHubIssue } from "../../api/types";
import { ConfirmModal } from "./ConfirmModal";

interface PickerTicket {
  key: string;
  summary: string;
  body?: string;
  url?: string;
  alreadyAdded: boolean;
  existingPrUrl?: string;
}

/** "#18" from a PR URL, else "a PR" — used in the re-add confirmation copy. */
function prLabel(prUrl: string): string {
  const m = prUrl.match(/\/(\d+)(?:[/?#].*)?$/);
  return m ? `#${m[1]}` : "a PR";
}

interface Props {
  ticketingSystem: string;
  /**
   * The repo the caller has currently selected in the header picker. For
   * GitHub mode, the picker fetches issues for THIS repo
   * (`/api/github/issues?repository=<name>`). When `null` ("All
   * repositories" is selected), the picker shows a CTA asking the user to
   * pick a specific repo first — there's no per-repo aggregation in v1.
   * Ignored in Jira mode.
   */
  activeRepoName?: string | null;
  onSelect: (key: string, summary: string, description?: string, url?: string) => void;
  onClose: () => void;
}

export function TicketPickerModal({ ticketingSystem, activeRepoName, onSelect, onClose }: Props) {
  const [tickets, setTickets] = useState<PickerTicket[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [confirmReAdd, setConfirmReAdd] = useState<PickerTicket | null>(null);
  const needsRepoForGitHub = ticketingSystem === "github" && !activeRepoName;

  useEffect(() => {
    // GitHub mode: hold off and surface a clearer CTA when no repo is selected.
    if (needsRepoForGitHub) {
      setLoading(false);
      setTickets([]);
      setError("");
      return;
    }
    setLoading(true);
    setError("");
    const endpoint =
      ticketingSystem === "github"
        ? `/api/github/issues?repository=${encodeURIComponent(activeRepoName!)}`
        : "/api/jira/todo-tickets-manual";

    apiJson<TodoTicket[] | GitHubIssue[]>(endpoint)
      .then((data) => {
        setTickets(
          data.map((t) => ({
            key: t.key,
            summary: t.summary,
            body: "body" in t ? t.body : undefined,
            url: "url" in t ? t.url : undefined,
            alreadyAdded: t.already_added,
            existingPrUrl: t.existing_pr_url ?? undefined,
          }))
        );
      })
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  }, [ticketingSystem, activeRepoName, needsRepoForGitHub]);

  // A ticket with no prior PR adds immediately; one with a prior PR routes
  // through a confirmation (the new run opens a separate PR).
  const handlePick = (t: PickerTicket) => {
    if (t.existingPrUrl) {
      setConfirmReAdd(t);
    } else {
      onSelect(t.key, t.summary, t.body, t.url);
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl max-w-3xl w-full mx-4 max-h-[80vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between p-4 border-b border-gray-800">
          <h3 className="text-lg font-medium text-white">
            {ticketingSystem === "github" ? "GitHub Issues" : "To Do Tickets"}
          </h3>
          <button onClick={onClose} className="text-gray-500 hover:text-gray-300 cursor-pointer">&times;</button>
        </div>

        <div className="overflow-y-auto flex-1 p-4">
          {needsRepoForGitHub && (
            <p className="text-gray-400 text-sm">
              Pick a specific repository in the header to see its GitHub issues.
              "All repositories" doesn't aggregate issues across multiple repos.
            </p>
          )}
          {!needsRepoForGitHub && loading && <p className="text-gray-500 text-sm">Loading...</p>}
          {error && <p className="text-red-400 text-sm">{error}</p>}
          {!loading && !needsRepoForGitHub && tickets.length === 0 && (
            <p className="text-gray-500 text-sm">No tickets found.</p>
          )}
          {tickets.map((t) =>
            t.alreadyAdded ? (
              <div
                key={t.key}
                aria-disabled="true"
                className="w-full text-left px-4 py-3 rounded-lg flex items-center gap-3 opacity-50 cursor-not-allowed"
              >
                <span className="font-mono text-xs text-blue-400 flex-shrink-0">{t.key}</span>
                <span className="text-sm text-gray-200 truncate">{t.summary}</span>
                <span className="ml-auto flex-shrink-0 text-xs text-gray-500">Already added</span>
              </div>
            ) : (
              <button
                key={t.key}
                onClick={() => handlePick(t)}
                className="w-full text-left px-4 py-3 rounded-lg hover:bg-gray-800 transition-colors flex items-center gap-3 cursor-pointer"
              >
                <span className="font-mono text-xs text-blue-400 flex-shrink-0">{t.key}</span>
                <span className="text-sm text-gray-200 truncate">{t.summary}</span>
              </button>
            )
          )}
        </div>
      </div>

      {confirmReAdd && (
        <ConfirmModal
          title="This issue already has a PR"
          message={`${confirmReAdd.key} already has ${prLabel(confirmReAdd.existingPrUrl!)}. Adding it again will create a NEW, separate pull request. Continue?`}
          confirmLabel="Add anyway"
          onConfirm={() => {
            const t = confirmReAdd;
            setConfirmReAdd(null);
            onSelect(t.key, t.summary, t.body, t.url);
          }}
          onCancel={() => setConfirmReAdd(null)}
        />
      )}
    </div>
  );
}

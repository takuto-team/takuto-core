// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect } from "react";
import { apiJson } from "../../api/client";
import type { TodoTicket, GitHubIssue } from "../../api/types";

interface Props {
  ticketingSystem: string;
  onSelect: (key: string, summary: string, description?: string) => void;
  onClose: () => void;
}

export function TicketPickerModal({ ticketingSystem, onSelect, onClose }: Props) {
  const [tickets, setTickets] = useState<{ key: string; summary: string; body?: string }[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    setLoading(true);
    const endpoint =
      ticketingSystem === "github" ? "/api/github/issues" : "/api/jira/todo-tickets-manual";

    apiJson<TodoTicket[] | GitHubIssue[]>(endpoint)
      .then((data) => {
        setTickets(
          data.map((t) => ({
            key: t.key,
            summary: t.summary,
            body: "body" in t ? t.body : undefined,
          }))
        );
      })
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  }, [ticketingSystem]);

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
          {loading && <p className="text-gray-500 text-sm">Loading...</p>}
          {error && <p className="text-red-400 text-sm">{error}</p>}
          {!loading && tickets.length === 0 && <p className="text-gray-500 text-sm">No tickets found.</p>}
          {tickets.map((t) => (
            <button
              key={t.key}
              onClick={() => onSelect(t.key, t.summary, t.body)}
              className="w-full text-left px-4 py-3 rounded-lg hover:bg-gray-800 transition-colors flex items-center gap-3 cursor-pointer"
            >
              <span className="font-mono text-xs text-blue-400 flex-shrink-0">{t.key}</span>
              <span className="text-sm text-gray-200 truncate">{t.summary}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

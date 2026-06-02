// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Header strip of `TicketDetailModal`. Renders the ticket key, the title
 * (read-only or as an inline text input depending on edit mode), and the
 * close button. The shell still owns `editTitle` and `pendingImprovement`.
 */

interface Props {
  ticketKey: string;
  summary: string;
  /** True when the user is editing AND not currently reviewing a diff. */
  editing: boolean;
  editTitle: string;
  onEditTitleChange: (value: string) => void;
  /** When the AI proposed a new summary, this overrides the read-only title. */
  pendingImprovedSummary?: string;
  onClose: () => void;
}

export function TicketDetailHeader({
  ticketKey,
  summary,
  editing,
  editTitle,
  onEditTitleChange,
  pendingImprovedSummary,
  onClose,
}: Props) {
  return (
    <div className="flex items-center justify-between p-4 border-b border-gray-800">
      <div className="min-w-0 flex-1">
        <span className="font-mono text-xs text-blue-400">{ticketKey}</span>
        {editing ? (
          <input
            type="text"
            value={editTitle}
            onChange={(e) => onEditTitleChange(e.target.value)}
            className="block w-full mt-1 bg-gray-950 border border-gray-700 rounded-lg px-3 py-1.5 text-lg font-medium text-white"
          />
        ) : (
          <h3 className="text-lg font-medium text-white truncate">
            {pendingImprovedSummary ?? summary}
          </h3>
        )}
      </div>
      <button onClick={onClose} className="text-gray-500 hover:text-gray-300 cursor-pointer text-xl flex-shrink-0 ml-4">
        &times;
      </button>
    </div>
  );
}

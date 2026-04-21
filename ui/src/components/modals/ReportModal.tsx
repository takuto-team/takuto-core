// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { WorkflowSummary } from "../../api/types";

interface Props {
  workflow: WorkflowSummary;
  onClose: () => void;
}

export function ReportModal({ workflow: w, onClose }: Props) {
  const report = {
    ticket_key: w.ticket_key,
    summary: w.ticket_summary,
    state: w.state,
    branch: w.branch_name,
    pr_url: w.pr_url,
    error: w.error,
    steps: w.steps_log.map((s) => ({
      name: s.name,
      status: s.status,
      error: s.error,
    })),
  };

  const text = JSON.stringify(report, null, 2);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl max-w-2xl w-full mx-4 max-h-[80vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between p-4 border-b border-gray-800">
          <h3 className="text-lg font-medium text-white">Workflow Report: {w.ticket_key}</h3>
          <button onClick={onClose} className="text-gray-500 hover:text-gray-300 cursor-pointer">&times;</button>
        </div>

        <div className="overflow-y-auto flex-1 p-4">
          <pre className="text-xs text-gray-300 font-mono whitespace-pre-wrap">{text}</pre>
        </div>

        <div className="flex justify-end gap-3 p-4 border-t border-gray-800">
          <button
            onClick={() => navigator.clipboard.writeText(text)}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
          >
            Copy
          </button>
          <button
            onClick={onClose}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

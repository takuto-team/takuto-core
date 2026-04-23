// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect } from "react";
import { api } from "../../api/client";
import type { WorkflowSummary } from "../../api/types";
import { MarkdownPreview } from "../MarkdownPreview";

interface Props {
  workflow: WorkflowSummary;
  onClose: () => void;
}

export function ReportModal({ workflow: w, onClose }: Props) {
  const [reportContent, setReportContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(w.has_report);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!w.has_report) return;
    setLoading(true);
    setError(null);
    api(`/api/workflows/${encodeURIComponent(w.ticket_key)}/report`)
      .then(async (res) => {
        if (!res.ok) {
          throw new Error(res.status === 404 ? "Report not found" : `HTTP ${res.status}`);
        }
        const data = await res.json();
        setReportContent(data.content);
      })
      .catch((e) => {
        setError(e instanceof Error ? e.message : "Failed to load report");
      })
      .finally(() => setLoading(false));
  }, [w.ticket_key, w.has_report]);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl w-full mx-4 max-h-[80vh] flex flex-col"
        style={{ maxWidth: "1000px" }}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between p-4 border-b border-gray-800">
          <h3 className="text-lg font-medium text-white">Workflow Report: {w.ticket_key}</h3>
          <button onClick={onClose} className="text-gray-500 hover:text-gray-300 cursor-pointer text-xl">&times;</button>
        </div>

        <div className="overflow-y-auto flex-1 p-6">
          {loading && (
            <div className="text-gray-400 text-sm">Loading report…</div>
          )}
          {error && (
            <div className="text-red-400 text-sm">{error}</div>
          )}
          {reportContent && (
            <MarkdownPreview markdown={reportContent} />
          )}
          {!loading && !error && !reportContent && (
            <div className="text-gray-500 text-sm">No report content available.</div>
          )}
        </div>

        <div className="flex justify-end gap-3 p-4 border-t border-gray-800">
          {reportContent && (
            <button
              onClick={() => navigator.clipboard.writeText(reportContent)}
              className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
            >
              Copy
            </button>
          )}
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

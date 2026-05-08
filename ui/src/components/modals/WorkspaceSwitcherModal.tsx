// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect } from "react";
import { apiJson, apiPost } from "../../api/client";
import type { Workspace } from "../../api/types";

interface Props {
  onClose: () => void;
  onSwitched: () => void;
  onAddRepo: () => void;
}

export function WorkspaceSwitcherModal({ onClose, onSwitched, onAddRepo }: Props) {
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [switching, setSwitching] = useState<string | null>(null);

  useEffect(() => {
    apiJson<Workspace[]>("/api/workspaces")
      .then(setWorkspaces)
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  }, []);

  const handleSwitch = async (name: string) => {
    setSwitching(name);
    try {
      await apiPost("/api/workspaces/switch", { name });
      onSwitched();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to switch workspace");
      setSwitching(null);
    }
  };

  const handleAddRepo = () => {
    onClose();
    onAddRepo();
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl max-w-lg w-full mx-4 flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between p-4 border-b border-gray-800">
          <h3 className="text-lg font-medium text-white">Switch Repository</h3>
          <button
            onClick={onClose}
            className="text-gray-500 hover:text-gray-300 cursor-pointer"
          >
            &times;
          </button>
        </div>

        {/* List */}
        <div className="overflow-y-auto max-h-[60vh] p-2">
          {loading && (
            <p className="text-gray-500 text-sm px-4 py-3">Loading workspaces…</p>
          )}
          {error && (
            <p className="text-red-400 text-sm px-4 py-3">{error}</p>
          )}
          {!loading && !error && workspaces.length === 0 && (
            <p className="text-gray-500 text-sm px-4 py-3">No repositories found in /workspaces/.</p>
          )}
          {workspaces.map((ws) => (
            <div
              key={ws.name}
              className={`flex items-center justify-between px-4 py-3 rounded-lg ${
                ws.active ? "bg-gray-800/60" : "hover:bg-gray-800/40"
              } transition-colors`}
            >
              <div className="flex-1 min-w-0 mr-3">
                <div className="flex items-center gap-2">
                  <span className="text-emerald-400 text-base w-4 flex-shrink-0">
                    {ws.active ? "✓" : ""}
                  </span>
                  {ws.html_url ? (
                    <a
                      href={ws.html_url}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="text-sm font-medium text-blue-400 hover:text-blue-300 transition-colors truncate"
                    >
                      {ws.name}
                    </a>
                  ) : (
                    <span className="text-sm font-medium text-gray-200 truncate">
                      {ws.name}
                    </span>
                  )}
                  {ws.active && (
                    <span className="text-xs text-gray-500 flex-shrink-0">active</span>
                  )}
                </div>
              </div>
              <button
                onClick={() => !ws.active && handleSwitch(ws.name)}
                disabled={ws.active || switching !== null}
                className="text-sm px-4 py-1.5 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-40 disabled:cursor-not-allowed transition-colors cursor-pointer flex-shrink-0"
              >
                {switching === ws.name ? "Switching…" : "Switch"}
              </button>
            </div>
          ))}
        </div>

        {/* Footer */}
        <div className="p-4 border-t border-gray-800">
          <button
            onClick={handleAddRepo}
            className="w-full text-sm px-4 py-2 rounded-lg border border-gray-700 text-gray-300 hover:bg-gray-800 hover:text-white transition-colors cursor-pointer"
          >
            + Add repository
          </button>
        </div>
      </div>
    </div>
  );
}

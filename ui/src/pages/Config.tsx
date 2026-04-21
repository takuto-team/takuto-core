// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect, type FormEvent } from "react";
import { Link } from "react-router-dom";
import { apiJson, api } from "../api/client";
import type { ConfigResponse } from "../api/types";

interface Props {
  onLogout: () => void;
  authEnabled: boolean;
}

export function Config({ onLogout, authEnabled }: Props) {
  const [config, setConfig] = useState<ConfigResponse | null>(null);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [maxConcurrent, setMaxConcurrent] = useState(1);
  const [maxActive, setMaxActive] = useState(0);
  const [status, setStatus] = useState<{ text: string; ok: boolean } | null>(null);

  useEffect(() => {
    apiJson<ConfigResponse>("/api/config")
      .then((data) => {
        setConfig(data);
        setUsername(data.web?.dashboard_username || "");
        setMaxConcurrent(data.general?.max_concurrent_workflows || 1);
        setMaxActive(data.general?.max_active_workflows || 0);
      })
      .catch(() => {});
  }, []);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    const payload: Record<string, unknown> = {
      general: {
        max_concurrent_workflows: Math.max(1, maxConcurrent),
        max_active_workflows: Math.max(0, maxActive),
      },
      web: {
        dashboard_username: username.trim(),
        ...(password.length > 0
          ? { dashboard_password: password }
          : !username.trim()
          ? { dashboard_password: "" }
          : {}),
      },
    };

    try {
      const res = await api("/api/config", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      if (res.ok) {
        const data = await res.json();
        setConfig(data);
        setPassword("");
        setStatus({ text: "Saved.", ok: true });
      } else {
        setStatus({ text: `Error: ${await res.text()}`, ok: false });
      }
    } catch (err) {
      setStatus({ text: `Failed: ${err instanceof Error ? err.message : "unknown"}`, ok: false });
    }
    setTimeout(() => setStatus(null), 6000);
  };

  return (
    <div className="min-h-screen">
      <header className="border-b border-gray-800 bg-gray-950/80 backdrop-blur-sm sticky top-0 z-40">
        <div className="max-w-3xl mx-auto px-4 sm:px-6 lg:px-8">
          <div className="flex items-center justify-between h-14">
            <Link to="/" className="flex items-center gap-2 text-gray-400 hover:text-gray-200 transition-colors text-sm">
              &larr; Dashboard
            </Link>
            <div className="flex items-center gap-2">
              <span className="text-lg font-bold text-white">Maestro</span>
              <span className="text-xs px-2 py-0.5 rounded-full bg-gray-800 text-gray-400 border border-gray-700">
                Runtime settings
              </span>
            </div>
            {authEnabled && (
              <button onClick={onLogout} className="text-xs text-gray-500 hover:text-gray-300 cursor-pointer">
                Log out
              </button>
            )}
          </div>
        </div>
      </header>

      <main className="max-w-3xl mx-auto px-4 sm:px-6 lg:px-8 py-8 space-y-8">
        <section className="rounded-lg border border-amber-500/30 bg-amber-500/5 px-4 py-3 text-sm text-amber-100/90">
          <p className="font-medium text-amber-200 mb-1">Security note</p>
          <p>
            Only the settings below can be changed from this page. Everything else must be edited in{" "}
            <code className="text-amber-300/90">config.toml</code> on the server, then restart Maestro.
          </p>
        </section>

        <section>
          <h2 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-3">
            Full config (read-only)
          </h2>
          <pre className="text-xs font-mono bg-gray-900 border border-gray-800 rounded-lg p-4 overflow-x-auto max-h-64 overflow-y-auto text-gray-400 whitespace-pre-wrap break-words">
            {config ? JSON.stringify(config, null, 2) : "Loading..."}
          </pre>
        </section>

        <form onSubmit={handleSubmit} className="space-y-8">
          <section>
            <h2 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-4">
              Web &amp; dashboard login
            </h2>
            <p className="text-xs text-gray-500 mb-4">
              Leave password blank to keep the current one. Both empty disables login.
            </p>
            <div className="space-y-4 max-w-md">
              <div>
                <label className="block text-sm font-medium text-gray-300 mb-1.5">Dashboard username</label>
                <input
                  type="text"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  autoComplete="username"
                  className="w-full bg-gray-900 border border-gray-700 rounded-lg px-4 py-2.5 text-sm text-gray-200"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-300 mb-1.5">New password</label>
                <input
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  autoComplete="new-password"
                  placeholder="Leave blank to keep unchanged"
                  className="w-full bg-gray-900 border border-gray-700 rounded-lg px-4 py-2.5 text-sm text-gray-200 placeholder-gray-600"
                />
              </div>
            </div>
          </section>

          <div className="border-t border-gray-800/60" />

          <section>
            <h2 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-4">Concurrency</h2>
            <div className="space-y-4 max-w-md">
              <div>
                <label className="block text-sm font-medium text-gray-300 mb-1.5">Max concurrent heavy steps</label>
                <input
                  type="number"
                  value={maxConcurrent}
                  onChange={(e) => setMaxConcurrent(parseInt(e.target.value) || 1)}
                  min={1}
                  max={50}
                  className="w-32 bg-gray-900 border border-gray-700 rounded-lg px-4 py-2.5 text-sm font-mono text-gray-200"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-300 mb-1.5">Max active workflows</label>
                <input
                  type="number"
                  value={maxActive}
                  onChange={(e) => setMaxActive(parseInt(e.target.value) || 0)}
                  min={0}
                  max={100}
                  className="w-32 bg-gray-900 border border-gray-700 rounded-lg px-4 py-2.5 text-sm font-mono text-gray-200"
                />
                <p className="mt-1.5 text-xs text-gray-600">0 = same as max concurrent.</p>
              </div>
            </div>
          </section>

          <div className="flex items-center justify-between pt-2 pb-8">
            {status && (
              <span className={`text-sm ${status.ok ? "text-green-400" : "text-red-400"}`}>{status.text}</span>
            )}
            <button
              type="submit"
              className="ml-auto text-sm font-medium px-6 py-2.5 rounded-lg bg-blue-600 text-white hover:bg-blue-500 cursor-pointer"
            >
              Save runtime settings
            </button>
          </div>
        </form>
      </main>
    </div>
  );
}

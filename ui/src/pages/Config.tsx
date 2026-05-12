// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect, useCallback } from "react";
import { Link } from "react-router-dom";
import { apiJson, api } from "../api/client";
import type { User } from "../api/types";

interface Props {
  onLogout: () => void;
  authEnabled: boolean;
  isAdmin?: boolean;
}

const ALL_TABS = ["Users"] as const;
type Tab = (typeof ALL_TABS)[number];

// ---------------------------------------------------------------------------
// Users tab
// ---------------------------------------------------------------------------

interface NewUserRow {
  username: string;
  password: string;
  role: "admin" | "user";
}

function UsersTab() {
  const [users, setUsers] = useState<User[]>([]);
  const [loading, setLoading] = useState(true);
  const [newRow, setNewRow] = useState<NewUserRow | null>(null);
  const [recoveryCodes, setRecoveryCodes] = useState<string[] | null>(null);
  const [createdUsername, setCreatedUsername] = useState("");
  const [error, setError] = useState("");

  const fetchUsers = useCallback(() => {
    setLoading(true);
    apiJson<User[]>("/api/users")
      .then(setUsers)
      .catch(() => {})
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    fetchUsers();
  }, [fetchUsers]);

  const handleCreate = async () => {
    if (!newRow || !newRow.username.trim() || !newRow.password) return;
    setError("");
    try {
      const res = await api("/api/users", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          username: newRow.username.trim(),
          password: newRow.password,
          role: newRow.role,
        }),
      });
      if (!res.ok) {
        const body = await res.json().catch(() => null);
        setError(body?.error ?? `Failed (${res.status})`);
        return;
      }
      const data = await res.json();
      setNewRow(null);
      setCreatedUsername(data.user?.username ?? newRow.username);
      setRecoveryCodes(data.recovery_codes ?? null);
      fetchUsers();
    } catch {
      setError("Could not reach the server.");
    }
  };

  const handleDelete = async (user: User) => {
    const ok = window.confirm(`Delete user "${user.username}"? This cannot be undone.`);
    if (!ok) return;
    await api(`/api/users/${user.id}`, { method: "DELETE" });
    fetchUsers();
  };

  const handleSuspend = async (user: User) => {
    const action = user.suspended ? "unsuspend" : "suspend";
    await api(`/api/users/${user.id}/${action}`, { method: "POST" });
    fetchUsers();
  };

  const handleRoleToggle = async (user: User) => {
    const newRole = user.role === "admin" ? "user" : "admin";
    await api(`/api/users/${user.id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ role: newRole }),
    });
    fetchUsers();
  };

  // Recovery codes modal after user creation
  if (recoveryCodes) {
    return (
      <div className="space-y-4">
        <div className="bg-amber-950 border border-amber-700 rounded-lg p-4">
          <h3 className="text-sm font-semibold text-amber-300 mb-2">
            Recovery codes for {createdUsername}
          </h3>
          <p className="text-xs text-amber-200/80 mb-3">
            Share these with the user. Each code can only be used once.
          </p>
          <div className="grid grid-cols-2 gap-2 mb-3 font-mono text-sm">
            {recoveryCodes.map((code) => (
              <div
                key={code}
                className="bg-gray-950 border border-gray-700 rounded px-3 py-1.5 text-gray-200 text-center"
              >
                {code}
              </div>
            ))}
          </div>
          <button
            type="button"
            onClick={() => {
              navigator.clipboard.writeText(recoveryCodes.join("\n")).catch(() => {});
            }}
            className="w-full py-1.5 rounded-lg bg-gray-800 text-gray-300 text-xs font-medium hover:bg-gray-700 cursor-pointer"
          >
            Copy all codes
          </button>
        </div>
        <button
          type="button"
          onClick={() => {
            setRecoveryCodes(null);
            setCreatedUsername("");
          }}
          className="text-sm text-blue-400 hover:text-blue-300 cursor-pointer"
        >
          Done
        </button>
      </div>
    );
  }

  if (loading) {
    return <p className="text-sm text-gray-500">Loading...</p>;
  }

  return (
    <div className="space-y-4">
      <table className="w-full text-sm">
        <thead>
          <tr className="text-left text-xs text-gray-500 uppercase tracking-wider">
            <th className="pb-2 font-medium">Username</th>
            <th className="pb-2 font-medium">Role</th>
            <th className="pb-2 font-medium">Status</th>
            <th className="pb-2 font-medium text-right">Actions</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-gray-800">
          {users.map((u) => (
            <tr key={u.id} className="group">
              <td className="py-2.5 text-gray-200">{u.username}</td>
              <td className="py-2.5">
                <button
                  onClick={() => handleRoleToggle(u)}
                  className={`text-xs px-2 py-0.5 rounded-full cursor-pointer ${
                    u.role === "admin"
                      ? "bg-blue-900/50 text-blue-300 border border-blue-700/50"
                      : "bg-gray-800 text-gray-400 border border-gray-700"
                  }`}
                  title={`Click to make ${u.role === "admin" ? "user" : "admin"}`}
                >
                  {u.role}
                </button>
              </td>
              <td className="py-2.5">
                {u.suspended ? (
                  <span className="text-xs text-red-400">Suspended</span>
                ) : (
                  <span className="text-xs text-green-400">Active</span>
                )}
              </td>
              <td className="py-2.5 text-right space-x-2">
                <button
                  onClick={() => handleSuspend(u)}
                  className="text-xs text-gray-500 hover:text-gray-300 cursor-pointer"
                >
                  {u.suspended ? "Unsuspend" : "Suspend"}
                </button>
                <button
                  onClick={() => handleDelete(u)}
                  className="text-xs text-red-500/70 hover:text-red-400 cursor-pointer"
                >
                  Delete
                </button>
              </td>
            </tr>
          ))}

          {/* New user inline row */}
          {newRow && (
            <tr>
              <td className="py-2.5">
                <input
                  type="text"
                  value={newRow.username}
                  onChange={(e) => setNewRow({ ...newRow, username: e.target.value })}
                  placeholder="Username"
                  autoFocus
                  className="w-full bg-gray-950 border border-gray-700 rounded px-2 py-1 text-sm text-gray-200"
                />
              </td>
              <td className="py-2.5">
                <select
                  value={newRow.role}
                  onChange={(e) =>
                    setNewRow({ ...newRow, role: e.target.value as "admin" | "user" })
                  }
                  className="bg-gray-950 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200"
                >
                  <option value="user">user</option>
                  <option value="admin">admin</option>
                </select>
              </td>
              <td className="py-2.5">
                <input
                  type="password"
                  value={newRow.password}
                  onChange={(e) => setNewRow({ ...newRow, password: e.target.value })}
                  placeholder="Password (12+ chars)"
                  className="w-full bg-gray-950 border border-gray-700 rounded px-2 py-1 text-sm text-gray-200"
                />
              </td>
              <td className="py-2.5 text-right space-x-2">
                <button
                  onClick={handleCreate}
                  disabled={!newRow.username.trim() || newRow.password.length < 12}
                  className="text-xs text-blue-400 hover:text-blue-300 disabled:text-gray-600 disabled:cursor-not-allowed cursor-pointer"
                >
                  Save
                </button>
                <button
                  onClick={() => {
                    setNewRow(null);
                    setError("");
                  }}
                  className="text-xs text-gray-500 hover:text-gray-300 cursor-pointer"
                >
                  Cancel
                </button>
              </td>
            </tr>
          )}
        </tbody>
      </table>

      {error && <p className="text-xs text-red-400">{error}</p>}

      {!newRow && (
        <button
          onClick={() => setNewRow({ username: "", password: "", role: "user" })}
          className="text-sm text-blue-400 hover:text-blue-300 cursor-pointer"
        >
          + Add user
        </button>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Config page
// ---------------------------------------------------------------------------

export function Config({ onLogout, authEnabled, isAdmin }: Props) {
  const tabs = ALL_TABS.filter((t) => t !== "Users" || isAdmin);
  const [tab, setTab] = useState<Tab>(tabs[0] ?? "Users");

  return (
    <div className="min-h-screen">
      <header className="border-b border-gray-800 bg-gray-950/80 backdrop-blur-sm sticky top-0 z-40">
        <div className="max-w-3xl mx-auto px-4 sm:px-6 lg:px-8">
          <div className="flex items-center justify-between h-14">
            <Link
              to="/"
              className="flex items-center gap-2 text-gray-400 hover:text-gray-200 transition-colors text-sm"
            >
              &larr; Dashboard
            </Link>
            <span className="text-lg font-bold text-white">Settings</span>
            {authEnabled && (
              <button
                onClick={onLogout}
                className="text-xs text-gray-500 hover:text-gray-300 cursor-pointer"
              >
                Log out
              </button>
            )}
          </div>
        </div>
      </header>

      {/* Tab bar */}
      <div className="border-b border-gray-800">
        <div className="max-w-3xl mx-auto px-4 sm:px-6 lg:px-8 flex gap-6">
          {tabs.map((t) => (
            <button
              key={t}
              onClick={() => setTab(t)}
              className={`py-3 text-sm font-medium border-b-2 cursor-pointer transition-colors ${
                tab === t
                  ? "border-blue-500 text-blue-400"
                  : "border-transparent text-gray-500 hover:text-gray-300"
              }`}
            >
              {t}
            </button>
          ))}
        </div>
      </div>

      <main className="max-w-3xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
        {tab === "Users" && <UsersTab />}
      </main>
    </div>
  );
}

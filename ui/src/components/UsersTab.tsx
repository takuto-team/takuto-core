// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState } from "react";
import type { User } from "../api/types";

interface NewUserRow {
  username: string;
  password: string;
  role: "admin" | "user";
}

interface Props {
  users: User[];
  onCreateUser: (username: string, password: string, role: "admin" | "user") => Promise<{ recovery_codes?: string[]; error?: string }>;
  onDeleteUser: (user: User) => Promise<void>;
  onSuspendToggle: (user: User) => Promise<void>;
  onRoleToggle: (user: User) => Promise<void>;
}

export function UsersTab({ users, onCreateUser, onDeleteUser, onSuspendToggle, onRoleToggle }: Props) {
  const [newRow, setNewRow] = useState<NewUserRow | null>(null);
  const [recoveryCodes, setRecoveryCodes] = useState<string[] | null>(null);
  const [createdUsername, setCreatedUsername] = useState("");
  const [error, setError] = useState("");

  const handleCreate = async () => {
    if (!newRow || !newRow.username.trim() || !newRow.password) return;
    setError("");
    const result = await onCreateUser(newRow.username.trim(), newRow.password, newRow.role);
    if (result.error) {
      setError(result.error);
      return;
    }
    setCreatedUsername(newRow.username.trim());
    setRecoveryCodes(result.recovery_codes ?? null);
    setNewRow(null);
  };

  const handleDelete = async (user: User) => {
    const ok = window.confirm(`Delete user "${user.username}"? This cannot be undone.`);
    if (!ok) return;
    await onDeleteUser(user);
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
                  onClick={() => onRoleToggle(u)}
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
                  onClick={() => onSuspendToggle(u)}
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

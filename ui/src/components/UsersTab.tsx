// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState } from "react";
import type { User } from "../api/types";
import { ConfirmModal } from "./modals/ConfirmModal";
import { copyToClipboard } from "../utils/clipboard";

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
  const [showPassword, setShowPassword] = useState(false);
  const [recoveryCodes, setRecoveryCodes] = useState<string[] | null>(null);
  const [createdUsername, setCreatedUsername] = useState("");
  const [error, setError] = useState("");
  const [confirmDelete, setConfirmDelete] = useState<User | null>(null);
  const [codesCopied, setCodesCopied] = useState(false);

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
            onClick={async () => {
              const ok = await copyToClipboard(recoveryCodes.join("\n"));
              if (ok) { setCodesCopied(true); setTimeout(() => setCodesCopied(false), 2000); }
            }}
            className="w-full py-1.5 rounded-lg bg-gray-800 text-gray-300 text-xs font-medium hover:bg-gray-700 cursor-pointer"
          >
            {codesCopied ? "Copied!" : "Copy all codes"}
          </button>
        </div>
        <button
          type="button"
          onClick={() => {
            setRecoveryCodes(null);
            setCreatedUsername("");
            setCodesCopied(false);
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
      <table className="w-full text-base">
        <thead>
          <tr className="text-left text-sm text-gray-500 uppercase tracking-wider">
            <th className="pb-3 font-medium">Username</th>
            <th className="pb-3 font-medium">Role</th>
            <th className="pb-3 font-medium">Status</th>
            <th className="pb-3 font-medium text-right">Actions</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-gray-800">
          {users.map((u) => (
            <tr key={u.id} className="group">
              <td className="py-3 text-gray-200">{u.username}</td>
              <td className="py-3">
                <button
                  onClick={() => onRoleToggle(u)}
                  className={`text-sm px-2.5 py-0.5 rounded-full cursor-pointer ${
                    u.role === "admin"
                      ? "bg-blue-900/50 text-blue-300 border border-blue-700/50"
                      : "bg-gray-800 text-gray-400 border border-gray-700"
                  }`}
                  title={`Click to make ${u.role === "admin" ? "user" : "admin"}`}
                >
                  {u.role}
                </button>
              </td>
              <td className="py-3">
                {u.suspended ? (
                  <span className="text-sm text-red-400">Suspended</span>
                ) : (
                  <span className="text-sm text-green-400">Active</span>
                )}
              </td>
              <td className="py-3 text-right space-x-3">
                <button
                  onClick={() => onSuspendToggle(u)}
                  className="text-sm text-gray-500 hover:text-gray-300 cursor-pointer"
                >
                  {u.suspended ? "Unsuspend" : "Suspend"}
                </button>
                <button
                  onClick={() => setConfirmDelete(u)}
                  className="text-sm text-red-500/70 hover:text-red-400 cursor-pointer"
                >
                  Delete
                </button>
              </td>
            </tr>
          ))}

          {/* New user inline row */}
          {newRow && (
            <tr>
              <td className="py-3">
                <input
                  type="text"
                  value={newRow.username}
                  onChange={(e) => setNewRow({ ...newRow, username: e.target.value })}
                  placeholder="Username"
                  autoFocus
                  className="w-full bg-gray-950 border border-gray-700 rounded px-3 py-1.5 text-base text-gray-200"
                />
              </td>
              <td className="py-3">
                <select
                  value={newRow.role}
                  onChange={(e) =>
                    setNewRow({ ...newRow, role: e.target.value as "admin" | "user" })
                  }
                  className="bg-gray-950 border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200"
                >
                  <option value="user">user</option>
                  <option value="admin">admin</option>
                </select>
              </td>
              <td className="py-3">
                <div className="relative">
                  <input
                    type={showPassword ? "text" : "password"}
                    value={newRow.password}
                    onChange={(e) => setNewRow({ ...newRow, password: e.target.value })}
                    placeholder="Password (12+ chars)"
                    className="w-full bg-gray-950 border border-gray-700 rounded px-3 py-1.5 pr-9 text-base text-gray-200"
                  />
                  <button
                    type="button"
                    onClick={() => setShowPassword(!showPassword)}
                    className="absolute right-2 top-1/2 -translate-y-1/2 text-gray-500 hover:text-gray-300 cursor-pointer"
                    title={showPassword ? "Hide password" : "Show password"}
                  >
                    {showPassword ? (
                      <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4.5 h-4.5">
                        <path fillRule="evenodd" d="M3.28 2.22a.75.75 0 0 0-1.06 1.06l14.5 14.5a.75.75 0 1 0 1.06-1.06l-1.745-1.745a10.029 10.029 0 0 0 3.3-4.38 1.651 1.651 0 0 0 0-1.185A10.004 10.004 0 0 0 9.999 3a9.956 9.956 0 0 0-4.744 1.194L3.28 2.22ZM7.752 6.69l1.092 1.092a2.5 2.5 0 0 1 3.374 3.373l1.092 1.092a4 4 0 0 0-5.558-5.558Z" clipRule="evenodd" />
                        <path d="M10.748 13.93l2.523 2.523A9.987 9.987 0 0 1 10 17c-4.257 0-7.893-2.66-9.336-6.41a1.651 1.651 0 0 1 0-1.186A10.007 10.007 0 0 1 4.818 5.88l1.426 1.426A4 4 0 0 0 10.748 13.93Z" />
                      </svg>
                    ) : (
                      <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4.5 h-4.5">
                        <path d="M10 12.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z" />
                        <path fillRule="evenodd" d="M.664 10.59a1.651 1.651 0 0 1 0-1.186A10.004 10.004 0 0 1 10 3c4.257 0 7.893 2.66 9.336 6.41.147.381.146.804 0 1.186A10.004 10.004 0 0 1 10 17c-4.257 0-7.893-2.66-9.336-6.41ZM14 10a4 4 0 1 1-8 0 4 4 0 0 1 8 0Z" clipRule="evenodd" />
                      </svg>
                    )}
                  </button>
                </div>
              </td>
              <td className="py-3 text-right space-x-3">
                <button
                  onClick={handleCreate}
                  disabled={!newRow.username.trim() || newRow.password.length < 12}
                  className="text-sm text-blue-400 hover:text-blue-300 disabled:text-gray-600 disabled:cursor-not-allowed cursor-pointer"
                >
                  Save
                </button>
                <button
                  onClick={() => {
                    setNewRow(null);
                    setShowPassword(false);
                    setError("");
                  }}
                  className="text-sm text-gray-500 hover:text-gray-300 cursor-pointer"
                >
                  Cancel
                </button>
              </td>
            </tr>
          )}
        </tbody>
      </table>

      {error && <p className="text-sm text-red-400">{error}</p>}

      {!newRow && (
        <button
          onClick={() => { setNewRow({ username: "", password: "", role: "user" }); setShowPassword(false); }}
          className="text-base text-blue-400 hover:text-blue-300 cursor-pointer"
        >
          + Add user
        </button>
      )}

      {confirmDelete && (
        <ConfirmModal
          title="Delete user"
          message={`Delete user "${confirmDelete.username}"? This cannot be undone.`}
          onConfirm={async () => {
            await onDeleteUser(confirmDelete);
            setConfirmDelete(null);
          }}
          onCancel={() => setConfirmDelete(null)}
        />
      )}
    </div>
  );
}

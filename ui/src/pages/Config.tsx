// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect, useCallback } from "react";
import { Link } from "react-router-dom";
import { apiJson, api } from "../api/client";
import type { User } from "../api/types";
import { UsersTab } from "../components/UsersTab";
import { SecurityTab } from "../components/SecurityTab";

interface Props {
  onLogout: () => void;
  authEnabled: boolean;
  isAdmin?: boolean;
}

const ALL_TABS = ["Security", "Users"] as const;
type Tab = (typeof ALL_TABS)[number];

// ---------------------------------------------------------------------------
// Users data wrapper — fetches from API and delegates to UsersTab
// ---------------------------------------------------------------------------

function UsersTabConnected() {
  const [users, setUsers] = useState<User[]>([]);
  const [loading, setLoading] = useState(true);

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

  const handleCreate = async (username: string, password: string, role: "admin" | "user") => {
    try {
      const res = await api("/api/users", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ username, password, role }),
      });
      if (!res.ok) {
        const body = await res.json().catch(() => null);
        return { error: body?.error ?? `Failed (${res.status})` };
      }
      const data = await res.json();
      fetchUsers();
      return { recovery_codes: data.recovery_codes };
    } catch {
      return { error: "Could not reach the server." };
    }
  };

  const handleDelete = async (user: User) => {
    await api(`/api/users/${user.id}`, { method: "DELETE" });
    fetchUsers();
  };

  const handleSuspendToggle = async (user: User) => {
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

  if (loading) {
    return <p className="text-sm text-gray-500">Loading...</p>;
  }

  return (
    <UsersTab
      users={users}
      onCreateUser={handleCreate}
      onDeleteUser={handleDelete}
      onSuspendToggle={handleSuspendToggle}
      onRoleToggle={handleRoleToggle}
    />
  );
}

// ---------------------------------------------------------------------------
// Security data wrapper
// ---------------------------------------------------------------------------

function SecurityTabConnected() {
  const handleChangePassword = async (currentPassword: string, newPassword: string) => {
    try {
      const res = await api("/api/auth/change-password", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ current_password: currentPassword, new_password: newPassword }),
      });
      if (!res.ok) {
        const body = await res.json().catch(() => null);
        return { error: body?.error ?? `Failed (${res.status})` };
      }
      return {};
    } catch {
      return { error: "Could not reach the server." };
    }
  };

  return <SecurityTab onChangePassword={handleChangePassword} />;
}

// ---------------------------------------------------------------------------
// Config page
// ---------------------------------------------------------------------------

export function Config({ onLogout, authEnabled, isAdmin }: Props) {
  const tabs = ALL_TABS.filter((t) => t !== "Users" || isAdmin);
  const [tab, setTab] = useState<Tab>(tabs[0]);

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
        {tab === "Security" && <SecurityTabConnected />}
        {tab === "Users" && <UsersTabConnected />}
      </main>
    </div>
  );
}

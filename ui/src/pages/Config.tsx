// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect, useCallback, useRef } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { apiJson, api } from "../api/client";
import { UnsavedChangesModal } from "../components/UnsavedChangesModal";
import { useUnsavedChangesGuard } from "../hooks/useUnsavedChangesGuard";
import { surfaceError } from "../utils/surfaceError";
import type { User } from "../api/types";
import { UsersTab } from "../components/UsersTab";
import { SecurityTab } from "../components/SecurityTab";
import { WorktreeSettingsTab } from "../components/WorktreeSettingsTab";
import { MyRepositoriesTab } from "../components/MyRepositoriesTab";
import { AiSettingsTab } from "../components/AiSettingsTab";
import { FlowsTab } from "../components/FlowsTab";
import { TicketingTab } from "../components/TicketingTab";
import { GitHubCredentialsSection } from "../components/credentials/GitHubCredentialsSection";

interface Props {
  onLogout: () => void;
  authEnabled: boolean;
  isAdmin?: boolean;
}

const ALL_TABS = [
  "Security",
  "AI Settings",
  "GitHub",
  "Ticketing",
  "Users",
  "My Repositories",
  "Repository Settings",
  "Workflows",
] as const;
type Tab = (typeof ALL_TABS)[number];

/** Maps each stable tab identifier to its `common:tabs.*` display key. */
const TAB_LABEL_KEY: Record<Tab, string> = {
  "Security": "common:tabs.security",
  "AI Settings": "common:tabs.ai",
  "GitHub": "common:tabs.github",
  "Ticketing": "common:tabs.ticketing",
  "Users": "common:tabs.users",
  "My Repositories": "common:tabs.repositories",
  "Repository Settings": "common:tabs.repositorySettings",
  "Workflows": "common:tabs.workflows",
};

// ---------------------------------------------------------------------------
// Users data wrapper — fetches from API and delegates to UsersTab
// ---------------------------------------------------------------------------

function UsersTabConnected() {
  const { t } = useTranslation("config");
  const [users, setUsers] = useState<User[]>([]);
  const [loading, setLoading] = useState(true);

  const fetchUsers = useCallback(() => {
    setLoading(true);
    apiJson<User[]>("/api/users")
      .then(setUsers)
      .catch((e) => surfaceError(e, t("users.loadFailed")))
      .finally(() => setLoading(false));
  }, [t]);

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
        return { error: body?.error ?? t("errors.failedStatus", { status: res.status }) };
      }
      const data = await res.json();
      fetchUsers();
      return { recovery_codes: data.recovery_codes };
    } catch {
      return { error: t("errors.serverUnreachable") };
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
    return <p className="text-sm text-gray-500">{t("users.loading")}</p>;
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
  const { t } = useTranslation("config");
  const handleChangePassword = async (currentPassword: string, newPassword: string) => {
    try {
      const res = await api("/api/auth/change-password", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ current_password: currentPassword, new_password: newPassword }),
      });
      if (!res.ok) {
        const body = await res.json().catch(() => null);
        return { error: body?.error ?? t("errors.failedStatus", { status: res.status }) };
      }
      return {};
    } catch {
      return { error: t("errors.serverUnreachable") };
    }
  };

  const handleRegenerateRecoveryCodes = async () => {
    try {
      const res = await api("/api/auth/recovery-codes", { method: "POST" });
      if (!res.ok) {
        const body = await res.json().catch(() => null);
        return { error: body?.error ?? t("errors.failedStatus", { status: res.status }) };
      }
      const data = await res.json();
      return { recovery_codes: data.recovery_codes };
    } catch {
      return { error: t("errors.serverUnreachable") };
    }
  };

  return (
    <SecurityTab
      onChangePassword={handleChangePassword}
      onRegenerateRecoveryCodes={handleRegenerateRecoveryCodes}
    />
  );
}

// ---------------------------------------------------------------------------
// Config page
// ---------------------------------------------------------------------------

export function Config({ onLogout, authEnabled, isAdmin }: Props) {
  const { t } = useTranslation("config");
  // Admin-only tabs: "Users". "Repository Settings" and "My Repositories" are
  // user-facing — no admin gate; each user manages their own data. Item polling
  // moved into the "Ticketing" tab, where it is admin-gated internally.
  const adminOnlyTabs: Tab[] = ["Users"];
  const tabs = ALL_TABS.filter((tab) => (adminOnlyTabs.includes(tab) ? isAdmin : true));

  // Allow direct deep-linking via `?tab=<slug>` (used by Header, redirects
  // from legacy /admin/ai and /me/credentials routes, and OnboardingBanner
  // CTAs).
  const initialTab: Tab = (() => {
    if (typeof window === "undefined") return tabs[0];
    const params = new URLSearchParams(window.location.search);
    const slug = params.get("tab");
    if (slug === "ai") return "AI Settings";
    if (slug === "github") return "GitHub";
    // Item polling merged into the Ticketing tab — keep the old slugs working.
    if (slug === "ticketing" || slug === "polling" || slug === "item-polling") {
      return "Ticketing";
    }
    if (slug === "repositories") return "My Repositories";
    if (slug === "worktree") return "Repository Settings";
    if (slug === "Flows" || slug === "flows" || slug === "workflows") return "Workflows";
    if (slug === "users" && isAdmin) return "Users";
    if (slug === "security") return "Security";
    return tabs[0];
  })();
  const [tab, setTab] = useState<Tab>(initialTab);
  const navigate = useNavigate();

  // Unsaved-changes guard, scoped to the AI Settings tab for now. The tab
  // reports its combined dirty state and registers a "save all" fn.
  const [aiDirty, setAiDirty] = useState(false);
  const aiSaveRef = useRef<() => Promise<boolean>>(() => Promise.resolve(true));
  // The action to run once the user resolves the unsaved-changes modal (switch
  // tab / go to dashboard / log out). Null when the modal is closed.
  const [pendingLeave, setPendingLeave] = useState<{ run: () => void } | null>(null);

  const guardActive = tab === "AI Settings" && aiDirty;
  useUnsavedChangesGuard(guardActive);

  // Route an intended departure through the modal when the guard is active.
  // Plain function (no useCallback) so the React Compiler can memoize it.
  const requestLeave = (run: () => void) => {
    if (guardActive) setPendingLeave({ run });
    else run();
  };

  return (
    <div className="min-h-screen">
      <header className="border-b border-gray-800 bg-gray-950/80 backdrop-blur-sm sticky top-0 z-40">
        <div className="w-full px-4 sm:px-6 lg:px-8">
          <div className="flex items-center justify-between h-14">
            <Link
              to="/"
              onClick={(e) => {
                if (guardActive) {
                  e.preventDefault();
                  requestLeave(() => navigate("/"));
                }
              }}
              className="flex items-center gap-2 text-gray-400 hover:text-gray-200 transition-colors text-sm"
            >
              {t("page.backToDashboard")}
            </Link>
            <span className="text-lg font-bold text-white">{t("page.title")}</span>
            {authEnabled && (
              <button
                onClick={() => requestLeave(onLogout)}
                className="text-xs text-gray-500 hover:text-gray-300 cursor-pointer"
              >
                {t("page.logout")}
              </button>
            )}
          </div>
        </div>
      </header>

      {/* Tab bar */}
      <div className="border-b border-gray-800">
        <div className="w-full px-4 sm:px-6 lg:px-8 flex gap-6">
          {tabs.map((tabName) => (
            <button
              key={tabName}
              onClick={() => requestLeave(() => setTab(tabName))}
              className={`py-3 text-sm font-medium border-b-2 cursor-pointer transition-colors ${
                tab === tabName
                  ? "border-blue-500 text-blue-400"
                  : "border-transparent text-gray-500 hover:text-gray-300"
              }`}
            >
              {t(TAB_LABEL_KEY[tabName])}
            </button>
          ))}
        </div>
      </div>

      <main className="w-full px-4 sm:px-6 lg:px-8 py-8">
        {tab === "Security" && <SecurityTabConnected />}
        {tab === "AI Settings" && (
          <AiSettingsTab
            isAdmin={!!isAdmin}
            onDirtyChange={setAiDirty}
            registerSave={(fn) => {
              aiSaveRef.current = fn;
            }}
          />
        )}
        {tab === "GitHub" && <GitHubCredentialsSection />}
        {tab === "Ticketing" && <TicketingTab isAdmin={isAdmin} />}
        {tab === "Users" && <UsersTabConnected />}
        {tab === "My Repositories" && <MyRepositoriesTab isAdmin={isAdmin} />}
        {tab === "Repository Settings" && <WorktreeSettingsTab />}
        {tab === "Workflows" && <FlowsTab />}
      </main>

      {pendingLeave && (
        <UnsavedChangesModal
          onSave={() => aiSaveRef.current()}
          onProceed={() => {
            const { run } = pendingLeave;
            setPendingLeave(null);
            setAiDirty(false);
            run();
          }}
          onCancel={() => setPendingLeave(null)}
        />
      )}
    </div>
  );
}

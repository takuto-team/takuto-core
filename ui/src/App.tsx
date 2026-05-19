// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { ReactNode } from "react";
import { Routes, Route, Navigate } from "react-router-dom";
import { AdminAiSettings } from "./pages/AdminAiSettings";
import { Dashboard } from "./pages/Dashboard";
import { Login } from "./pages/Login";
import { Onboarding } from "./pages/Onboarding";
import { Setup } from "./pages/Setup";
import { Config } from "./pages/Config";
import { UserCredentials } from "./pages/UserCredentials";
import { useAuth } from "./hooks/useAuth";
import { ToastProvider, ToastContainer } from "./hooks/useToast";

/**
 * Belt-and-braces admin route guard. Even though `useAuth` now waits for
 * `currentUser` before flipping `loading` to false (fix in #34 follow-up),
 * the route element should ALSO refuse to decide until loading resolves —
 * otherwise any future refactor that re-introduces the
 * `loading-false-but-currentUser-null` race silently bounces admins on a
 * direct URL load.
 *
 * Order of checks matters: loading → null currentUser → role.
 */
export function RequireAdmin({
  loading,
  currentUser,
  children,
}: {
  loading: boolean;
  currentUser: { role: "admin" | "user" } | null;
  children: ReactNode;
}) {
  // ALWAYS check loading first. A direct URL load (e.g. user pastes
  // /admin/ai into the address bar) renders this route during the
  // auth-loading window before currentUser has resolved. Returning
  // Navigate here would bounce admins to "/" — instead, show a spinner
  // and let the auth fetch settle.
  if (loading) {
    return (
      <div className="flex items-center justify-center min-h-screen">
        <span className="text-gray-500 text-sm">Loading...</span>
      </div>
    );
  }
  // Loading is done — currentUser is the authoritative answer. `null`
  // means the /api/auth/me fetch returned a non-OK response, which is
  // effectively "not signed in / not an admin" — redirect home rather
  // than stick the user on a spinner forever.
  if (currentUser === null || currentUser.role !== "admin") {
    return <Navigate to="/" replace />;
  }
  return <>{children}</>;
}

export function App() {
  const { authEnabled, loggedIn, setupRequired, currentUser, loading, login, logout, completeSetup } = useAuth();

  if (loading) {
    return (
      <div className="flex items-center justify-center min-h-screen">
        <span className="text-gray-500 text-sm">Loading...</span>
      </div>
    );
  }

  if (setupRequired) {
    return <Setup onSetupComplete={completeSetup} onLogin={login} />;
  }

  if (authEnabled && !loggedIn) {
    return <Login onLogin={login} />;
  }

  return (
    <ToastProvider>
      <Routes>
        <Route
          path="/"
          element={
            <Dashboard
              onLogout={logout}
              authEnabled={authEnabled}
              isAdmin={currentUser?.role === "admin"}
            />
          }
        />
        <Route path="/login.html" element={<Login onLogin={login} />} />
        <Route path="/config.html" element={<Config onLogout={logout} authEnabled={authEnabled} isAdmin={currentUser?.role === "admin"} />} />
        {/* Phase 1 (auth-overhaul) — admin AI Settings + onboarding wizard.
            /admin/ai is admin-gated client-side; the server enforces this
            again on PUT /api/config/agent (04_architecture.md §2.3).
            `RequireAdmin` keeps the user on the page during the
            auth-loading window instead of Navigate-bouncing — fix for the
            "admin user gets sent to / on direct URL load" race. */}
        <Route
          path="/admin/ai"
          element={
            <RequireAdmin loading={loading} currentUser={currentUser}>
              <AdminAiSettings
                onLogout={logout}
                authEnabled={authEnabled}
                isAdmin
              />
            </RequireAdmin>
          }
        />
        <Route
          path="/onboarding"
          element={<Onboarding onLogout={logout} authEnabled={authEnabled} />}
        />
        {/* Phase 2 (auth-overhaul) — per-user credential surface. Any logged-in
            user can manage their own credentials; the App.tsx login gate
            above already guarantees we only land here authenticated. */}
        <Route
          path="/me/credentials"
          element={
            <UserCredentials onLogout={logout} authEnabled={authEnabled} />
          }
        />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
      <ToastContainer />
    </ToastProvider>
  );
}

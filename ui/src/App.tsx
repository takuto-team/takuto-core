// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { ReactNode } from "react";
import { Routes, Route, Navigate } from "react-router-dom";
import { Dashboard } from "./pages/Dashboard";
import { Login } from "./pages/Login";
import { Onboarding } from "./pages/Onboarding";
import { Setup } from "./pages/Setup";
import { Config } from "./pages/Config";
import { useAuth } from "./hooks/useAuth";
import { ToastProvider, ToastContainer } from "./hooks/useToast";

/**
 * Belt-and-braces admin route guard. Kept around for any future admin-only
 * route — though the legacy `/admin/ai` route has been folded into a tab
 * inside `/config.html` (`?tab=ai`), where the admin section is gated
 * client-side by props rather than by a route wrapper. Server-side
 * enforcement at the underlying endpoints is the real security boundary.
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
  // ALWAYS check loading first. A direct URL load on an admin-only route
  // renders this component during the auth-loading window before
  // currentUser has resolved. Returning Navigate here would bounce
  // admins — instead, show a spinner and let the auth fetch settle.
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
        {/* Legacy routes kept as redirects so old bookmarks and external
            links (e.g. notification emails) still land users in the right
            place. AI provider config + per-user credentials now live in
            one consolidated tab on /config.html (admin section is gated
            inside the tab; the route itself is open to all signed-in
            users). */}
        <Route
          path="/admin/ai"
          element={<Navigate to="/config.html?tab=ai" replace />}
        />
        <Route
          path="/me/credentials"
          element={<Navigate to="/config.html?tab=ai" replace />}
        />
        <Route
          path="/onboarding"
          element={<Onboarding onLogout={logout} authEnabled={authEnabled} />}
        />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
      <ToastContainer />
    </ToastProvider>
  );
}

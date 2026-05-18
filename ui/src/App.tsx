// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

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
            again on PUT /api/config/agent (04_architecture.md §2.3). */}
        <Route
          path="/admin/ai"
          element={
            currentUser?.role === "admin" ? (
              <AdminAiSettings
                onLogout={logout}
                authEnabled={authEnabled}
                isAdmin
              />
            ) : (
              <Navigate to="/" replace />
            )
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

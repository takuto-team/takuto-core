// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { Routes, Route, Navigate } from "react-router-dom";
import { Dashboard } from "./pages/Dashboard";
import { Login } from "./pages/Login";
import { Setup } from "./pages/Setup";
import { Config } from "./pages/Config";
import { useAuth } from "./hooks/useAuth";
import { ToastProvider, ToastContainer } from "./hooks/useToast";

export function App() {
  const { authEnabled, loggedIn, setupRequired, loading, login, logout, completeSetup } = useAuth();

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
        <Route path="/" element={<Dashboard onLogout={logout} authEnabled={authEnabled} />} />
        <Route path="/login.html" element={<Login onLogin={login} />} />
        <Route path="/config.html" element={<Config onLogout={logout} authEnabled={authEnabled} />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
      <ToastContainer />
    </ToastProvider>
  );
}

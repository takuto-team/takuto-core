// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect, useCallback } from "react";
import type { AuthStatus } from "../api/types";

export function useAuth() {
  const [authEnabled, setAuthEnabled] = useState(false);
  const [loggedIn, setLoggedIn] = useState(false);
  const [setupRequired, setSetupRequired] = useState(false);
  const [loading, setLoading] = useState(true);

  const checkAuth = useCallback(() => {
    fetch("/api/auth/status", { credentials: "same-origin" })
      .then((r) => r.json() as Promise<AuthStatus>)
      .then((data) => {
        setAuthEnabled(data.dashboard_auth_enabled);
        setSetupRequired(data.setup_required);
        if (!data.dashboard_auth_enabled && !data.setup_required) {
          setLoggedIn(true);
        } else if (data.setup_required) {
          setLoggedIn(false);
        } else {
          // Try a protected endpoint to see if session cookie is valid
          return fetch("/api/config", { credentials: "same-origin" }).then((r) => {
            setLoggedIn(r.ok);
          });
        }
      })
      .catch(() => {
        setAuthEnabled(false);
        setLoggedIn(true);
      })
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    checkAuth();
  }, [checkAuth]);

  const login = useCallback(async (username: string, password: string) => {
    const res = await fetch("/api/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify({ username, password }),
    });
    if (res.ok) {
      setLoggedIn(true);
      return true;
    }
    return false;
  }, []);

  const logout = useCallback(async () => {
    await fetch("/api/auth/logout", { method: "POST", credentials: "same-origin" });
    setLoggedIn(false);
  }, []);

  const completeSetup = useCallback(() => {
    setSetupRequired(false);
    setAuthEnabled(true);
  }, []);

  return { authEnabled, loggedIn, setupRequired, loading, login, logout, completeSetup };
}

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect, useCallback } from "react";
import type { AuthStatus } from "../api/types";

interface CurrentUser {
  username: string;
  role: "admin" | "user";
}

export function useAuth() {
  const [authEnabled, setAuthEnabled] = useState(false);
  const [loggedIn, setLoggedIn] = useState(false);
  const [setupRequired, setSetupRequired] = useState(false);
  const [currentUser, setCurrentUser] = useState<CurrentUser | null>(null);
  const [loading, setLoading] = useState(true);

  const fetchMe = useCallback(() => {
    fetch("/api/auth/me", { credentials: "same-origin" })
      .then((r) => (r.ok ? (r.json() as Promise<CurrentUser>) : null))
      .then((u) => setCurrentUser(u ?? null))
      .catch(() => setCurrentUser(null));
  }, []);

  const checkAuth = useCallback(() => {
    fetch("/api/auth/status", { credentials: "same-origin" })
      .then((r) => r.json() as Promise<AuthStatus>)
      .then((data) => {
        setAuthEnabled(data.dashboard_auth_enabled);
        setSetupRequired(data.setup_required);
        if (!data.dashboard_auth_enabled && !data.setup_required) {
          setLoggedIn(true);
          fetchMe();
        } else if (data.setup_required) {
          setLoggedIn(false);
        } else {
          return fetch("/api/config", { credentials: "same-origin" }).then((r) => {
            setLoggedIn(r.ok);
            if (r.ok) fetchMe();
          });
        }
      })
      .catch(() => {
        setAuthEnabled(false);
        setLoggedIn(true);
      })
      .finally(() => setLoading(false));
  }, [fetchMe]);

  useEffect(() => {
    checkAuth();
  }, [checkAuth]);

  const login = useCallback(
    async (username: string, password: string) => {
      const res = await fetch("/api/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "same-origin",
        body: JSON.stringify({ username, password }),
      });
      if (res.ok) {
        setLoggedIn(true);
        fetchMe();
        return true;
      }
      return false;
    },
    [fetchMe],
  );

  const logout = useCallback(async () => {
    await fetch("/api/auth/logout", { method: "POST", credentials: "same-origin" });
    setLoggedIn(false);
    setCurrentUser(null);
  }, []);

  const completeSetup = useCallback(() => {
    setSetupRequired(false);
    setAuthEnabled(true);
  }, []);

  return {
    authEnabled,
    loggedIn,
    setupRequired,
    currentUser,
    loading,
    login,
    logout,
    completeSetup,
  };
}

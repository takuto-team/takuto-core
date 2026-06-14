// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { queryKeys } from "../api/queryClient";
import type { AuthStatus } from "../api/types";

interface CurrentUser {
  username: string;
  role: "admin" | "user";
}

interface AuthBootstrap {
  authEnabled: boolean;
  loggedIn: boolean;
  setupRequired: boolean;
  currentUser: CurrentUser | null;
}

async function fetchMe(): Promise<CurrentUser | null> {
  return fetch("/api/auth/me", { credentials: "same-origin" })
    .then((r) => (r.ok ? (r.json() as Promise<CurrentUser>) : null))
    .catch(() => null);
}

/**
 * The whole auth bootstrap runs inside a single query function so the query
 * only resolves once the entire `status → config → me` sequence is done.
 * That keeps `loading` (the query's pending flag) `true` until
 * `currentUser` has been populated — closing the race that previously
 * bounced an admin to "/" on a direct admin-route load (the
 * `loading=false + currentUser=null` window).
 */
async function bootstrapAuth(): Promise<AuthBootstrap> {
  try {
    const statusRes = await fetch("/api/auth/status", { credentials: "same-origin" });
    const status = (await statusRes.json()) as AuthStatus;
    const authEnabled = !!status.dashboard_auth_enabled;
    const setupRequired = !!status.setup_required;

    if (!authEnabled && !setupRequired) {
      const currentUser = await fetchMe();
      return { authEnabled, setupRequired, loggedIn: true, currentUser };
    }
    if (setupRequired) {
      return { authEnabled, setupRequired, loggedIn: false, currentUser: null };
    }
    const cfgRes = await fetch("/api/config", { credentials: "same-origin" });
    const loggedIn = cfgRes.ok;
    const currentUser = loggedIn ? await fetchMe() : null;
    return { authEnabled, setupRequired, loggedIn, currentUser };
  } catch {
    return { authEnabled: false, setupRequired: false, loggedIn: true, currentUser: null };
  }
}

export function useAuth() {
  const queryClient = useQueryClient();
  const { data, isPending } = useQuery({
    queryKey: queryKeys.auth,
    queryFn: bootstrapAuth,
  });

  const patch = useCallback(
    (next: Partial<AuthBootstrap>) => {
      queryClient.setQueryData<AuthBootstrap>(queryKeys.auth, (prev) => ({
        authEnabled: prev?.authEnabled ?? false,
        loggedIn: prev?.loggedIn ?? false,
        setupRequired: prev?.setupRequired ?? false,
        currentUser: prev?.currentUser ?? null,
        ...next,
      }));
    },
    [queryClient],
  );

  const loginMutation = useMutation({
    mutationFn: async (vars: { username: string; password: string }): Promise<boolean> => {
      const res = await fetch("/api/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "same-origin",
        body: JSON.stringify({ username: vars.username, password: vars.password }),
      });
      return res.ok;
    },
    onSuccess: async (ok) => {
      if (!ok) return;
      const currentUser = await fetchMe();
      patch({ loggedIn: true, currentUser });
    },
  });

  const logoutMutation = useMutation({
    mutationFn: async (): Promise<void> => {
      await fetch("/api/auth/logout", { method: "POST", credentials: "same-origin" });
    },
    onSuccess: () => {
      patch({ loggedIn: false, currentUser: null });
    },
  });

  const login = useCallback(
    (username: string, password: string) => loginMutation.mutateAsync({ username, password }),
    [loginMutation],
  );

  const logout = useCallback(() => logoutMutation.mutateAsync(), [logoutMutation]);

  const completeSetup = useCallback(() => {
    patch({ setupRequired: false, authEnabled: true });
  }, [patch]);

  return {
    authEnabled: data?.authEnabled ?? false,
    loggedIn: data?.loggedIn ?? false,
    setupRequired: data?.setupRequired ?? false,
    currentUser: data?.currentUser ?? null,
    loading: isPending,
    login,
    logout,
    completeSetup,
  };
}

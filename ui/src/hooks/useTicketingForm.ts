// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  deleteJiraCredential,
  fetchUserCredentials,
  RuntimeConfigError,
  putRuntimeConfig,
  setJiraCredential,
  UserCredentialsError,
} from "../api/client";
import type { TicketingSystemId, UserJiraCredentialStatus } from "../api/types";
import { useToast } from "./useToast";

interface Config {
  /** Current saved ticketing system from `/api/config`, used as the initial
   *  selection once the parent's config fetch resolves. */
  initialSystem: TicketingSystemId;
  /** Flips to `true` once the parent finished loading `/api/config`, so the
   *  selector can seed itself from the persisted value exactly once. */
  ready: boolean;
}

/** Structured error codes the Jira endpoint returns that have dedicated
 *  human-readable copy. Unmapped codes fall through to the raw message. */
const JIRA_ERROR_CODES = [
  "invalid_token",
  "unauthorized",
  "site_empty",
  "site_too_long",
  "site_invalid",
  "email_invalid",
  "jira_transport_error",
  "master_key_unavailable",
  "database_unavailable",
  "seal_failed",
  "write_failed",
] as const;

/**
 * Onboarding step-1 state machine: which ticketing system the deployment
 * should use (None / GitHub / Jira) plus the per-user Jira credential fields
 * shown when Jira is selected.
 *
 * `save()` writes `[general] ticketing_system` via `PUT /api/config` and, when
 * Jira is selected and all three credential fields are filled, posts the
 * per-user Jira credential. A half-filled Jira form blocks navigation; an
 * already-connected user can leave the form blank to keep their stored
 * credential. Returns `true` / `false` so the wizard flow can gate "Continue".
 */
export function useTicketingForm({ initialSystem, ready }: Config) {
  const { t } = useTranslation("config");
  const { showToast } = useToast();
  const jiraErrorCopy = useCallback(
    (code: string): string | undefined =>
      (JIRA_ERROR_CODES as readonly string[]).includes(code)
        ? t(`ticketing.jiraErrors.${code}`)
        : undefined,
    [t],
  );
  const [system, setSystem] = useState<TicketingSystemId>("none");
  // The last-persisted ticketing system. We only PUT /api/config when the
  // selection differs from this — so a user who leaves the system untouched
  // (e.g. a non-admin managing only their Jira credential) never triggers the
  // admin-gated config write.
  const [persistedSystem, setPersistedSystem] = useState<TicketingSystemId>("none");
  const [seeded, setSeeded] = useState(false);
  const [site, setSite] = useState("");
  const [email, setEmail] = useState("");
  const [token, setToken] = useState("");
  const [saving, setSaving] = useState(false);
  const [connected, setConnected] = useState<UserJiraCredentialStatus | null>(null);

  // Seed the selector from the persisted value once the config has loaded.
  // Guarded by `seeded` so a later re-render of the parent doesn't clobber a
  // selection the user has since changed.
  useEffect(() => {
    if (ready && !seeded) {
      setSystem(initialSystem);
      setPersistedSystem(initialSystem);
      setSeeded(true);
    }
  }, [ready, seeded, initialSystem]);

  const refreshConnected = useCallback(async () => {
    const creds = await fetchUserCredentials().catch(() => null);
    setConnected(creds?.jira ?? null);
  }, []);

  useEffect(() => {
    void refreshConnected();
  }, [refreshConnected]);

  // Pre-fill the non-secret site/email from the stored credential once it
  // loads, so an already-connected user sees their values (the token stays
  // masked). Only seeds while both inputs are still untouched/empty.
  useEffect(() => {
    if (connected && site === "" && email === "") {
      setSite(connected.site);
      setEmail(connected.email);
    }
    // Intentionally keyed on `connected` only: re-seeding on every keystroke
    // would fight the user's edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connected]);

  const hasConnected = connected !== null;
  const tokenProvided = token.trim().length > 0;
  // Compared against the stored values so a pre-filled-but-unchanged form reads
  // as "keep" (no write), and an edit reads as a non-secret change.
  const siteChanged = hasConnected ? site.trim() !== connected.site : site.trim().length > 0;
  const emailChanged = hasConnected ? email.trim() !== connected.email : email.trim().length > 0;
  const filledCount = [site, email, token].filter((v) => v.trim().length > 0).length;

  // First-time setup needs all three fields. A connected user can save any
  // combination: token alone (rotate), site/email alone (keep token, partial
  // update), or nothing (no-op).
  const jiraComplete = hasConnected
    ? tokenProvided || siteChanged || emailChanged
    : filledCount === 3;
  const jiraPartial = !hasConnected && filledCount > 0 && filledCount < 3;
  const systemChanged = system !== persistedSystem;
  // Dirty when the system selection changed, or (Jira) a field diverges from
  // the stored credential / a token was typed.
  const isDirty =
    systemChanged ||
    (system === "jira" && (tokenProvided || siteChanged || emailChanged));

  const save = useCallback(async (): Promise<boolean> => {
    if (system === "jira" && jiraPartial) {
      showToast(t("ticketing.jiraPartial"), "error");
      return false;
    }
    setSaving(true);
    try {
      if (systemChanged) {
        await putRuntimeConfig({ general: { ticketing_system: system } });
        setPersistedSystem(system);
      }
      let connectedName: string | null = null;
      if (system === "jira" && jiraComplete) {
        // KEEP/REPLACE wire convention: include `token` only when the user
        // entered a new one (REPLACE); omit it to KEEP the stored token and
        // re-validate it against the (possibly changed) site/email.
        const body: { site: string; email: string; token?: string } = {
          site: site.trim(),
          email: email.trim(),
        };
        if (tokenProvided) body.token = token;
        const saved = await setJiraCredential(body);
        await refreshConnected();
        setToken("");
        connectedName = saved.account.display_name;
      }
      // Toast the most specific thing that happened; stay silent on a no-op so
      // the wizard's "Continue" doesn't nag when nothing changed.
      if (connectedName) {
        showToast(t("ticketing.jiraConnected", { name: connectedName }), "success");
      } else if (systemChanged) {
        showToast(t("ticketing.systemSaved"), "success");
      }
      return true;
    } catch (e: unknown) {
      let msg: string;
      if (e instanceof UserCredentialsError) {
        msg = jiraErrorCopy(e.code) ?? t("errors.withCode", { message: e.message, code: e.code });
      } else if (e instanceof RuntimeConfigError) {
        msg =
          e.status === 403
            ? t("ticketing.adminOnly")
            : t("errors.withCode", { message: e.message, code: e.code });
      } else if (e instanceof Error) {
        msg = e.message;
      } else {
        msg = String(e);
      }
      showToast(msg, "error");
      return false;
    } finally {
      setSaving(false);
    }
  }, [
    system,
    systemChanged,
    jiraComplete,
    jiraPartial,
    tokenProvided,
    site,
    email,
    token,
    refreshConnected,
    showToast,
    t,
    jiraErrorCopy,
  ]);

  const disconnect = useCallback(async (): Promise<boolean> => {
    setSaving(true);
    try {
      await deleteJiraCredential();
      await refreshConnected();
      setSite("");
      setEmail("");
      setToken("");
      showToast(t("ticketing.jiraRemoved"), "success");
      return true;
    } catch (e: unknown) {
      const msg =
        e instanceof UserCredentialsError
          ? jiraErrorCopy(e.code) ?? t("errors.withCode", { message: e.message, code: e.code })
          : e instanceof Error
            ? e.message
            : String(e);
      showToast(msg, "error");
      return false;
    } finally {
      setSaving(false);
    }
  }, [refreshConnected, showToast, t, jiraErrorCopy]);

  return {
    system,
    setSystem,
    /** The last-persisted ticketing system (what's actually saved on the
     *  server), as opposed to the live `system` selection which may be an
     *  unsaved edit. Consumers that gate deployment config on the *effective*
     *  system (e.g. showing polling settings) should read this. */
    persistedSystem,
    site,
    setSite,
    email,
    setEmail,
    token,
    setToken,
    saving,
    connected,
    isDirty,
    save,
    disconnect,
  };
}

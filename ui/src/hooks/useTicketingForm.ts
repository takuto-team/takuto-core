// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback, useEffect, useState } from "react";
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

/** Human-readable copy for the structured error codes the Jira endpoint
 *  returns. Unmapped codes fall through to the raw message. */
const JIRA_ERROR_COPY: Record<string, string> = {
  invalid_token: "Jira rejected the token — check the site URL, email, and API token.",
  unauthorized: "Your Jira account isn't authorized for that site.",
  site_empty: "Enter your Atlassian site URL.",
  site_too_long: "That site URL is too long.",
  site_invalid: "Enter a full site URL starting with https://.",
  email_invalid: "Enter the email tied to your Atlassian account.",
  jira_transport_error: "Couldn't reach Jira — check the site URL and your network.",
  master_key_unavailable: "The server can't seal credentials right now. Try again later.",
  database_unavailable: "The server is unavailable right now. Try again later.",
  seal_failed: "The server couldn't store the credential. Try again.",
  write_failed: "The server couldn't store the credential. Try again.",
};

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
  const { showToast } = useToast();
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

  const filledCount = [site, email, token].filter((v) => v.trim().length > 0).length;
  const jiraComplete = filledCount === 3;
  // An already-connected user who leaves every field blank keeps their stored
  // credential — no re-post, no validation block.
  const keepingExisting = connected !== null && filledCount === 0;
  const jiraPartial = !keepingExisting && filledCount > 0 && filledCount < 3;
  const systemChanged = system !== persistedSystem;

  const save = useCallback(async (): Promise<boolean> => {
    if (system === "jira" && jiraPartial) {
      showToast(
        "Fill in the Jira site, email, and API token — or clear them to keep your current Jira connection.",
        "error",
      );
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
        const saved = await setJiraCredential({
          site: site.trim(),
          email: email.trim(),
          token,
        });
        await refreshConnected();
        setToken("");
        connectedName = saved.account.display_name;
      }
      // Toast the most specific thing that happened; stay silent on a no-op so
      // the wizard's "Continue" doesn't nag when nothing changed.
      if (connectedName) {
        showToast(`Jira connected as ${connectedName}.`, "success");
      } else if (systemChanged) {
        showToast("Ticketing system saved.", "success");
      }
      return true;
    } catch (e: unknown) {
      let msg: string;
      if (e instanceof UserCredentialsError) {
        msg = JIRA_ERROR_COPY[e.code] ?? `${e.message} (code: ${e.code})`;
      } else if (e instanceof RuntimeConfigError) {
        msg =
          e.status === 403
            ? "Only an admin can change the deployment's ticketing system. Ask an admin, or skip this step."
            : `${e.message} (code: ${e.code})`;
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
    site,
    email,
    token,
    refreshConnected,
    showToast,
  ]);

  const disconnect = useCallback(async (): Promise<boolean> => {
    setSaving(true);
    try {
      await deleteJiraCredential();
      await refreshConnected();
      setSite("");
      setEmail("");
      setToken("");
      showToast("Jira credential removed.", "success");
      return true;
    } catch (e: unknown) {
      const msg =
        e instanceof UserCredentialsError
          ? JIRA_ERROR_COPY[e.code] ?? `${e.message} (code: ${e.code})`
          : e instanceof Error
            ? e.message
            : String(e);
      showToast(msg, "error");
      return false;
    } finally {
      setSaving(false);
    }
  }, [refreshConnected, showToast]);

  return {
    system,
    setSystem,
    site,
    setSite,
    email,
    setEmail,
    token,
    setToken,
    saving,
    connected,
    save,
    disconnect,
  };
}

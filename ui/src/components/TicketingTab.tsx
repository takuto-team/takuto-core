// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Configuration → "Ticketing" tab. Lets a user manage the deployment ticketing
 * system (admin-gated write) and their own per-user Jira credential. Reuses the
 * wizard step-1 surface: `TicketingStep` for rendering and `useTicketingForm`
 * for the selector / credential state + save logic.
 *
 * Non-admins see the system selector read-only but can still set, rotate, or
 * remove their personal Jira credential. The admin-gated `PUT /api/config`
 * write only fires when the system selection actually changes, so a non-admin
 * managing only their credential never trips the 403.
 *
 * Three settings sections live on this same page below the ticketing controls:
 *  - Per-user-per-repository polling settings (`RepoPollingSettingsSection`),
 *    shown to every user when a ticketing system is selected.
 *  - Deployment-global Jira-context processing fields (`GlobalJiraContextSection`),
 *    admin-only, shown when Jira is the active system.
 *  - Deployment-global "general limits" (`GeneralLimitsSection`), admin-only.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { apiJson } from "../api/client";
import type { ConfigResponse, TicketingSystemId } from "../api/types";
import { TicketingStep } from "../pages/Onboarding/TicketingStep";
import { useTicketingForm } from "../hooks/useTicketingForm";
import { RepoPollingSettingsSection } from "./admin/RepoPollingSettingsSection";
import { GlobalJiraContextSection } from "./admin/GlobalJiraContextSection";
import { GeneralLimitsSection } from "./admin/GeneralLimitsSection";
import type { ConfigSectionHandle } from "./admin/configSection";

interface Props {
  isAdmin?: boolean;
  /** Reports combined dirty (ticketing + polling) so Config's footer enables. */
  onDirtyChange?: (dirty: boolean) => void;
  /** Registers this tab's "save all" fn so the page-level Save can drive it. */
  registerSave?: (save: () => Promise<boolean>) => void;
}

export function TicketingTab({ isAdmin, onDirtyChange, registerSave }: Props) {
  const { t } = useTranslation("config");
  const [initialSystem, setInitialSystem] = useState<TicketingSystemId>("none");
  const [loading, setLoading] = useState(true);
  const repoPollingRef = useRef<ConfigSectionHandle>(null);
  const [repoPollingDirty, setRepoPollingDirty] = useState(false);
  const jiraContextRef = useRef<ConfigSectionHandle>(null);
  const [jiraContextDirty, setJiraContextDirty] = useState(false);
  const generalLimitsRef = useRef<ConfigSectionHandle>(null);
  const [generalLimitsDirty, setGeneralLimitsDirty] = useState(false);

  useEffect(() => {
    let mounted = true;
    apiJson<ConfigResponse>("/api/config")
      .then((c) => {
        if (mounted) {
          setInitialSystem((c.ticketing_system as TicketingSystemId) ?? "none");
        }
      })
      .catch(() => {
        // Leave the default ("none"); the selector still works and the save
        // path surfaces any server error.
      })
      .finally(() => {
        if (mounted) setLoading(false);
      });
    return () => {
      mounted = false;
    };
  }, []);

  const ticketing = useTicketingForm({ initialSystem, ready: !loading });

  const showDisconnect = ticketing.system === "jira" && ticketing.connected !== null;
  // Per-user-per-repo polling — shown to every user (no admin gate) once a
  // ticketing system is selected. Gate on the live selection so choosing "None"
  // hides it immediately, without waiting for a save.
  const showRepoPolling = !loading && ticketing.system !== "none";
  // Deployment-global Jira-context processing fields — admin-only, shown when
  // Jira is the active system (these patch [jira] via PUT /api/config/jira).
  const showJiraContext = !loading && !!isAdmin && ticketing.system === "jira";
  // Deployment-global general limits — admin-only, deployment-wide (apply even
  // with no ticketing system), so gate on admin alone.
  const showGeneralLimits = !loading && !!isAdmin;

  // Combined dirty + a single saver, folded into the page-level Save footer.
  const effectiveRepoPollingDirty = showRepoPolling && repoPollingDirty;
  const effectiveJiraContextDirty = showJiraContext && jiraContextDirty;
  const effectiveGeneralLimitsDirty = showGeneralLimits && generalLimitsDirty;
  const dirty =
    ticketing.isDirty ||
    effectiveRepoPollingDirty ||
    effectiveJiraContextDirty ||
    effectiveGeneralLimitsDirty;
  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);

  const saveAll = useCallback(async (): Promise<boolean> => {
    const ok = await ticketing.save();
    if (!ok) return false;
    const repoOk = repoPollingRef.current ? await repoPollingRef.current.save() : true;
    if (!repoOk) return false;
    const jiraOk = jiraContextRef.current ? await jiraContextRef.current.save() : true;
    if (!jiraOk) return false;
    return generalLimitsRef.current ? generalLimitsRef.current.save() : true;
  }, [ticketing]);
  useEffect(() => {
    registerSave?.(saveAll);
  }, [registerSave, saveAll]);

  return (
    <section aria-labelledby="ticketing-tab-title" className="flex flex-col gap-8">
      <div className="flex flex-col gap-4 max-w-2xl">
        <div>
          <h2 id="ticketing-tab-title" className="text-lg font-semibold text-white">
            {t("ticketing.title")}
          </h2>
          <p className="text-sm text-gray-500 mt-1">
            {t("ticketing.description")}
          </p>
        </div>

        {loading ? (
          <p className="text-sm text-gray-500">{t("actions.loading")}</p>
        ) : (
          <>
            <TicketingStep
              system={ticketing.system}
              onChangeSystem={ticketing.setSystem}
              site={ticketing.site}
              onChangeSite={ticketing.setSite}
              email={ticketing.email}
              onChangeEmail={ticketing.setEmail}
              token={ticketing.token}
              onChangeToken={ticketing.setToken}
              connected={ticketing.connected}
              canEditSystem={!!isAdmin}
            />

            {/* The Save is the page-level footer; only the discrete Disconnect
                action stays inline here. */}
            {showDisconnect && (
              <div className="flex items-center gap-3">
                <button
                  type="button"
                  onClick={() => void ticketing.disconnect()}
                  disabled={ticketing.saving}
                  className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
                >
                  {t("ticketing.disconnectJira")}
                </button>
              </div>
            )}
          </>
        )}
      </div>

      {showRepoPolling && (
        <div className="border-t border-gray-800 pt-6">
          <RepoPollingSettingsSection
            ref={repoPollingRef}
            ticketingSystem={ticketing.system}
            onDirtyChange={setRepoPollingDirty}
          />
        </div>
      )}

      {showJiraContext && (
        <div className="border-t border-gray-800 pt-6">
          <GlobalJiraContextSection ref={jiraContextRef} onDirtyChange={setJiraContextDirty} />
        </div>
      )}

      {showGeneralLimits && (
        <div className="border-t border-gray-800 pt-6">
          <GeneralLimitsSection ref={generalLimitsRef} onDirtyChange={setGeneralLimitsDirty} />
        </div>
      )}
    </section>
  );
}

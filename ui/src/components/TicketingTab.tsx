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
 */

import { useEffect, useState } from "react";
import { apiJson } from "../api/client";
import type { ConfigResponse, TicketingSystemId } from "../api/types";
import { TicketingStep } from "../pages/Onboarding/TicketingStep";
import { useTicketingForm } from "../hooks/useTicketingForm";

interface Props {
  isAdmin?: boolean;
}

export function TicketingTab({ isAdmin }: Props) {
  const [initialSystem, setInitialSystem] = useState<TicketingSystemId>("none");
  const [loading, setLoading] = useState(true);

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

  return (
    <section aria-labelledby="ticketing-tab-title" className="flex flex-col gap-4 max-w-2xl">
      <div>
        <h2 id="ticketing-tab-title" className="text-lg font-semibold text-white">
          Ticketing
        </h2>
        <p className="text-sm text-gray-500 mt-1">
          Choose where Takuto reads work items from, and connect your personal
          Jira account.
        </p>
      </div>

      {loading ? (
        <p className="text-sm text-gray-500">Loading…</p>
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

          <div className="flex items-center gap-3">
            <button
              type="button"
              onClick={() => void ticketing.save()}
              disabled={ticketing.saving}
              className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
            >
              {ticketing.saving ? "Saving…" : "Save"}
            </button>
            {showDisconnect && (
              <button
                type="button"
                onClick={() => void ticketing.disconnect()}
                disabled={ticketing.saving}
                className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
              >
                Disconnect Jira
              </button>
            )}
          </div>
        </>
      )}
    </section>
  );
}

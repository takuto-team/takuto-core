// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";

interface Props {
  /**
   * The active repository's `auto_polling` setting — the SINGLE source of
   * truth is the per-repo "Enable items auto polling" toggle in Ticketing
   * settings. This label only reflects it.
   */
  autoPolling: boolean;
  ticketingSystem: string;
}

/** Config → Ticketing deep link (matches the `?tab=` slugs Config parses). */
const TICKETING_TAB_PATH = "/config.html?tab=ticketing";

export function PollingLabel({ autoPolling, ticketingSystem }: Props) {
  const { t } = useTranslation("dashboard");
  if (ticketingSystem === "none") return null;

  return (
    <div className="w-full bg-gray-900/60 border-b border-gray-800 px-4 py-1.5 flex items-center justify-center">
      <Link
        to={TICKETING_TAB_PATH}
        title={t("polling.openSettings")}
        className={
          autoPolling
            ? "text-xs text-emerald-500/70 hover:text-emerald-400 transition-colors"
            : "text-xs text-amber-400/80 hover:text-amber-300 transition-colors"
        }
      >
        {autoPolling ? t("polling.active") : t("polling.off")}
      </Link>
    </div>
  );
}

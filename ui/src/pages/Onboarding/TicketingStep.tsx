// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { Trans, useTranslation } from "react-i18next";
import type { TicketingSystemId, UserJiraCredentialStatus } from "../../api/types";

const TICKETING_OPTION_IDS: TicketingSystemId[] = ["none", "github", "jira"];

interface Props {
  system: TicketingSystemId;
  onChangeSystem: (s: TicketingSystemId) => void;
  site: string;
  onChangeSite: (v: string) => void;
  email: string;
  onChangeEmail: (v: string) => void;
  token: string;
  onChangeToken: (v: string) => void;
  connected: UserJiraCredentialStatus | null;
  /** When `false`, the system selector is read-only (the deployment-wide
   *  ticketing system is admin-gated). Defaults to `true`. */
  canEditSystem?: boolean;
}

export function TicketingStep({
  system,
  onChangeSystem,
  site,
  onChangeSite,
  email,
  onChangeEmail,
  token,
  onChangeToken,
  connected,
  canEditSystem = true,
}: Props) {
  const { t } = useTranslation("onboarding");
  const activeHint = t(`ticketing.options.${system}.hint`);

  return (
    <div className="flex flex-col gap-4">
      <div>
        <label htmlFor="onb-ticketing" className="block text-xs text-gray-400 mb-1">
          {t("ticketing.label")}
        </label>
        <select
          id="onb-ticketing"
          value={system}
          onChange={(e) => onChangeSystem(e.target.value as TicketingSystemId)}
          disabled={!canEditSystem}
          className={`w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm ${
            canEditSystem ? "text-gray-200" : "text-gray-500 cursor-not-allowed"
          }`}
        >
          {TICKETING_OPTION_IDS.map((id) => (
            <option key={id} value={id}>
              {t(`ticketing.options.${id}.label`)}
            </option>
          ))}
        </select>
        <p className="text-xs text-gray-500 mt-1">{activeHint}</p>
        {!canEditSystem && (
          <p className="text-xs text-gray-500 mt-1">{t("ticketing.adminOnly")}</p>
        )}
      </div>

      {system === "jira" && (
        <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 flex flex-col gap-3">
          {connected && (
            <p className="text-sm text-green-400">
              <Trans
                i18nKey="ticketing.jira.connectedAs"
                ns="onboarding"
                values={{ email: connected.email, site: connected.site }}
                components={{ strong: <strong />, site: <span className="font-mono" /> }}
              />
            </p>
          )}
          <p className="text-sm text-gray-300">{t("ticketing.jira.encryptedNote")}</p>
          <div>
            <label htmlFor="onb-jira-site" className="block text-xs text-gray-400 mb-1">
              {t("ticketing.jira.site")}
            </label>
            <input
              id="onb-jira-site"
              type="text"
              value={site}
              onChange={(e) => onChangeSite(e.target.value)}
              placeholder={t("ticketing.jira.sitePlaceholder")}
              className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
            />
          </div>
          <div>
            <label htmlFor="onb-jira-email" className="block text-xs text-gray-400 mb-1">
              {t("ticketing.jira.email")}
            </label>
            <input
              id="onb-jira-email"
              type="email"
              value={email}
              onChange={(e) => onChangeEmail(e.target.value)}
              placeholder={t("ticketing.jira.emailPlaceholder")}
              className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
            />
          </div>
          <div>
            <label htmlFor="onb-jira-token" className="block text-xs text-gray-400 mb-1">
              {t("ticketing.jira.token")}
            </label>
            <input
              id="onb-jira-token"
              type="password"
              value={token}
              onChange={(e) => onChangeToken(e.target.value)}
              placeholder={t("ticketing.jira.tokenPlaceholder")}
              autoComplete="off"
              className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
            />
            <p className="text-xs text-gray-500 mt-1">
              {t("ticketing.jira.createTokenPrefix")}{" "}
              <a
                href="https://id.atlassian.com/manage-profile/security/api-tokens"
                target="_blank"
                rel="noopener noreferrer"
                className="text-blue-400 hover:text-blue-300"
                aria-label={t("ticketing.jira.tokenLinkAria")}
              >
                {t("ticketing.jira.tokenLinkText")}
              </a>
            </p>
          </div>
        </div>
      )}
    </div>
  );
}

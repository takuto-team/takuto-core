// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Onboarding wizard — `/onboarding`.
 *
 * Step shell + nav controls. Step bodies live in `./Onboarding/*.tsx`;
 * the wizard navigation state machine is in `../hooks/useOnboardingFlow`,
 * the provider-form state + `/api/config` fetch are in `../hooks/useProviderForm`,
 * the step-1 ticketing selector state is in `../hooks/useTicketingForm`,
 * the step-3 git settings in `../hooks/useGitForm`, and the step-5 step-timeout
 * in `../hooks/useStepTimeoutForm`. All of them seed from the single
 * `/api/config` fetch in `useProviderForm`.
 *
 * 5 steps:
 *   1. Ticketing      — selector (None / GitHub / Jira); Jira shows a
 *                        site/email/token form saved per-user. Writes
 *                        `[general] ticketing_system` via PUT /api/config.
 *                        Admins also see the deployment item-polling section
 *                        (PUT /api/config/polling) when a system is selected.
 *   2. AI provider    — provider form (PUT /api/config/agent) + inline AI
 *                        API-key entry (AiCredentialPanel).
 *   3. Git & GitHub   — git base branch + remote (PUT /api/config/git),
 *                        GitHub App status, and an optional per-user PAT.
 *   4. Repositories   — add the GitHub repos Takuto can work in
 *                        (MyRepositoriesTab) so step 5 has repos to attach
 *                        flows to. Adds/removes persist via their own buttons.
 *   5. Workflows      — step timeout (PUT /api/config/agent) + the per-user /
 *                        per-workspace flows editor (FlowsTab).
 *
 * Each step has Skip / Back / Continue; the last step has Finish instead of
 * Continue. "Finish" calls `POST /api/onboarding/complete` and navigates back
 * to the dashboard.
 */

import { useRef } from "react";
import { Link } from "react-router-dom";
import { Trans, useTranslation } from "react-i18next";
import { useOnboardingFlow } from "../hooks/useOnboardingFlow";
import type { AiCredentialPanelHandle } from "../components/credentials/AiCredentialPanel";
import type { GitHubCredentialPanelHandle } from "../components/credentials/GitHubCredentialPanel";
import { useProviderForm } from "../hooks/useProviderForm";
import { useTicketingForm } from "../hooks/useTicketingForm";
import { useGitForm } from "../hooks/useGitForm";
import { useStepTimeoutForm } from "../hooks/useStepTimeoutForm";
import { FlowsTab } from "../components/FlowsTab";
import {
  ItemPollingSettingsSection,
  type ItemPollingSettingsHandle,
} from "../components/admin/ItemPollingSettingsSection";
import { MyRepositoriesTab } from "../components/MyRepositoriesTab";
import type { TicketingSystemId } from "../api/types";
import { GitHubStep } from "./Onboarding/GitHubStep";
import { OnboardingAiKey } from "./Onboarding/OnboardingAiKey";
import { ONBOARDING_STEPS, Stepper } from "./Onboarding/Stepper";
import { ProviderStep } from "./Onboarding/ProviderStep";
import { TicketingStep } from "./Onboarding/TicketingStep";

interface Props {
  onLogout: () => void;
  authEnabled: boolean;
  isAdmin?: boolean;
}

export function Onboarding({ onLogout, authEnabled, isAdmin }: Props) {
  const { t } = useTranslation("onboarding");
  const {
    loading,
    saving,
    provider,
    setProvider,
    baseUrl,
    setBaseUrl,
    model,
    setModel,
    extraArgsText,
    setExtraArgsText,
    ticketingSystem,
    githubAppConfigured,
    gitBaseBranch,
    gitRemote,
    stepTimeoutSecs,
    save,
  } = useProviderForm();

  const ticketing = useTicketingForm({
    initialSystem: ticketingSystem as TicketingSystemId,
    ready: !loading,
  });

  const git = useGitForm({
    initialBaseBranch: gitBaseBranch,
    initialRemote: gitRemote,
    ready: !loading,
    canSave: !!isAdmin,
  });

  const timeout = useStepTimeoutForm({ initialSecs: stepTimeoutSecs, ready: !loading });

  // Imperative handles into the inline credential panels so "Continue" also
  // persists the per-user AI key (step 2) and GitHub PAT (step 3) the user
  // typed — the provider/git PUTs alone do not carry those credentials.
  const aiKeyRef = useRef<AiCredentialPanelHandle>(null);
  const githubPatRef = useRef<GitHubCredentialPanelHandle>(null);
  const pollingRef = useRef<ItemPollingSettingsHandle>(null);

  const { step, completing, goNext, goSkip, goBack } = useOnboardingFlow({
    onBeforeNext: async (s) => {
      if (s === 1) {
        const ok = await ticketing.save();
        if (!ok) return false;
        // Persist the embedded item-polling section too (it has no own Save in
        // the wizard) so toggles like "disable polling" are actually saved.
        return pollingRef.current ? pollingRef.current.save() : true;
      }
      if (s === 2) {
        const provider = await save();
        if (!provider) return false;
        // Blank key → saveIfDirty resolves true (skip / deployment default).
        return aiKeyRef.current ? aiKeyRef.current.saveIfDirty() : true;
      }
      if (s === 3) {
        // Persist the per-user PAT first, then the deployment git settings.
        // A typed PAT that fails to save (e.g. a GitHub transport error)
        // must block the step BEFORE git is saved — otherwise the user sees
        // a misleading "Git settings saved." success toast alongside the PAT
        // error in the same Continue. A blank PAT resolves true (no-op), so
        // the common case still saves git and shows its own success toast.
        const patOk = githubPatRef.current
          ? await githubPatRef.current.saveIfDirty()
          : true;
        if (!patOk) return false;
        return git.save();
      }
      // Step 4 (repositories) saves via its own Add/Remove buttons — nothing
      // to persist on Continue.
      if (s === 5) return timeout.save();
      return true;
    },
  });

  // Deployment-level polling settings only apply once a ticketing system is
  // selected, and only admins may edit them. Gate on the live selection so
  // choosing "None" hides the section immediately (matching TicketingTab).
  const showPolling = !loading && !!isAdmin && ticketing.system !== "none";

  return (
    <div className="min-h-screen flex flex-col">
      <header className="border-b border-gray-800 bg-gray-950/80 backdrop-blur-sm sticky top-0 z-40">
        <div className="w-full px-4 sm:px-6 lg:px-8">
          <div className="flex items-center justify-between h-14">
            <Link
              to="/"
              className="flex items-center gap-2 text-gray-400 hover:text-gray-200 transition-colors text-sm"
            >
              {t("header.skip")}
            </Link>
            <span className="text-lg font-bold text-white">{t("header.title")}</span>
            {authEnabled && (
              <button
                onClick={onLogout}
                className="text-xs text-gray-500 hover:text-gray-300 cursor-pointer"
              >
                {t("common:nav.logout")}
              </button>
            )}
          </div>
        </div>
      </header>

      <main className="flex-1 w-full px-4 sm:px-6 lg:px-8 py-8 flex flex-col gap-6">
        <Stepper current={step} />

        <div className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-4">
          <div>
            <h2 className="text-lg font-semibold text-white">
              {t(ONBOARDING_STEPS[step - 1].titleKey)}
            </h2>
            <p className="text-sm text-gray-400 mt-1">{t(ONBOARDING_STEPS[step - 1].bodyKey)}</p>
          </div>

          {loading ? (
            <p className="text-sm text-gray-500">{t("loading")}</p>
          ) : (
            <>
              {step === 1 && (
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
                  {showPolling && (
                    <div className="border-t border-gray-800 pt-6">
                      <p className="text-xs text-gray-500 mb-4">{t("polling.note")}</p>
                      <ItemPollingSettingsSection ref={pollingRef} />
                    </div>
                  )}
                </>
              )}
              {step === 2 && (
                <div className="flex flex-col gap-6">
                  <ProviderStep
                    provider={provider}
                    onChangeProvider={setProvider}
                    baseUrl={baseUrl}
                    onChangeBaseUrl={setBaseUrl}
                    model={model}
                    onChangeModel={setModel}
                    extraArgsText={extraArgsText}
                    onChangeExtraArgs={setExtraArgsText}
                  />
                  <OnboardingAiKey ref={aiKeyRef} provider={provider} />
                </div>
              )}
              {step === 3 && (
                <GitHubStep
                  githubAppConfigured={githubAppConfigured}
                  baseBranch={git.baseBranch}
                  onChangeBaseBranch={git.setBaseBranch}
                  remote={git.remote}
                  onChangeRemote={git.setRemote}
                  baseBranchInvalid={git.baseBranchInvalid}
                  remoteInvalid={git.remoteInvalid}
                  canEditGit={!!isAdmin}
                  patPanelRef={githubPatRef}
                />
              )}
              {step === 4 && <MyRepositoriesTab isAdmin={isAdmin} />}
              {step === 5 && (
                <div className="flex flex-col gap-4">
                  <div className="flex flex-col gap-3">
                    <h3 className="text-sm font-semibold text-gray-300 mb-1">{t("stepTimeout.heading")}</h3>
                    <div className="max-w-xs">
                      <label htmlFor="onb-step-timeout" className="block text-xs text-gray-400 mb-1">
                        {t("stepTimeout.label")}
                      </label>
                      <input
                        id="onb-step-timeout"
                        type="number"
                        min={1}
                        value={timeout.value}
                        onChange={(e) => timeout.setValue(e.target.value)}
                        placeholder="1800"
                        className={`w-full bg-gray-950 border rounded-lg px-3 py-2 text-sm text-gray-200 ${
                          timeout.invalid ? "border-red-500" : "border-gray-700"
                        }`}
                      />
                      {timeout.invalid ? (
                        <p className="text-xs text-red-400 mt-1">{t("stepTimeout.invalid")}</p>
                      ) : (
                        <p className="text-xs text-gray-500 mt-1">{t("stepTimeout.hint")}</p>
                      )}
                    </div>
                  </div>
                  <div className="border-t border-gray-800 pt-4">
                    <FlowsTab />
                  </div>
                </div>
              )}
            </>
          )}

          {step === 5 && (
            <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-3 text-xs text-gray-400">
              <Trans
                i18nKey="dbNote"
                ns="onboarding"
                components={{ strong: <strong />, code: <code className="font-mono" /> }}
              />
            </div>
          )}

          <div className="flex justify-between items-center mt-2">
            <button
              type="button"
              onClick={goBack}
              disabled={step === 1}
              className="text-xs text-gray-400 hover:text-gray-200 disabled:opacity-30 disabled:cursor-not-allowed cursor-pointer"
            >
              {t("nav.back")}
            </button>
            <div className="flex gap-3">
              <button
                type="button"
                onClick={goSkip}
                className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
              >
                {t("nav.skip")}
              </button>
              <button
                type="button"
                onClick={goNext}
                disabled={
                  saving ||
                  ticketing.saving ||
                  git.saving ||
                  timeout.saving ||
                  completing ||
                  // Self-hosted spec (2026-05-27 §2.5): block Continue on
                  // step 2 when OpenCode is selected without base_url +
                  // model. The server returns 400 in that case; the
                  // client-side guard makes the requirement visible up
                  // front. The "Skip for now" button stays enabled — the
                  // operator can come back to AI Settings later.
                  (step === 2 &&
                    provider === "opencode" &&
                    (baseUrl.trim() === "" || model.trim() === ""))
                }
                className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
              >
                {step === 5
                  ? completing
                    ? t("nav.finishing")
                    : t("nav.finish")
                  : saving || ticketing.saving || git.saving
                    ? t("nav.saving")
                    : t("nav.continue")}
              </button>
            </div>
          </div>
        </div>
      </main>
    </div>
  );
}

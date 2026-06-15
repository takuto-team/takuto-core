// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Onboarding wizard — `/onboarding`.
 *
 * Step shell + nav controls. Step bodies live in `./Onboarding/*.tsx`;
 * the wizard navigation state machine is in `../hooks/useOnboardingFlow`,
 * the provider-form state + `/api/config` fetch are in `../hooks/useProviderForm`,
 * and the step-1 ticketing selector state is in `../hooks/useTicketingForm`.
 *
 * 4 steps:
 *   1. Ticketing      — selector (None / GitHub / Jira); Jira shows a
 *                        site/email/token form saved per-user. Writes
 *                        `[general] ticketing_system` via PUT /api/config.
 *   2. AI provider    — provider form (PUT /api/config/agent) + inline AI
 *                        API-key entry (AiCredentialPanel).
 *   3. GitHub         — optional per-user PAT (GitHubCredentialsSection) +
 *                        a "Set up a GitHub App" doc link.
 *   4. Workflows      — the per-user/per-workspace flows editor (FlowsTab).
 *
 * Each step has Skip / Back / Continue; the last step has Finish instead of
 * Continue. "Finish" calls `POST /api/onboarding/complete` and navigates back
 * to the dashboard.
 */

import { Link } from "react-router-dom";
import { useOnboardingFlow } from "../hooks/useOnboardingFlow";
import { useProviderForm } from "../hooks/useProviderForm";
import { useTicketingForm } from "../hooks/useTicketingForm";
import { FlowsTab } from "../components/FlowsTab";
import type { TicketingSystemId } from "../api/types";
import { GitHubStep } from "./Onboarding/GitHubStep";
import { OnboardingAiKey } from "./Onboarding/OnboardingAiKey";
import { ONBOARDING_STEPS, Stepper } from "./Onboarding/Stepper";
import { ProviderStep } from "./Onboarding/ProviderStep";
import { TicketingStep } from "./Onboarding/TicketingStep";

interface Props {
  onLogout: () => void;
  authEnabled: boolean;
}

export function Onboarding({ onLogout, authEnabled }: Props) {
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
    save,
  } = useProviderForm();

  const ticketing = useTicketingForm({
    initialSystem: ticketingSystem as TicketingSystemId,
    ready: !loading,
  });

  const { step, completing, goNext, goSkip, goBack } = useOnboardingFlow({
    onBeforeNext: async (s) => {
      if (s === 1) return ticketing.save();
      if (s === 2) return save();
      return true;
    },
  });

  return (
    <div className="min-h-screen flex flex-col">
      <header className="border-b border-gray-800 bg-gray-950/80 backdrop-blur-sm sticky top-0 z-40">
        <div className="w-full px-4 sm:px-6 lg:px-8">
          <div className="flex items-center justify-between h-14">
            <Link
              to="/"
              className="flex items-center gap-2 text-gray-400 hover:text-gray-200 transition-colors text-sm"
            >
              Skip setup &rarr;
            </Link>
            <span className="text-lg font-bold text-white">Set up Takuto</span>
            {authEnabled && (
              <button
                onClick={onLogout}
                className="text-xs text-gray-500 hover:text-gray-300 cursor-pointer"
              >
                Log out
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
              {ONBOARDING_STEPS[step - 1].title}
            </h2>
            <p className="text-sm text-gray-400 mt-1">{ONBOARDING_STEPS[step - 1].body}</p>
          </div>

          {loading ? (
            <p className="text-sm text-gray-500">Loading current settings…</p>
          ) : (
            <>
              {step === 1 && (
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
                />
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
                  <OnboardingAiKey provider={provider} />
                </div>
              )}
              {step === 3 && <GitHubStep githubAppConfigured={githubAppConfigured} />}
              {step === 4 && <FlowsTab />}
            </>
          )}

          <div className="flex justify-between items-center mt-2">
            <button
              type="button"
              onClick={goBack}
              disabled={step === 1}
              className="text-xs text-gray-400 hover:text-gray-200 disabled:opacity-30 disabled:cursor-not-allowed cursor-pointer"
            >
              &larr; Back
            </button>
            <div className="flex gap-3">
              <button
                type="button"
                onClick={goSkip}
                className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
              >
                Skip for now
              </button>
              <button
                type="button"
                onClick={goNext}
                disabled={
                  saving ||
                  ticketing.saving ||
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
                {step === 4
                  ? completing
                    ? "Finishing…"
                    : "Finish setup"
                  : saving || ticketing.saving
                    ? "Saving…"
                    : "Continue"}
              </button>
            </div>
          </div>
        </div>
      </main>
    </div>
  );
}

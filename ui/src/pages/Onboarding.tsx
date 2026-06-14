// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Onboarding wizard — `/onboarding`.
 *
 * Step shell + nav controls. Step bodies live in `./Onboarding/*.tsx`;
 * the wizard navigation state machine is in `../hooks/useOnboardingFlow`,
 * and the provider-form's local state + `/api/config` fetch are in
 * `../hooks/useProviderForm`.
 *
 * 4 steps per 04_architecture.md §9:
 *   1. Ticketing system  — read-only display (changes go through config.toml today)
 *   2. AI provider       — delegates to <ProviderForm> in AdminAiSettings (lifted)
 *   3. GitHub integration — read-only display
 *   4. Your credentials   — placeholder card
 *
 * Each step has Skip / Back / Continue; the last step has Finish instead of
 * Continue. Skip is a no-op today; "Finish" calls
 * `POST /api/onboarding/complete` and navigates back to the dashboard.
 */

import { Link } from "react-router-dom";
import { useOnboardingFlow } from "../hooks/useOnboardingFlow";
import { useProviderForm } from "../hooks/useProviderForm";
import { CredentialsStep } from "./Onboarding/CredentialsStep";
import { GitHubStep } from "./Onboarding/GitHubStep";
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

  const { step, completing, goNext, goSkip, goBack } = useOnboardingFlow({
    onBeforeNext: async (s) => {
      if (s !== 2) return true;
      return save();
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
            <span className="text-lg font-bold text-white">Set up Maestro</span>
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
              {step === 1 && <TicketingStep ticketingSystem={ticketingSystem} />}
              {step === 2 && (
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
              )}
              {step === 3 && <GitHubStep githubAppConfigured={githubAppConfigured} />}
              {step === 4 && <CredentialsStep />}
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
                  : saving
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

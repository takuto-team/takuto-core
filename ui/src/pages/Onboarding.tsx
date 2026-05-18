// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Phase 1 onboarding wizard — `/onboarding`.
 *
 * 4 steps per 04_architecture.md §9:
 *   1. Ticketing system  — read-only display (changes go through config.toml today)
 *   2. AI provider       — delegates to <ProviderForm> in AdminAiSettings (lifted)
 *   3. GitHub integration — read-only display (PAT capture is Phase 2)
 *   4. Your credentials   — placeholder card (Phase 2 wires per-user creds)
 *
 * Each step has Skip / Back / Continue; the last step has Finish instead of
 * Continue. Skip writes nothing in Phase 1 — Phase 2 hooks
 * `POST /api/onboarding/skip` here. "Finish" calls
 * `POST /api/onboarding/complete` and navigates back to the dashboard.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { apiJson, apiPost, putAgentConfig, AgentConfigError } from "../api/client";
import { useToast } from "../hooks/useToast";
import type {
  AgentConfig,
  AgentConfigPatch,
  AgentProviderId,
  ConfigResponse,
} from "../api/types";

interface Props {
  onLogout: () => void;
  authEnabled: boolean;
}

type StepIndex = 1 | 2 | 3 | 4;

const STEPS: { index: StepIndex; title: string; body: string }[] = [
  {
    index: 1,
    title: "Ticketing",
    body: "Pick where Maestro should read tasks from. You can change this later.",
  },
  {
    index: 2,
    title: "AI provider",
    body: "Choose the AI that drives your workflows. Each teammate brings their own login on top of this.",
  },
  {
    index: 3,
    title: "GitHub integration",
    body: "Connect a GitHub App for shared access, or skip and have each teammate bring a personal token.",
  },
  {
    index: 4,
    title: "Your credentials",
    body: "Add your own AI provider key and GitHub token so you can run workflows immediately.",
  },
];

const V1_PROVIDERS: AgentProviderId[] = ["claude", "cursor", "codex", "opencode"];
const PROVIDER_LABEL: Record<AgentProviderId, string> = {
  claude: "Claude",
  cursor: "Cursor",
  codex: "Codex",
  opencode: "OpenCode",
  gemini: "Gemini (v2)",
  none: "None",
};

export function Onboarding({ onLogout, authEnabled }: Props) {
  const navigate = useNavigate();
  const { showToast } = useToast();
  const [step, setStep] = useState<StepIndex>(1);
  const [config, setConfig] = useState<ConfigResponse | null>(null);
  const [loading, setLoading] = useState(true);
  // Step 2 controlled state (provider + a single base URL field — kept thin to
  // avoid duplicating the full ProviderForm here; the admin AI Settings page
  // is the place to fine-tune extras).
  const [provider, setProvider] = useState<AgentProviderId>("claude");
  const [baseUrl, setBaseUrl] = useState("");
  const [extraArgsText, setExtraArgsText] = useState("");
  const [savingProvider, setSavingProvider] = useState(false);
  const [completing, setCompleting] = useState(false);

  useEffect(() => {
    apiJson<ConfigResponse>("/api/config")
      .then((c) => {
        setConfig(c);
        const agent = (c.agent ?? {}) as AgentConfig;
        const p: AgentProviderId = (agent.provider ?? "claude") as AgentProviderId;
        setProvider(p);
        const sub = agent.providers?.[p as keyof typeof agent.providers] as
          | { base_url?: string; extra_args?: string[] }
          | undefined;
        setBaseUrl(sub?.base_url ?? "");
        setExtraArgsText((sub?.extra_args ?? []).join("\n"));
      })
      .catch(() => {
        // Server might still be coming up — wizard remains usable but the
        // step-2 form will save against blank defaults.
      })
      .finally(() => setLoading(false));
  }, []);

  const ticketingSystem = useMemo(
    () => (config?.ticketing_system ?? "none"),
    [config],
  );
  const githubAppConfigured = useMemo(
    () => Boolean(config?.github_app_configured),
    [config],
  );

  const saveProviderStep = useCallback(async () => {
    setSavingProvider(true);
    const extraArgs = extraArgsText
      .split("\n")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    const sub: Record<string, unknown> = {
      base_url: baseUrl,
      extra_args: extraArgs,
    };
    if (provider === "cursor") {
      // Cursor has no base_url field — drop it to satisfy the server's
      // `deny_unknown_fields` (04_architecture.md §2.2 amendment A1).
      delete sub.base_url;
    }
    const patch: AgentConfigPatch = {
      provider,
      providers: { [provider]: sub } as AgentConfigPatch["providers"],
    };
    try {
      await putAgentConfig(patch);
      showToast("Provider configured.", "success");
      return true;
    } catch (e: unknown) {
      const msg =
        e instanceof AgentConfigError
          ? `${e.message} (code: ${e.code})`
          : e instanceof Error
            ? e.message
            : String(e);
      showToast(msg, "error");
      return false;
    } finally {
      setSavingProvider(false);
    }
  }, [provider, baseUrl, extraArgsText, showToast]);

  const completeWizard = useCallback(async () => {
    setCompleting(true);
    try {
      // Phase 2 wires the actual server endpoint; in Phase 1 this is a
      // best-effort call — the wizard still navigates home on 404 / 401.
      await apiPost("/api/onboarding/complete");
    } catch {
      // Swallow — server doesn't have to support the endpoint yet.
    } finally {
      setCompleting(false);
    }
    navigate("/");
  }, [navigate]);

  const goNext = useCallback(async () => {
    if (step === 2) {
      const ok = await saveProviderStep();
      if (!ok) return;
    }
    if (step === 4) {
      await completeWizard();
      return;
    }
    setStep((s) => (s + 1) as StepIndex);
  }, [step, saveProviderStep, completeWizard]);

  const goSkip = useCallback(() => {
    if (step === 4) {
      void completeWizard();
      return;
    }
    setStep((s) => (s + 1) as StepIndex);
  }, [step, completeWizard]);

  const goBack = useCallback(() => {
    setStep((s) => (s > 1 ? ((s - 1) as StepIndex) : s));
  }, []);

  return (
    <div className="min-h-screen flex flex-col">
      <header className="border-b border-gray-800 bg-gray-950/80 backdrop-blur-sm sticky top-0 z-40">
        <div className="max-w-3xl mx-auto px-4 sm:px-6 lg:px-8">
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

      <main className="flex-1 max-w-3xl mx-auto w-full px-4 sm:px-6 lg:px-8 py-8 flex flex-col gap-6">
        <Stepper current={step} />

        <div className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-4">
          <div>
            <h2 className="text-lg font-semibold text-white">
              {STEPS[step - 1].title}
            </h2>
            <p className="text-sm text-gray-400 mt-1">{STEPS[step - 1].body}</p>
          </div>

          {loading ? (
            <p className="text-sm text-gray-500">Loading current settings…</p>
          ) : (
            <>
              {step === 1 && (
                <TicketingStep ticketingSystem={ticketingSystem} />
              )}
              {step === 2 && (
                <ProviderStep
                  provider={provider}
                  onChangeProvider={setProvider}
                  baseUrl={baseUrl}
                  onChangeBaseUrl={setBaseUrl}
                  extraArgsText={extraArgsText}
                  onChangeExtraArgs={setExtraArgsText}
                />
              )}
              {step === 3 && (
                <GitHubStep githubAppConfigured={githubAppConfigured} />
              )}
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
                disabled={savingProvider || completing}
                className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
              >
                {step === 4
                  ? completing
                    ? "Finishing…"
                    : "Finish setup"
                  : savingProvider
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

// ---------------------------------------------------------------------------
// Stepper (small, presentational).
// ---------------------------------------------------------------------------

function Stepper({ current }: { current: StepIndex }) {
  return (
    <nav aria-label="Setup steps">
      <ol className="flex items-center justify-between gap-2 text-xs text-gray-400">
        {STEPS.map((s) => {
          const active = s.index === current;
          const done = s.index < current;
          return (
            <li
              key={s.index}
              aria-current={active ? "step" : undefined}
              className={`flex-1 px-2 py-1.5 rounded-md text-center ${
                active
                  ? "bg-blue-900/40 text-blue-300 border border-blue-700/50"
                  : done
                    ? "bg-gray-800/50 text-gray-400 border border-gray-700"
                    : "bg-gray-900 text-gray-500 border border-gray-800"
              }`}
            >
              {s.index}. {s.title}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

// ---------------------------------------------------------------------------
// Step bodies — extracted so each block is small enough to scan.
// ---------------------------------------------------------------------------

function TicketingStep({ ticketingSystem }: { ticketingSystem: string }) {
  return (
    <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 text-sm text-gray-300">
      <p>
        Current ticketing system: <strong>{ticketingSystem || "none"}</strong>
      </p>
      <p className="text-xs text-gray-500 mt-2">
        Change this by editing{" "}
        <code className="text-gray-400">[general] ticketing_system</code> in{" "}
        <code className="text-gray-400">config.toml</code>. A dashboard editor
        ships in a later phase.
      </p>
    </div>
  );
}

interface ProviderStepProps {
  provider: AgentProviderId;
  onChangeProvider: (p: AgentProviderId) => void;
  baseUrl: string;
  onChangeBaseUrl: (v: string) => void;
  extraArgsText: string;
  onChangeExtraArgs: (v: string) => void;
}

function ProviderStep({
  provider,
  onChangeProvider,
  baseUrl,
  onChangeBaseUrl,
  extraArgsText,
  onChangeExtraArgs,
}: ProviderStepProps) {
  const cursorBaseUrlDisabled = provider === "cursor";
  return (
    <div className="flex flex-col gap-4">
      <div>
        <label
          htmlFor="onb-provider"
          className="block text-xs text-gray-400 mb-1"
        >
          Provider
        </label>
        <select
          id="onb-provider"
          value={provider}
          onChange={(e) => onChangeProvider(e.target.value as AgentProviderId)}
          className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
        >
          {V1_PROVIDERS.map((p) => (
            <option key={p} value={p}>
              {PROVIDER_LABEL[p]}
            </option>
          ))}
        </select>
      </div>

      <div>
        <label
          htmlFor="onb-base-url"
          className="block text-xs text-gray-400 mb-1"
        >
          Base URL
        </label>
        <input
          id="onb-base-url"
          type="text"
          value={cursorBaseUrlDisabled ? "" : baseUrl}
          onChange={(e) => onChangeBaseUrl(e.target.value)}
          placeholder="Leave empty to use the vendor public API"
          disabled={cursorBaseUrlDisabled}
          title={
            cursorBaseUrlDisabled
              ? "Cursor CLI does not support custom upstream endpoints"
              : undefined
          }
          className={`w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm font-mono ${
            cursorBaseUrlDisabled
              ? "text-gray-600 cursor-not-allowed"
              : "text-gray-200"
          }`}
        />
        {cursorBaseUrlDisabled && (
          <p className="text-xs text-gray-500 mt-1">
            Cursor CLI does not support custom upstream endpoints.
          </p>
        )}
      </div>

      <div>
        <label
          htmlFor="onb-extra-args"
          className="block text-xs text-gray-400 mb-1"
        >
          Extra args (one per line)
        </label>
        <textarea
          id="onb-extra-args"
          value={extraArgsText}
          onChange={(e) => onChangeExtraArgs(e.target.value)}
          rows={3}
          className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
        />
      </div>
    </div>
  );
}

function GitHubStep({ githubAppConfigured }: { githubAppConfigured: boolean }) {
  return (
    <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 text-sm text-gray-300">
      <p>
        GitHub App:{" "}
        <strong>
          {githubAppConfigured ? "configured" : "not configured"}
        </strong>
      </p>
      <p className="text-xs text-gray-500 mt-2">
        Per-user GitHub personal access token capture ships in Phase 2. For
        now, the wizard records that this step was seen but writes nothing.
      </p>
    </div>
  );
}

function CredentialsStep() {
  return (
    <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 text-sm text-gray-300">
      <p>
        <strong>Your credentials</strong>
      </p>
      <p className="text-xs text-gray-500 mt-2">
        Per-user provider keys and GitHub tokens land in Phase 2 alongside the
        per-user credential store. For now, click <em>Finish setup</em> to
        complete the wizard — workflows will use the deployment-default
        credentials from <code className="text-gray-400">maestro.env</code> if
        any.
      </p>
    </div>
  );
}

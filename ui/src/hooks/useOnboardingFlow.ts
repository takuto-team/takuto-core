// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback, useState } from "react";
import { useNavigate } from "react-router-dom";
import { apiPost } from "../api/client";

export type StepIndex = 1 | 2 | 3 | 4;

interface FlowConfig {
  /** Per-step pre-flight hook. Return `false` to block forward navigation
   *  (e.g. provider-save failed). `void` / `undefined` / `true` allow advance. */
  onBeforeNext?: (step: StepIndex) => boolean | Promise<boolean | void>;
}

/**
 * Wizard navigation state machine: tracks the current step, exposes
 * `goNext` / `goSkip` / `goBack`, and routes "finish" / "skip" on the
 * last step to `POST /api/onboarding/complete` then `/`.
 *
 * Decoupled from the provider-form state — `onBeforeNext` is the seam
 * the page uses to plug `saveProviderStep` into the wizard's "Continue"
 * action without the flow knowing what that step does.
 */
export function useOnboardingFlow({ onBeforeNext }: FlowConfig = {}) {
  const navigate = useNavigate();
  const [step, setStep] = useState<StepIndex>(1);
  const [completing, setCompleting] = useState(false);

  const completeWizard = useCallback(async () => {
    setCompleting(true);
    try {
      // Best-effort call — the wizard still navigates home on 404 / 401.
      await apiPost("/api/onboarding/complete");
    } catch {
      // Swallow — server doesn't have to support the endpoint yet.
    } finally {
      setCompleting(false);
    }
    navigate("/");
  }, [navigate]);

  const goNext = useCallback(async () => {
    if (onBeforeNext) {
      const ok = await onBeforeNext(step);
      if (ok === false) return;
    }
    if (step === 4) {
      await completeWizard();
      return;
    }
    setStep((s) => (s + 1) as StepIndex);
  }, [step, onBeforeNext, completeWizard]);

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

  return { step, completing, goNext, goSkip, goBack };
}

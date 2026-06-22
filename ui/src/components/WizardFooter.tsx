// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Fixed footer for the onboarding wizard: Back / Skip / "Save and Continue".
 * Rendered as the last flex child of the page (after the scrollable <main>),
 * so it stays pinned to the viewport bottom regardless of step height.
 */

import { useTranslation } from "react-i18next";

interface Props {
  isFirstStep: boolean;
  isLastStep: boolean;
  /** Disable the primary button (no unsaved changes, a save in flight, or a
   *  step-specific validation block). */
  continueDisabled: boolean;
  /** A per-step save is in flight (drives the primary button's "Saving…" copy). */
  saving: boolean;
  /** The wizard is finishing (last step → POST /onboarding/complete). */
  completing: boolean;
  onBack: () => void;
  onSkip: () => void;
  onContinue: () => void;
}

export function WizardFooter({
  isFirstStep,
  isLastStep,
  continueDisabled,
  saving,
  completing,
  onBack,
  onSkip,
  onContinue,
}: Props) {
  const { t } = useTranslation("onboarding");
  const primaryLabel = isLastStep
    ? completing
      ? t("nav.finishing")
      : t("nav.finish")
    : saving
      ? t("nav.saving")
      : t("nav.continue");
  return (
    <footer className="border-t border-gray-800 bg-gray-950/80 backdrop-blur-sm">
      <div className="w-full px-4 sm:px-6 lg:px-8 py-3 flex justify-between items-center">
        <button
          type="button"
          onClick={onBack}
          disabled={isFirstStep}
          className="text-xs text-gray-400 hover:text-gray-200 disabled:opacity-30 disabled:cursor-not-allowed cursor-pointer"
        >
          {t("nav.back")}
        </button>
        <div className="flex gap-3">
          <button
            type="button"
            onClick={onSkip}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
          >
            {t("nav.skip")}
          </button>
          <button
            type="button"
            onClick={onContinue}
            disabled={continueDisabled}
            className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
          >
            {primaryLabel}
          </button>
        </div>
      </div>
    </footer>
  );
}

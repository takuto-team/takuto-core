// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Fixed footer for the onboarding wizard: Back / "Save and Continue".
 * Rendered as the last flex child of the page (after the scrollable <main>),
 * so it stays pinned to the viewport bottom regardless of step height. The
 * primary button is always clickable (only blocked while a save is in flight);
 * clicking it saves the step then advances.
 */

import { useTranslation } from "react-i18next";

interface Props {
  isFirstStep: boolean;
  isLastStep: boolean;
  /** Disable the primary button only while a save / finish is in flight. */
  continueDisabled: boolean;
  /** A per-step save is in flight (drives the primary button's "Saving…" copy). */
  saving: boolean;
  /** The wizard is finishing (last step → POST /onboarding/complete). */
  completing: boolean;
  onBack: () => void;
  onContinue: () => void;
}

export function WizardFooter({
  isFirstStep,
  isLastStep,
  continueDisabled,
  saving,
  completing,
  onBack,
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
        <button
          type="button"
          onClick={onContinue}
          disabled={continueDisabled}
          className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
        >
          {primaryLabel}
        </button>
      </div>
    </footer>
  );
}

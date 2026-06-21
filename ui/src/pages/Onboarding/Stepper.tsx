// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useTranslation } from "react-i18next";
import type { StepIndex } from "../../hooks/useOnboardingFlow";

export const ONBOARDING_STEPS: { index: StepIndex; titleKey: string; bodyKey: string }[] = [
  { index: 1, titleKey: "step.ticketing.title", bodyKey: "step.ticketing.body" },
  { index: 2, titleKey: "step.provider.title", bodyKey: "step.provider.body" },
  { index: 3, titleKey: "step.git.title", bodyKey: "step.git.body" },
  { index: 4, titleKey: "step.workflows.title", bodyKey: "step.workflows.body" },
];

export function Stepper({ current }: { current: StepIndex }) {
  const { t } = useTranslation("onboarding");
  return (
    <nav aria-label={t("stepper.ariaLabel")}>
      <ol className="flex items-center justify-between gap-2 text-xs text-gray-400">
        {ONBOARDING_STEPS.map((s) => {
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
              {s.index}. {t(s.titleKey)}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

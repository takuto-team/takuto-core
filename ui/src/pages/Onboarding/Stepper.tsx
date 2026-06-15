// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { StepIndex } from "../../hooks/useOnboardingFlow";

export const ONBOARDING_STEPS: { index: StepIndex; title: string; body: string }[] = [
  {
    index: 1,
    title: "Ticketing",
    body: "Welcome — this is Takuto's first-time setup. Pick where it should read tasks from. You can change any of these settings from the Configuration page later.",
  },
  {
    index: 2,
    title: "AI provider",
    body: "Choose the AI that drives your work items, then add your own provider key.",
  },
  {
    index: 3,
    title: "Git & GitHub",
    body: "Set the git branch and remote Takuto works from, then connect a GitHub App or personal token.",
  },
  {
    index: 4,
    title: "Workflows",
    body: "Review the flows you can run on each work item. Tweak or re-seed them any time.",
  },
];

export function Stepper({ current }: { current: StepIndex }) {
  return (
    <nav aria-label="Setup steps">
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
              {s.index}. {s.title}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

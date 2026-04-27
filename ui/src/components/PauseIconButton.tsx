// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

interface Props {
  onClick: () => void;
  title?: string;
}

export function PauseIconButton({ onClick, title = "Pause the workflow" }: Props) {
  return (
    <button
      onClick={onClick}
      title={title}
      className="w-[22px] h-[22px] rounded-full flex items-center justify-center text-yellow-400 bg-gray-900 border border-yellow-600/50 hover:bg-yellow-900/40 hover:border-yellow-500/70 transition-colors cursor-pointer"
    >
      <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
        <path strokeLinecap="round" strokeLinejoin="round" d="M10 9v6m4-6v6" />
      </svg>
    </button>
  );
}

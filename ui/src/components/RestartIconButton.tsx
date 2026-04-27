// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

interface Props {
  onClick: () => void;
  title?: string;
}

export function RestartIconButton({ onClick, title = "Restart from scratch" }: Props) {
  return (
    <button
      onClick={onClick}
      title={title}
      className="w-[22px] h-[22px] rounded-full flex items-center justify-center text-blue-400 bg-gray-900 border border-blue-600/50 hover:bg-blue-900/40 hover:border-blue-500/70 transition-colors cursor-pointer"
    >
      <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
        <path strokeLinecap="round" strokeLinejoin="round" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
      </svg>
    </button>
  );
}

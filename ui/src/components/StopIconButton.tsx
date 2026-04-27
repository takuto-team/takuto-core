// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

interface Props {
  onClick: () => void;
  title?: string;
}

export function StopIconButton({ onClick, title = "Stop workflow" }: Props) {
  return (
    <button
      onClick={onClick}
      title={title}
      className="w-[22px] h-[22px] rounded-full flex items-center justify-center text-red-400 bg-gray-900 border border-red-600/50 hover:bg-red-900/40 hover:border-red-500/70 transition-colors cursor-pointer"
    >
      <svg className="w-2.5 h-2.5" fill="currentColor" viewBox="0 0 24 24">
        <rect x="6" y="6" width="12" height="12" rx="1" />
      </svg>
    </button>
  );
}

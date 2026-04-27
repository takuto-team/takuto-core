// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

interface Props {
  onClick: () => void;
  title?: string;
}

export function ResumeIconButton({ onClick, title = "Resume workflow" }: Props) {
  return (
    <button
      onClick={onClick}
      title={title}
      className="w-[22px] h-[22px] rounded-full flex items-center justify-center text-emerald-400 bg-gray-900 border border-emerald-600/50 hover:bg-emerald-900/40 hover:border-emerald-500/70 transition-colors cursor-pointer"
    >
      <svg className="w-3 h-3" fill="currentColor" viewBox="0 0 24 24">
        <path d="M5 5v14l9-7-9-7z" />
        <rect x="17" y="5" width="2.5" height="14" rx="0.5" />
      </svg>
    </button>
  );
}

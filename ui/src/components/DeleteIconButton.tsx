// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

interface Props {
  onClick: () => void;
  title?: string;
}

export function DeleteIconButton({ onClick, title = "Delete" }: Props) {
  return (
    <button
      onClick={onClick}
      title={title}
      className="w-7 h-7 rounded-full flex items-center justify-center text-gray-500 bg-gray-900 border border-gray-600 hover:text-red-400 hover:bg-red-900 hover:border-red-500/30 transition-colors cursor-pointer"
    >
      <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
        <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
      </svg>
    </button>
  );
}

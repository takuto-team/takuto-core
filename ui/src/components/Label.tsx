// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

export type LabelVariant = "default" | "success" | "danger" | "warning" | "info" | "purple";

const STYLES: Record<LabelVariant, { bg: string; text: string; border: string }> = {
  default: { bg: "rgba(107,114,128,0.15)", text: "#9ca3af", border: "rgba(107,114,128,0.25)" },
  success: { bg: "rgba(34,197,94,0.15)",   text: "#4ade80", border: "rgba(34,197,94,0.2)"   },
  danger:  { bg: "rgba(239,68,68,0.15)",   text: "#f87171", border: "rgba(239,68,68,0.2)"   },
  warning: { bg: "rgba(234,179,8,0.15)",   text: "#facc15", border: "rgba(234,179,8,0.2)"   },
  info:    { bg: "rgba(59,130,246,0.15)",  text: "#60a5fa", border: "rgba(59,130,246,0.2)"  },
  purple:  { bg: "rgba(168,85,247,0.15)",  text: "#c084fc", border: "rgba(168,85,247,0.2)"  },
};

interface LabelProps {
  variant?: LabelVariant;
  href?: string;
  children: React.ReactNode;
  className?: string;
}

export function Label({ variant = "default", href, children, className }: LabelProps) {
  const { bg, text, border } = STYLES[variant];
  const base = `inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full${className ? ` ${className}` : ""}`;
  const style = { backgroundColor: bg, color: text, borderWidth: 1, borderColor: border };

  if (href) {
    return (
      <a
        href={href}
        target="_blank"
        rel="noopener noreferrer"
        className={`${base} hover:brightness-125 transition-[filter] cursor-pointer`}
        style={style}
      >
        {children}
      </a>
    );
  }

  return (
    <span className={base} style={style}>
      {children}
    </span>
  );
}

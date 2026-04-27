// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

export type ButtonVariant = "primary" | "secondary" | "success" | "danger";

interface ButtonProps {
  variant: ButtonVariant;
  onClick?: () => void;
  children: React.ReactNode;
  title?: string;
  className?: string;
  disabled?: boolean;
}

const VARIANT_CLASS: Record<ButtonVariant, string> = {
  primary: "wf-btn-primary",
  secondary: "wf-btn-secondary",
  success: "wf-btn-success",
  danger: "wf-btn-danger",
};

export function Button({ variant, onClick, children, title, className, disabled }: ButtonProps) {
  const cls = VARIANT_CLASS[variant];
  return (
    <button
      onClick={onClick}
      title={title}
      disabled={disabled}
      className={`action-btn ${cls}${className ? ` ${className}` : ""}${disabled ? " opacity-50 cursor-not-allowed" : ""}`}
    >
      {children}
    </button>
  );
}

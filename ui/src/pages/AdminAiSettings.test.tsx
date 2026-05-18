// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { ProviderSwitchConfirm } from "./AdminAiSettings";
import { ToastProvider } from "../hooks/useToast";

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  // Auto-cleanup isn't always wired with vitest-projects; clean manually so
  // each `render` starts with a fresh DOM and getByRole returns one element.
  cleanup();
  vi.restoreAllMocks();
});

function renderWithProviders(node: React.ReactNode) {
  return render(
    <ToastProvider>
      <MemoryRouter>{node}</MemoryRouter>
    </ToastProvider>,
  );
}

describe("ProviderSwitchConfirm", () => {
  it("requires the literal 'SWITCH' before the confirm button enables", () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    renderWithProviders(
      <ProviderSwitchConfirm
        from="claude"
        to="cursor"
        onConfirm={onConfirm}
        onCancel={onCancel}
      />,
    );

    const confirmBtn = screen.getByRole("button", {
      name: /switch provider/i,
    }) as HTMLButtonElement;
    // Disabled by default — the type-SWITCH gate is unmet.
    expect(confirmBtn.disabled).toBe(true);

    // A wrong word keeps it disabled.
    const input = screen.getByLabelText(/type/i);
    fireEvent.change(input, { target: { value: "nope" } });
    expect(confirmBtn.disabled).toBe(true);

    // Typing SWITCH enables the confirm button.
    fireEvent.change(input, { target: { value: "SWITCH" } });
    expect(confirmBtn.disabled).toBe(false);

    fireEvent.click(confirmBtn);
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("Cancel button invokes onCancel without onConfirm", () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    renderWithProviders(
      <ProviderSwitchConfirm
        from="claude"
        to="codex"
        onConfirm={onConfirm}
        onCancel={onCancel}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("renders the human-readable provider labels in the body", () => {
    renderWithProviders(
      <ProviderSwitchConfirm
        from="opencode"
        to="codex"
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    // alertdialog body mentions both labels (case-sensitive) — copy lives in
    // the source file so this also serves as a regression test for it.
    expect(screen.getAllByText(/OpenCode/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/Codex/).length).toBeGreaterThan(0);
  });
});

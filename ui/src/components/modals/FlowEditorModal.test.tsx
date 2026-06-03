// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Coverage for the Add / Edit flow modal. The modal owns the client-side
 * mirror of the backend validator (unique name, unique slug, dependency
 * cycles, >= 1 step) and submits the whole list on save.
 */

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { FlowEditorModal } from "./FlowEditorModal";
import type { UserFlow } from "../../api/flows";

function flow(name: string, depends_on: string[] = []): UserFlow {
  return {
    name,
    depends_on,
    steps: [{ name: "step", prompt: "do it", skills: [] }],
  };
}

afterEach(() => cleanup());

describe("FlowEditorModal — name validation", () => {
  it("surfaces an inline error when the name collides with another flow", () => {
    render(
      <FlowEditorModal
        flows={[flow("Build")]}
        editIndex={null}
        onSubmit={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByPlaceholderText(/lint_and_test/i), {
      target: { value: "Build" },
    });
    expect(screen.getByText(/a flow named "Build" already exists/i)).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: /create flow/i }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it("detects a slug collision between two distinct names", () => {
    render(
      <FlowEditorModal
        flows={[flow("Implement Ticket")]}
        editIndex={null}
        onSubmit={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    // "implement-ticket" kebab-cases to the same slug as "Implement Ticket".
    fireEvent.change(screen.getByPlaceholderText(/lint_and_test/i), {
      target: { value: "implement-ticket" },
    });
    expect(screen.getByText(/both become "implement-ticket"/i)).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: /create flow/i }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });
});

describe("FlowEditorModal — dependency cycle", () => {
  it("warns and blocks save when a new dependency closes a cycle", () => {
    // B already depends on A. Editing A and making it depend on B closes the loop.
    render(
      <FlowEditorModal
        flows={[flow("A"), flow("B", ["A"])]}
        editIndex={0}
        onSubmit={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    // Open the depends-on dropdown and select the only sibling, "B".
    fireEvent.click(screen.getByRole("button", { name: /add dependency/i }));
    fireEvent.click(screen.getByRole("button", { name: "B" }));

    expect(screen.getByText(/create a cycle/i)).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: /save flow/i }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });
});

describe("FlowEditorModal — steps repeater", () => {
  it("adds and removes step rows; the last remaining step cannot be removed", () => {
    render(
      <FlowEditorModal flows={[]} editIndex={null} onSubmit={vi.fn()} onClose={vi.fn()} />,
    );
    // Starts with exactly one step whose Remove button is disabled.
    expect(screen.getAllByText(/^Step \d+$/)).toHaveLength(1);
    expect((screen.getByRole("button", { name: /^remove$/i }) as HTMLButtonElement).disabled).toBe(
      true,
    );

    fireEvent.click(screen.getByRole("button", { name: /add step/i }));
    expect(screen.getAllByText(/^Step \d+$/)).toHaveLength(2);

    // With two steps, Remove is enabled; clicking it drops back to one.
    const removeButtons = screen.getAllByRole("button", { name: /^remove$/i });
    expect((removeButtons[0] as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(removeButtons[0]);
    expect(screen.getAllByText(/^Step \d+$/)).toHaveLength(1);
  });
});

describe("FlowEditorModal — save", () => {
  it("submits the full list with the new flow appended", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const onClose = vi.fn();
    render(
      <FlowEditorModal
        flows={[flow("Build")]}
        editIndex={null}
        onSubmit={onSubmit}
        onClose={onClose}
      />,
    );

    fireEvent.change(screen.getByPlaceholderText(/lint_and_test/i), {
      target: { value: "Deploy" },
    });
    fireEvent.change(screen.getByPlaceholderText(/cargo fmt/i), {
      target: { value: "ship it" },
    });
    fireEvent.change(screen.getByPlaceholderText(/sent verbatim/i), {
      target: { value: "Run the deploy" },
    });

    fireEvent.click(screen.getByRole("button", { name: /create flow/i }));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const submitted = onSubmit.mock.calls[0][0] as UserFlow[];
    expect(submitted.map((f) => f.name)).toEqual(["Build", "Deploy"]);
    expect(submitted[1].steps[0]).toMatchObject({ name: "ship it", prompt: "Run the deploy" });
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });
});

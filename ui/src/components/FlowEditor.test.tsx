// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Coverage for the inline flow editor. The editor owns the dependency-cycle
 * check, the steps repeater, the >= 1 step + per-step rules, and the save
 * action. Name + slug validation is surfaced to the parent via `onNameError`
 * (the parent — FlowCard or FlowsTab — renders the editable name in the
 * card header).
 */

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { FlowEditor } from "./FlowEditor";
import type { UserFlow } from "../api/flows";

function flow(name: string, depends_on: string[] = []): UserFlow {
  return {
    name,
    depends_on,
    steps: [{ name: "step", prompt: "do it", skills: [] }],
  };
}

afterEach(() => cleanup());

describe("FlowEditor — name validation surfaced upward", () => {
  it("reports a collision via onNameError", () => {
    const onNameError = vi.fn();
    render(
      <FlowEditor
        flows={[flow("Build")]}
        editIndex={null}
        name="Build"
        onNameError={onNameError}
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    expect(onNameError).toHaveBeenLastCalledWith(
      expect.stringMatching(/a workflow named "Build" already exists/i),
    );
    expect(
      (screen.getByRole("button", { name: /create workflow/i }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it("reports a slug collision between two distinct names via onNameError", () => {
    const onNameError = vi.fn();
    render(
      <FlowEditor
        flows={[flow("Implement Ticket")]}
        editIndex={null}
        name="implement-ticket"
        onNameError={onNameError}
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    expect(onNameError).toHaveBeenLastCalledWith(
      expect.stringMatching(/both become "implement-ticket"/i),
    );
    expect(
      (screen.getByRole("button", { name: /create workflow/i }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });
});

describe("FlowEditor — dependency cycle", () => {
  it("warns and blocks save when a new dependency closes a cycle", () => {
    render(
      <FlowEditor
        flows={[flow("A"), flow("B", ["A"])]}
        editIndex={0}
        name="A"
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /add dependency/i }));
    fireEvent.click(screen.getByRole("button", { name: "B" }));

    expect(screen.getByText(/create a cycle/i)).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: /save workflow/i }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });
});

describe("FlowEditor — steps repeater", () => {
  it("adds and removes step rows; the last remaining step cannot be removed", () => {
    render(
      <FlowEditor
        flows={[]}
        editIndex={null}
        name="x"
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    expect(screen.getAllByRole("button", { name: /^remove$/i })).toHaveLength(1);
    expect((screen.getByRole("button", { name: /^remove$/i }) as HTMLButtonElement).disabled).toBe(
      true,
    );

    fireEvent.click(screen.getByRole("button", { name: /add step/i }));
    expect(screen.getAllByRole("button", { name: /^remove$/i })).toHaveLength(2);

    const removeButtons = screen.getAllByRole("button", { name: /^remove$/i });
    expect((removeButtons[0] as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(removeButtons[0]);
    expect(screen.getAllByRole("button", { name: /^remove$/i })).toHaveLength(1);
  });

  it("renames a step in place via the click-to-edit header", () => {
    render(
      <FlowEditor
        flows={[]}
        editIndex={null}
        name="x"
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /untitled step/i }));
    const headerInput = screen.getByPlaceholderText(/untitled step/i);
    fireEvent.change(headerInput, { target: { value: "lint" } });
    fireEvent.keyDown(headerInput, { key: "Enter" });
    expect(screen.getByRole("button", { name: "lint" })).toBeTruthy();
  });
});

describe("FlowEditor — save", () => {
  it("submits the full list with the new flow appended", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const onCancel = vi.fn();
    render(
      <FlowEditor
        flows={[flow("Build")]}
        editIndex={null}
        name="Deploy"
        onSubmit={onSubmit}
        onCancel={onCancel}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /untitled step/i }));
    fireEvent.change(screen.getByPlaceholderText(/untitled step/i), {
      target: { value: "ship it" },
    });
    fireEvent.change(screen.getByPlaceholderText(/sent verbatim/i), {
      target: { value: "Run the deploy" },
    });

    fireEvent.click(screen.getByRole("button", { name: /create workflow/i }));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const submitted = onSubmit.mock.calls[0][0] as UserFlow[];
    expect(submitted.map((f) => f.name)).toEqual(["Build", "Deploy"]);
    expect(submitted[1].steps[0]).toMatchObject({ name: "ship it", prompt: "Run the deploy" });
  });
});

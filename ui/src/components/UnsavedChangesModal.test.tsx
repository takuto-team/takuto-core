// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react";
import { UnsavedChangesModal } from "./UnsavedChangesModal";

afterEach(cleanup);

describe("UnsavedChangesModal", () => {
  it("Cancel stays (no save, no proceed)", () => {
    const onSave = vi.fn().mockResolvedValue(true);
    const onProceed = vi.fn();
    const onCancel = vi.fn();
    render(<UnsavedChangesModal onSave={onSave} onProceed={onProceed} onCancel={onCancel} />);
    fireEvent.click(screen.getByRole("button", { name: /^cancel$/i }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onSave).not.toHaveBeenCalled();
    expect(onProceed).not.toHaveBeenCalled();
  });

  it("Discard leaves without saving", () => {
    const onSave = vi.fn().mockResolvedValue(true);
    const onProceed = vi.fn();
    render(<UnsavedChangesModal onSave={onSave} onProceed={onProceed} onCancel={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /discard changes/i }));
    expect(onProceed).toHaveBeenCalledTimes(1);
    expect(onSave).not.toHaveBeenCalled();
  });

  it("Save & leave saves then proceeds on success", async () => {
    const onSave = vi.fn().mockResolvedValue(true);
    const onProceed = vi.fn();
    render(<UnsavedChangesModal onSave={onSave} onProceed={onProceed} onCancel={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /save & leave/i }));
    await waitFor(() => expect(onProceed).toHaveBeenCalledTimes(1));
    expect(onSave).toHaveBeenCalledTimes(1);
  });

  it("Save & leave does NOT proceed when the save fails", async () => {
    const onSave = vi.fn().mockResolvedValue(false);
    const onProceed = vi.fn();
    render(<UnsavedChangesModal onSave={onSave} onProceed={onProceed} onCancel={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /save & leave/i }));
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
    expect(onProceed).not.toHaveBeenCalled();
  });
});

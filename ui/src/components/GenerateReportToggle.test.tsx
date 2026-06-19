// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * The presentational report toggle: renders its explanation, reflects `value`
 * via aria-checked, and calls `onChange` with the flipped value on click.
 */

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, cleanup, fireEvent } from "@testing-library/react";
import { GenerateReportToggle } from "./GenerateReportToggle";

afterEach(cleanup);

describe("GenerateReportToggle", () => {
  it("renders the label and explanation", () => {
    render(<GenerateReportToggle value={false} onChange={() => {}} />);
    expect(screen.getByText("Generate work-item reports")).toBeTruthy();
    expect(screen.getByText(/Show Report/)).toBeTruthy();
  });

  it("reflects value via aria-checked", () => {
    const { rerender } = render(<GenerateReportToggle value={false} onChange={() => {}} />);
    expect(screen.getByRole("switch").getAttribute("aria-checked")).toBe("false");
    rerender(<GenerateReportToggle value={true} onChange={() => {}} />);
    expect(screen.getByRole("switch").getAttribute("aria-checked")).toBe("true");
  });

  it("calls onChange with the flipped value", () => {
    const onChange = vi.fn();
    render(<GenerateReportToggle value={false} onChange={onChange} />);
    fireEvent.click(screen.getByRole("switch"));
    expect(onChange).toHaveBeenCalledWith(true);
  });

  it("does not fire onChange while disabled", () => {
    const onChange = vi.fn();
    render(<GenerateReportToggle value={false} onChange={onChange} disabled />);
    fireEvent.click(screen.getByRole("switch"));
    expect(onChange).not.toHaveBeenCalled();
  });
});

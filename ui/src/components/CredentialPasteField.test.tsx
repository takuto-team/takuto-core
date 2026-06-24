// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * CredentialPasteField masking: when `masked` (the secret is stored), the field
 * shows a fixed `••••••` indicator + a Replace affordance instead of an empty
 * input. Replace reveals an editable field; the sentinel is never the input's
 * editable value, so it can never be submitted as a secret.
 */

import { describe, it, expect, afterEach } from "vitest";
import { useState } from "react";
import { render, screen, cleanup, fireEvent } from "@testing-library/react";
import { CredentialPasteField } from "./CredentialPasteField";

afterEach(() => cleanup());

const MASK = "••••••";

function Harness({ masked }: { masked: boolean }) {
  const [value, setValue] = useState("");
  return (
    <CredentialPasteField
      label="API key"
      value={value}
      onChange={setValue}
      onSubmit={() => {}}
      masked={masked}
    />
  );
}

describe("CredentialPasteField — masked secret", () => {
  it("shows the •••••• indicator (read-only) and no Save button when set", () => {
    render(<Harness masked />);
    const input = screen.getByLabelText("API key") as HTMLInputElement;
    expect(input.value).toBe(MASK);
    expect(input.readOnly).toBe(true);
    // No Save while masked/untouched — KEEP is the default.
    expect(screen.queryByRole("button", { name: /save/i })).toBeNull();
    expect(screen.getByRole("button", { name: "Replace" })).toBeTruthy();
  });

  it("Replace reveals an empty editable input (sentinel is never the value)", () => {
    render(<Harness masked />);
    fireEvent.click(screen.getByRole("button", { name: "Replace" }));
    const input = screen.getByLabelText("API key") as HTMLInputElement;
    expect(input.value).toBe(""); // empty, NOT the •••••• sentinel
    expect(input.readOnly).toBe(false);
    fireEvent.change(input, { target: { value: "new-secret" } });
    expect((screen.getByLabelText("API key") as HTMLInputElement).value).toBe("new-secret");
  });

  it("renders a normal editable input (no mask) when not set", () => {
    render(<Harness masked={false} />);
    const input = screen.getByLabelText("API key") as HTMLInputElement;
    expect(input.value).toBe("");
    expect(input.readOnly).toBe(false);
    expect(screen.queryByRole("button", { name: "Replace" })).toBeNull();
  });
});

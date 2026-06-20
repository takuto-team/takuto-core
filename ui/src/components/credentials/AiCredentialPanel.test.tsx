// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Regression guards for two root-cause fixes:
 *
 *   1. Cross-provider key carry-over — a value typed for provider A must NOT
 *      survive a switch to provider B (it would otherwise be saved under B,
 *      creating a credential row for a provider the user never keyed).
 *   2. Per-provider Delete button — renders only when the active provider has
 *      an api_key credential, and fires `onDelete("api_key")` after a two-step
 *      inline confirm.
 */

import { describe, it, expect, vi, afterEach } from "vitest";
import {
  render,
  screen,
  cleanup,
  fireEvent,
} from "@testing-library/react";
import { AiCredentialPanel } from "./AiCredentialPanel";
import type { UserCredentialsStatus } from "../../api/types";

afterEach(cleanup);

/** A credentials bundle with an active api_key for `provider`. */
function connectedFor(provider: string): UserCredentialsStatus {
  return {
    provider: {
      provider,
      api_key: {
        provider,
        kind: "api_key",
        active: true,
        last_validated_at: "2026-06-15T08:00:00Z",
        last_used_at: null,
      },
    },
    github: null,
  };
}

describe("AiCredentialPanel — cross-provider carry-over", () => {
  it("clears the typed api key when activeProvider changes", () => {
    const onSave = vi.fn().mockResolvedValue(true);
    const { rerender } = render(
      <AiCredentialPanel
        activeProvider="cursor"
        credentials={null}
        onSave={onSave}
      />,
    );

    const input = screen.getByLabelText(/Cursor API key/i) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "sk-typed-for-cursor" } });
    expect(input.value).toBe("sk-typed-for-cursor");

    // Switching provider must wipe the dirty value so it can't be saved under
    // the new provider.
    rerender(
      <AiCredentialPanel
        activeProvider="opencode"
        credentials={null}
        onSave={onSave}
      />,
    );
    const opencodeInput = screen.getByLabelText(
      /Bearer token/i,
    ) as HTMLInputElement;
    expect(opencodeInput.value).toBe("");
  });
});

describe("AiCredentialPanel — Delete button", () => {
  it("is hidden when the active provider has no credential", () => {
    render(
      <AiCredentialPanel
        activeProvider="cursor"
        credentials={null}
        onSave={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    expect(screen.queryByRole("button", { name: /^Delete/i })).toBeNull();
  });

  it("is hidden when no onDelete handler is supplied (e.g. onboarding)", () => {
    render(
      <AiCredentialPanel
        activeProvider="cursor"
        credentials={connectedFor("cursor")}
        onSave={vi.fn()}
      />,
    );
    expect(screen.queryByRole("button", { name: /^Delete/i })).toBeNull();
  });

  it("renders Delete when connected and fires onDelete('api_key') after confirm", async () => {
    const onDelete = vi.fn().mockResolvedValue(true);
    render(
      <AiCredentialPanel
        activeProvider="cursor"
        credentials={connectedFor("cursor")}
        onSave={vi.fn()}
        onDelete={onDelete}
      />,
    );

    const del = screen.getByRole("button", { name: /Delete Cursor API key/i });
    // First click arms the confirm; it must NOT delete yet.
    fireEvent.click(del);
    expect(onDelete).not.toHaveBeenCalled();
    const confirm = screen.getByRole("button", {
      name: /Confirm delete Cursor API key/i,
    });
    fireEvent.click(confirm);
    expect(onDelete).toHaveBeenCalledTimes(1);
    expect(onDelete).toHaveBeenCalledWith("api_key");
  });
});

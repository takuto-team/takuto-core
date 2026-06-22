// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Coverage for the wizard's inline AI-key panel connection pill.
 *
 * Regression: when the provider selected in step 2 differs from the
 * deployment's persisted `[agent] provider`, the pill used to stay on "Not
 * connected" because the credentials GET was scoped to the persisted provider.
 * The fetch is now scoped to the selected provider (`?provider=`), so the pill
 * flips to "Token provided" once the key is saved. The wizard footer's "Save
 * and Continue" persists the typed key via the panel's `saveIfDirty()` handle
 * (the panel no longer has its own Save button in the wizard — `deferSave`).
 */

import { createRef } from "react";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, fireEvent, act } from "@testing-library/react";
import { OnboardingAiKey } from "./OnboardingAiKey";
import type { AiCredentialPanelHandle } from "../../components/credentials/AiCredentialPanel";
import { ToastProvider } from "../../hooks/useToast";

let posted = false;
let getCalls: string[] = [];

const activeBundle = {
  provider: {
    provider: "cursor",
    api_key: { provider: "cursor", kind: "api_key", active: true, last_validated_at: null, last_used_at: null },
  },
  github: null,
  jira: null,
};

beforeEach(() => {
  posted = false;
  getCalls = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        });

      if (init?.method === "POST" && url.startsWith("/api/users/me/credentials/")) {
        posted = true;
        return json({});
      }
      if (url.startsWith("/api/users/me/credentials")) {
        getCalls.push(url);
        // Before the POST the user has no Cursor credential; after it the
        // (provider-scoped) GET reports the active bundle.
        return json(posted ? activeBundle : { provider: null, github: null, jira: null });
      }
      return json({});
    }),
  );
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("OnboardingAiKey — fold-in save flips the connection pill", () => {
  it("scopes the credentials fetch to the selected provider and shows Token provided after saveIfDirty", async () => {
    const ref = createRef<AiCredentialPanelHandle>();
    render(
      <ToastProvider>
        <OnboardingAiKey ref={ref} provider="cursor" />
      </ToastProvider>,
    );

    // Initial fetch is scoped to the selected provider, not the persisted one.
    await waitFor(() => expect(getCalls.length).toBeGreaterThan(0));
    expect(getCalls.every((u) => u.includes("provider=cursor"))).toBe(true);

    // Pill starts disconnected. No inline Save button (deferSave in the wizard).
    const keyInput = await screen.findByLabelText(/Cursor API key/i);
    expect(screen.getByText("Not connected")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Save$/i })).toBeNull();

    fireEvent.change(keyInput, { target: { value: "cur_test_key" } });
    // The footer's "Save and Continue" drives this handle.
    await act(async () => {
      await ref.current!.saveIfDirty();
    });

    // After the save + provider-scoped refresh, the pill flips to the
    // per-user "Token provided" state.
    await waitFor(() => expect(screen.getByText("Token provided")).toBeTruthy());
    expect(posted).toBe(true);
  });
});

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  render,
  screen,
  fireEvent,
  waitFor,
  cleanup,
} from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { AdminAiSettings, ProviderSwitchConfirm } from "./AdminAiSettings";
import { ToastProvider, ToastContainer } from "../hooks/useToast";

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

// ---------------------------------------------------------------------------
// #35 — persist_warning surfacing on PUT /api/config/agent.
//
// These tests drive the full AdminAiSettings page through a mocked fetch
// so we exercise the actual save handler and confirm the right toast
// variant appears. ToastContainer is rendered alongside so toast DOM is
// inspectable.
// ---------------------------------------------------------------------------

function renderAdminPage() {
  return render(
    <ToastProvider>
      <MemoryRouter>
        <AdminAiSettings onLogout={vi.fn()} authEnabled isAdmin />
        <ToastContainer />
      </MemoryRouter>
    </ToastProvider>,
  );
}

/**
 * Minimal config shape the page expects to read on mount. The page reads
 * `agent.provider`, `agent.available_providers`, and the provider sub-table
 * — anything else can be empty.
 */
function baseConfig(): unknown {
  return {
    general: { ticketing_system: "none" },
    agent: {
      provider: "claude",
      available_providers: ["claude", "cursor", "codex", "opencode"],
      providers: {
        claude: {
          model: "",
          base_url: "",
          extra_args: [],
          allow_shared_default: false,
        },
      },
    },
    jira: { project_keys: [], site: "" },
    github: { app_id: 0, app_installation_id: 0 },
    web: { dashboard_username: "" },
    jira_available: false,
    ticketing_system: "none",
    github_app_configured: false,
    repo_exists: true,
  };
}

/**
 * Install a fetch stub that returns `baseConfig` for the initial GET and
 * the supplied response for the PUT. Returns the install/teardown handles.
 */
function stubConfigFetch(putResponse: unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init?: RequestInit) => {
      if (typeof input === "string" && input === "/api/config" && (!init || !init.method)) {
        return new Response(JSON.stringify(baseConfig()), { status: 200 });
      }
      if (
        typeof input === "string" &&
        input === "/api/config/agent" &&
        init?.method === "PUT"
      ) {
        return new Response(JSON.stringify(putResponse), { status: 200 });
      }
      return new Response("not found", { status: 404 });
    }),
  );
}

describe("AdminAiSettings — persist_warning surfacing (#35)", () => {
  it("shows an ERROR toast when the server reports persisted=false + persist_warning", async () => {
    stubConfigFetch({
      ...(baseConfig() as Record<string, unknown>),
      persisted: false,
      persist_warning: "Permission denied (os error 13)",
    });
    renderAdminPage();

    // Wait for initial /api/config GET to settle and the form to mount.
    const saveBtn = await waitFor(() =>
      screen.getByRole("button", { name: /^save changes$/i }),
    );
    fireEvent.click(saveBtn);

    // The page should surface the persist warning verbatim AND warn the
    // admin that the change will be lost at restart.
    await waitFor(() => {
      expect(
        screen.getByText(/applied in memory but NOT persisted to disk/i),
      ).toBeTruthy();
    });
    const toastBody =
      screen.getByText(/applied in memory but NOT persisted to disk/i)
        .textContent ?? "";
    expect(toastBody).toContain("Permission denied (os error 13)");
    expect(toastBody).toMatch(/lost on next restart/i);
    // The "AI provider settings saved." success copy must NOT appear in
    // the toast region.
    expect(
      screen.queryByText(/^AI provider settings saved\.$/i),
    ).toBeNull();
  });

  it("shows a SUCCESS toast when persisted=true", async () => {
    stubConfigFetch({
      ...(baseConfig() as Record<string, unknown>),
      persisted: true,
      // persist_warning omitted — backend skips serialising it on success.
    });
    renderAdminPage();

    const saveBtn = await waitFor(() =>
      screen.getByRole("button", { name: /^save changes$/i }),
    );
    fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(screen.getByText(/AI provider settings saved\./i)).toBeTruthy();
    });
    expect(
      screen.queryByText(/applied in memory but NOT persisted/i),
    ).toBeNull();
  });

  it("assumes success when neither persisted nor persist_warning is present (legacy server)", async () => {
    // Pre-Phase-1 server, or any backend that hasn't been updated to emit
    // the persisted/persist_warning fields. The strict `=== false` check
    // means undefined falls through to the success branch.
    stubConfigFetch(baseConfig());
    renderAdminPage();

    const saveBtn = await waitFor(() =>
      screen.getByRole("button", { name: /^save changes$/i }),
    );
    fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(screen.getByText(/AI provider settings saved\./i)).toBeTruthy();
    });
  });

  it("falls back to 'unknown error' in the toast when persist_warning is null", async () => {
    stubConfigFetch({
      ...(baseConfig() as Record<string, unknown>),
      persisted: false,
      persist_warning: null,
    });
    renderAdminPage();

    const saveBtn = await waitFor(() =>
      screen.getByRole("button", { name: /^save changes$/i }),
    );
    fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(screen.getByText(/unknown error/i)).toBeTruthy();
    });
  });
});

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Task #27 coverage — the per-warning-code "Set up" deep-link mapping.
 *
 * Each row in the spec table from the task description gets a test here so
 * any regression on the mapping fails fast.
 */

import { describe, it, expect, afterEach } from "vitest";
import { render, screen, cleanup, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { OnboardingBanner } from "./OnboardingBanner";
import type { StructuredWarning, SystemStatus } from "../api/types";

function healthy(): SystemStatus {
  return {
    config_toml_ok: true,
    github: {
      mode: "app",
      app_configured: true,
      app_id: 1,
      app_name: "maestro-bot",
    },
    provider: {
      selected: "claude",
      deployment_default_credential_present: true,
      headless_capable: true,
      custom_base_url: null,
    },
    ticketing: { system: "jira", acli_ok: true },
    per_user_required: true,
    warnings: [],
  };
}

function statusWith(warnings: StructuredWarning[]): SystemStatus {
  return { ...healthy(), warnings };
}

function renderBanner(
  status: SystemStatus | null | undefined,
  opts: { isAdmin?: boolean; legacyPreflightError?: string | null } = {},
) {
  return render(
    <MemoryRouter>
      <OnboardingBanner
        status={status}
        isAdmin={opts.isAdmin ?? false}
        legacyPreflightError={opts.legacyPreflightError ?? null}
      />
    </MemoryRouter>,
  );
}

afterEach(() => {
  cleanup();
});

// ---------------------------------------------------------------------------
// User-facing internal links (visible to every authenticated user).
// ---------------------------------------------------------------------------

describe.each([
  ["claude_not_authenticated", "Set Claude credential"],
  ["cursor_not_authenticated", "Set Cursor credential"],
  ["codex_not_authenticated", "Set Codex credential"],
  ["opencode_not_authenticated", "Set OpenCode credential"],
  ["gh_auth_missing", "Set GitHub PAT"],
])("OnboardingBanner — %s deep-link", (code, expectedLabel) => {
  it(`renders a /me/credentials Link labelled "${expectedLabel}"`, () => {
    renderBanner(
      statusWith([
        {
          code,
          severity: "critical",
          message: `Test message for ${code}`,
        },
      ]),
    );
    const link = screen.getByRole("link", {
      name: new RegExp(expectedLabel, "i"),
    }) as HTMLAnchorElement;
    expect(link).toBeTruthy();
    // Use endsWith because the rendered href is absolute under jsdom.
    expect(link.getAttribute("href")).toBe("/me/credentials");
    expect(link.getAttribute("target")).toBeNull(); // internal — not new tab
  });

  it(`renders the same link for non-admins (${code} is not admin-only)`, () => {
    renderBanner(
      statusWith([
        { code, severity: "critical", message: "msg" },
      ]),
      { isAdmin: false },
    );
    expect(
      screen.getByRole("link", { name: new RegExp(expectedLabel, "i") }),
    ).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// Admin-only internal link: provider_not_implemented.
// ---------------------------------------------------------------------------

describe("OnboardingBanner — provider_not_implemented (admin-only)", () => {
  const warning: StructuredWarning = {
    code: "provider_not_implemented",
    severity: "critical",
    message: "Codex adapter ships in Phase 4.",
  };

  it("shows a /admin/ai 'Change provider' link for admins", () => {
    renderBanner(statusWith([warning]), { isAdmin: true });
    const link = screen.getByRole("link", { name: /change provider/i }) as HTMLAnchorElement;
    expect(link.getAttribute("href")).toBe("/admin/ai");
  });

  it("renders the greyed 'Ask your admin' hint for non-admins, with NO link", () => {
    renderBanner(statusWith([warning]), { isAdmin: false });
    expect(
      screen.getByText(/ask your admin to change the provider/i),
    ).toBeTruthy();
    // Confirm we did NOT render a link element for this warning.
    expect(
      screen.queryByRole("link", { name: /change provider/i }),
    ).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Admin-only external docs links.
// ---------------------------------------------------------------------------

describe.each([
  "master_key_unavailable",
  "secret_key_world_readable",
  "config_missing",
  "acli_missing",
])("OnboardingBanner — %s docs link", (code) => {
  const warning: StructuredWarning = {
    code,
    severity: "critical",
    message: `Test ${code} message`,
  };

  it("shows a docs link opening in a new tab for admins", () => {
    renderBanner(statusWith([warning]), { isAdmin: true });
    const link = screen.getByRole("link", { name: /read docs/i }) as HTMLAnchorElement;
    expect(link.getAttribute("href")).toBe(
      "https://github.com/morphet81/maestro/blob/main/AGENTS.md",
    );
    expect(link.getAttribute("target")).toBe("_blank");
    expect(link.getAttribute("rel")).toBe("noopener noreferrer");
  });

  it("hides the link for non-admins and shows a hint instead", () => {
    renderBanner(statusWith([warning]), { isAdmin: false });
    expect(screen.queryByRole("link", { name: /read docs/i })).toBeNull();
    expect(screen.getByText(/ask your admin/i)).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// Unknown / no-CTA codes.
// ---------------------------------------------------------------------------

describe("OnboardingBanner — unknown code", () => {
  it("renders the message but no CTA when the code is not in the mapping", () => {
    const warning: StructuredWarning = {
      code: "some_future_code_we_dont_know_about",
      severity: "critical",
      message: "Mystery warning",
    };
    renderBanner(statusWith([warning]));
    expect(screen.getByText(/mystery warning/i)).toBeTruthy();
    // The banner itself renders no <a> elements in this case.
    expect(screen.queryAllByRole("link")).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Multiple warnings — same destination should NOT collapse.
// ---------------------------------------------------------------------------

describe("OnboardingBanner — multiple warnings", () => {
  it("renders one link per warning even when they share /me/credentials", () => {
    renderBanner(
      statusWith([
        {
          code: "claude_not_authenticated",
          severity: "critical",
          message: "Claude missing",
        },
        {
          code: "gh_auth_missing",
          severity: "critical",
          message: "GitHub missing",
        },
      ]),
    );
    // One link per warning, distinct labels — never collapsed onto a single
    // shared button.
    expect(screen.getByRole("link", { name: /set claude credential/i })).toBeTruthy();
    expect(screen.getByRole("link", { name: /set github pat/i })).toBeTruthy();
  });

  it("ignores non-critical warnings (info / warning severities render no row)", () => {
    renderBanner(
      statusWith([
        {
          code: "acli_missing",
          severity: "info",
          message: "acli not authed",
        },
      ]),
    );
    // Banner renders nothing — no critical warnings means no banner.
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Legacy fallback — no structured codes, no deep-links.
// ---------------------------------------------------------------------------

describe("OnboardingBanner — legacy preflight fallback", () => {
  it("renders the legacy preflight error with NO deep-link buttons", () => {
    renderBanner(null, {
      legacyPreflightError: "GITHUB_APP_PRIVATE_KEY is not set",
    });
    // The legacy block has no <a role="link"> CTAs since codes are absent.
    expect(screen.queryAllByRole("link")).toHaveLength(0);
    // But the message itself is rendered inside the alert.
    const alert = screen.getByRole("alert");
    expect(
      within(alert).getByText(/GITHUB_APP_PRIVATE_KEY is not set/),
    ).toBeTruthy();
  });
});

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Unit tests for the client-side Claude `~/.claude.json` validator (#40).
 * The Rust server is the authoritative validator — this test set just
 * confirms the UI's pre-flight check mirrors the server's required-field
 * rules (`oauthAccount.{accountUuid, emailAddress, organizationUuid}`).
 */

import { describe, it, expect } from "vitest";
import { parseClaudeSessionBlob } from "./claudeSession";

describe("parseClaudeSessionBlob", () => {
  it("returns ok on a minimal valid blob (only oauthAccount + required keys)", () => {
    const blob = JSON.stringify({
      oauthAccount: {
        accountUuid: "11111111-1111-1111-1111-111111111111",
        emailAddress: "alice@example.com",
        organizationUuid: "22222222-2222-2222-2222-222222222222",
      },
    });
    expect(parseClaudeSessionBlob(blob)).toEqual({ ok: true });
  });

  it("returns ok when the blob contains extra unknown fields", () => {
    // The server ignores extra fields — the client must too.
    const blob = JSON.stringify({
      shellSnapshot: "/tmp/x",
      oauthAccount: {
        accountUuid: "a",
        emailAddress: "b@c.d",
        organizationUuid: "o",
        scopes: ["user:inference"],
      },
      hasCompletedOnboarding: true,
    });
    expect(parseClaudeSessionBlob(blob)).toEqual({ ok: true });
  });

  it("rejects an empty blob with code=empty", () => {
    const r = parseClaudeSessionBlob("");
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("empty");
  });

  it("rejects whitespace-only input with code=empty", () => {
    const r = parseClaudeSessionBlob("   \n\t  ");
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("empty");
  });

  it("rejects non-JSON with code=invalid_json", () => {
    const r = parseClaudeSessionBlob("not json {");
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("invalid_json");
  });

  it("rejects a top-level JSON array with code=invalid_json (not an object)", () => {
    const r = parseClaudeSessionBlob("[1, 2, 3]");
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("invalid_json");
  });

  it("rejects a blob missing oauthAccount with code=missing_oauth_account", () => {
    const r = parseClaudeSessionBlob(JSON.stringify({ shellSnapshot: "x" }));
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("missing_oauth_account");
  });

  it("rejects a blob where oauthAccount is a string with code=missing_oauth_account", () => {
    const r = parseClaudeSessionBlob(
      JSON.stringify({ oauthAccount: "not-an-object" }),
    );
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("missing_oauth_account");
  });

  it("rejects oauthAccount missing a single required key (organizationUuid)", () => {
    const r = parseClaudeSessionBlob(
      JSON.stringify({
        oauthAccount: {
          accountUuid: "a",
          emailAddress: "b@c.d",
        },
      }),
    );
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.code).toBe("missing_required_fields");
      expect(r.message).toContain("organizationUuid");
    }
  });

  it("rejects oauthAccount whose required key is an empty string", () => {
    const r = parseClaudeSessionBlob(
      JSON.stringify({
        oauthAccount: {
          accountUuid: "a",
          emailAddress: "",
          organizationUuid: "o",
        },
      }),
    );
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.code).toBe("missing_required_fields");
      expect(r.message).toContain("emailAddress");
    }
  });

  it("rejects oauthAccount whose required key is not a string", () => {
    const r = parseClaudeSessionBlob(
      JSON.stringify({
        oauthAccount: {
          accountUuid: 123,
          emailAddress: "b@c.d",
          organizationUuid: "o",
        },
      }),
    );
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.code).toBe("missing_required_fields");
      expect(r.message).toContain("accountUuid");
    }
  });
});

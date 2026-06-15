// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { GitConfigError, putGitConfig } from "./gitConfig";
import type { GitConfigPatch } from "./types";

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  vi.restoreAllMocks();
});

const patch: GitConfigPatch = {
  base_branch: "develop",
  remote: "upstream",
};

describe("putGitConfig()", () => {
  it("PUTs the patch and returns the parsed ConfigResponse on 200", async () => {
    const updated = {
      git: { base_branch: "develop", remote: "upstream" },
      persisted: true,
    };
    const res = new Response(JSON.stringify(updated), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
    Object.defineProperty(res, "ok", { value: true });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);

    const got = await putGitConfig(patch);
    expect(fetch).toHaveBeenCalledWith("/api/config/git", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
      credentials: "same-origin",
    });
    expect(got).toEqual(updated);
  });

  it("throws GitConfigError with structured code on 400", async () => {
    const body = { error: "base_branch_empty", message: "Base branch must not be empty" };
    const res = new Response(JSON.stringify(body), { status: 400 });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);

    let caught: unknown;
    try {
      await putGitConfig(patch);
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(GitConfigError);
    const err = caught as GitConfigError;
    expect(err.code).toBe("base_branch_empty");
    expect(err.status).toBe(400);
    expect(err.message).toBe("Base branch must not be empty");
  });

  it("falls back to http_<status> when the server returns no JSON body (admin-gated 403)", async () => {
    const res = new Response("forbidden", { status: 403 });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);

    let caught: unknown;
    try {
      await putGitConfig(patch);
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(GitConfigError);
    const err = caught as GitConfigError;
    expect(err.code).toBe("http_403");
    expect(err.status).toBe(403);
    expect(err.message).toBe("forbidden");
  });
});

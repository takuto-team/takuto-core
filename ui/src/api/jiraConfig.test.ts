// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { JiraConfigError, putJiraConfig } from "./jiraConfig";
import type { JiraConfigPatch } from "./types";

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  vi.restoreAllMocks();
});

const patch: JiraConfigPatch = {
  linked_items_in_prompt: "summary_only",
  ticket_context_max_description_bytes: 4096,
  linked_issue_description_max_bytes: 0,
  jql_filter: 'labels = "maestro"',
  done_status: "Done",
  project_keys: ["PROJ", "OPS"],
};

describe("putJiraConfig()", () => {
  it("PUTs the patch and returns the parsed ConfigResponse on 200", async () => {
    const updated = {
      general: { ticketing_system: "jira" },
      jira: { project_keys: ["PROJ", "OPS"], site: "x.atlassian.net" },
      persisted: true,
    };
    const res = new Response(JSON.stringify(updated), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
    Object.defineProperty(res, "ok", { value: true });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);

    const got = await putJiraConfig(patch);
    expect(fetch).toHaveBeenCalledWith("/api/config/jira", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
      credentials: "same-origin",
    });
    expect(got).toEqual(updated);
  });

  it("throws JiraConfigError with structured code on 400", async () => {
    const body = { error: "invalid_jql", message: "JQL failed to parse" };
    const res = new Response(JSON.stringify(body), { status: 400 });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);

    let caught: unknown;
    try {
      await putJiraConfig(patch);
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(JiraConfigError);
    const err = caught as JiraConfigError;
    expect(err.code).toBe("invalid_jql");
    expect(err.status).toBe(400);
    expect(err.message).toBe("JQL failed to parse");
  });

  it("falls back to http_<status> when the server returns no JSON body", async () => {
    const res = new Response("forbidden", { status: 403 });
    Object.defineProperty(res, "ok", { value: false });
    (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(res);

    let caught: unknown;
    try {
      await putJiraConfig(patch);
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(JiraConfigError);
    const err = caught as JiraConfigError;
    expect(err.code).toBe("http_403");
    expect(err.status).toBe(403);
    expect(err.message).toBe("forbidden");
  });
});

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * RepoPollingSettingsSection — the per-user-per-repository polling section. A
 * repo is selected in the sidebar; the form loads from
 * `GET /api/me/polling-settings/{workspace}` and saves the per-repo fields via
 * `PUT /api/me/polling-settings/{workspace}`. The Jira filter block is gated to
 * `ticketingSystem === "jira"` and the GitHub block to `"github"`.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createRef } from "react";
import { render, screen, waitFor, cleanup, fireEvent, act } from "@testing-library/react";
import { RepoPollingSettingsSection } from "./RepoPollingSettingsSection";
import type { ConfigSectionHandle } from "./configSection";
import { createQueryWrapper } from "../../test/queryWrapper";

const REPO = "takuto-core";

let fetchMock: ReturnType<typeof vi.fn>;
let lastPutBody: Record<string, unknown> | null;

function json(body: unknown, status = 200): Response {
  const res = new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
  Object.defineProperty(res, "ok", { value: status >= 200 && status < 300 });
  return res;
}

const SETTINGS = {
  auto_polling: true,
  auto_start_flow: "",
  max_parallel_items: 0,
  project_keys: ["PROJ"],
  item_types: ["Task", "Bug"],
  jira_summary_keywords: [],
  github_labels: ["bug"],
  github_title_keywords: [],
  linked_items_in_prompt: "full",
  ticket_context_max_description_bytes: 0,
  linked_issue_description_max_bytes: 0,
  jql_filter: "",
  done_status: "Done",
};

beforeEach(() => {
  lastPutBody = null;
  try {
    localStorage.clear();
  } catch {
    /* ignore */
  }
  fetchMock = vi.fn(async (input: string, init?: RequestInit) => {
    const url = typeof input === "string" ? input : String(input);
    const method = init?.method ?? "GET";
    if (url === "/api/repositories") {
      return json([{ id: "r1", name: REPO, default_branch: "main", local_path: `/w/${REPO}` }]);
    }
    if (url === "/api/repositories/access") {
      return json([{ name: REPO, accessible: true }]);
    }
    if (url === "/api/me/polling-settings") {
      return json([{ workspace_name: REPO, settings: SETTINGS, updated_at: 1 }]);
    }
    if (url.startsWith("/api/me/flows")) {
      return json({ flows: [], workspace: REPO });
    }
    if (url === `/api/me/polling-settings/${REPO}` && method === "GET") {
      return json({ workspace_name: REPO, settings: SETTINGS, updated_at: 1 });
    }
    if (url === `/api/me/polling-settings/${REPO}` && method === "PUT") {
      lastPutBody = JSON.parse(String(init!.body));
      return json({
        workspace_name: REPO,
        settings: { ...SETTINGS, ...lastPutBody },
        updated_at: 2,
      });
    }
    return json({});
  });
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderSection(
  ticketingSystem: "jira" | "github",
  ref: React.RefObject<ConfigSectionHandle | null>,
) {
  const { wrapper: Wrapper } = createQueryWrapper();
  return render(
    <Wrapper>
      <RepoPollingSettingsSection ticketingSystem={ticketingSystem} ref={ref} />
    </Wrapper>,
  );
}

describe("RepoPollingSettingsSection", () => {
  it("loads the selected repo's settings and shows the Jira filter block in jira mode", async () => {
    const ref = createRef<ConfigSectionHandle>();
    const { container } = renderSection("jira", ref);
    await waitFor(() => {
      expect(container.querySelector("#jira-project-keys-input")).not.toBeNull();
    });
    // Jira block shown, GitHub block hidden.
    expect(container.querySelector("#jira-item-types-input")).not.toBeNull();
    expect(container.querySelector("#github-labels-input")).toBeNull();
  });

  it("keeps Jira project keys visible when auto-polling is OFF (manual Add-item needs them)", async () => {
    const OFF = { ...SETTINGS, auto_polling: false };
    fetchMock.mockImplementation(async (input: string, init?: RequestInit) => {
      const url = typeof input === "string" ? input : String(input);
      const method = init?.method ?? "GET";
      if (url === "/api/repositories") {
        return json([{ id: "r1", name: REPO, default_branch: "main", local_path: `/w/${REPO}` }]);
      }
      if (url === "/api/repositories/access") return json([{ name: REPO, accessible: true }]);
      if (url === "/api/me/polling-settings") {
        return json([{ workspace_name: REPO, settings: OFF, updated_at: 1 }]);
      }
      if (url.startsWith("/api/me/flows")) return json({ flows: [], workspace: REPO });
      if (url === `/api/me/polling-settings/${REPO}` && method === "GET") {
        return json({ workspace_name: REPO, settings: OFF, updated_at: 1 });
      }
      return json({});
    });
    const ref = createRef<ConfigSectionHandle>();
    const { container } = renderSection("jira", ref);
    // Project keys are ALWAYS shown for Jira, independent of the enable toggle.
    await waitFor(() => {
      expect(container.querySelector("#jira-project-keys-input")).not.toBeNull();
    });
    // The enable-gated tuning fields stay hidden while auto-polling is off.
    expect(container.querySelector("#jira-item-types-input")).toBeNull();
    expect(container.querySelector("#jql-filter-input")).toBeNull();
  });

  it("shows the GitHub filter block (and hides Jira) in github mode", async () => {
    const ref = createRef<ConfigSectionHandle>();
    const { container } = renderSection("github", ref);
    await waitFor(() => {
      expect(container.querySelector("#github-labels-input")).not.toBeNull();
    });
    expect(container.querySelector("#jira-project-keys-input")).toBeNull();
  });

  it("PUTs the edited per-repo settings (keys preserved) to the selected workspace", async () => {
    const ref = createRef<ConfigSectionHandle>();
    const { container } = renderSection("jira", ref);
    await waitFor(() => {
      expect(container.querySelector("#jql-filter-input")).not.toBeNull();
    });

    fireEvent.change(container.querySelector("#jql-filter-input")!, {
      target: { value: "status = Open" },
    });
    fireEvent.change(container.querySelector("#max-parallel-items-input")!, {
      target: { value: "5" },
    });

    expect(ref.current?.isDirty()).toBe(true);

    await act(async () => {
      await ref.current!.save();
    });

    expect(lastPutBody).toMatchObject({
      project_keys: ["PROJ"],
      jql_filter: "status = Open",
      max_parallel_items: 5,
      auto_polling: true,
    });
    // The PUT targeted the selected repo's workspace endpoint.
    const putCall = fetchMock.mock.calls.find(
      ([u, i]) => u === `/api/me/polling-settings/${REPO}` && i?.method === "PUT",
    );
    expect(putCall).toBeTruthy();
  });

  it("renders the empty-state prompt when the user has no repositories", async () => {
    fetchMock.mockImplementation(async (input: string) => {
      const url = typeof input === "string" ? input : String(input);
      if (url === "/api/repositories") return json([]);
      if (url === "/api/repositories/access") return json([]);
      if (url === "/api/me/polling-settings") return json([]);
      return json({});
    });
    const ref = createRef<ConfigSectionHandle>();
    renderSection("jira", ref);
    expect(await screen.findByText("Select a repository to configure its polling.")).toBeTruthy();
  });
});

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MyRepositoriesTab } from "./MyRepositoriesTab";
import type { RepositoryRow } from "../api/client";
import type { GitHubRepo } from "../api/types";

function repoRow(overrides: Partial<RepositoryRow> = {}): RepositoryRow {
  return {
    id: "r1",
    name: "acme/app",
    repo_url: "https://github.com/acme/app",
    local_path: "/data/acme/app",
    default_branch: "main",
    co_users_count: 0,
    ...overrides,
  };
}

function ghRepo(overrides: Partial<GitHubRepo> = {}): GitHubRepo {
  return {
    full_name: "acme/other",
    description: "another repo",
    private: false,
    html_url: "https://github.com/acme/other",
    ...overrides,
  };
}

interface MockState {
  mine: RepositoryRow[];
  available: GitHubRepo[];
}

function installFetch(state: MockState) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init?: RequestInit) => {
      const url = String(input);
      const method = init?.method ?? "GET";
      if (url === "/api/repositories" && method === "GET") {
        return new Response(JSON.stringify(state.mine), { status: 200 });
      }
      if (url.startsWith("/api/github/repos") && method === "GET") {
        return new Response(JSON.stringify(state.available), { status: 200 });
      }
      if (url === "/api/repositories" && method === "POST") {
        const added = repoRow({ id: "r-new", name: "acme/other" });
        state.mine = [...state.mine, added];
        return new Response(JSON.stringify(added), { status: 200 });
      }
      if (url.startsWith("/api/repositories/") && method === "DELETE") {
        const id = decodeURIComponent(url.split("/").pop() ?? "");
        state.mine = state.mine.filter((r) => r.id !== id);
        return new Response(null, { status: 204 });
      }
      return new Response("not found", { status: 404 });
    }),
  );
}

function renderTab(isAdmin = false) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <MyRepositoriesTab isAdmin={isAdmin} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("MyRepositoriesTab", () => {
  it("lists my repositories after load", async () => {
    installFetch({ mine: [repoRow()], available: [] });
    renderTab();

    await waitFor(() => {
      expect(screen.getByText("acme/app")).toBeTruthy();
    });
    expect(screen.getByText("/data/acme/app")).toBeTruthy();
  });

  it("shows available repos not already added, and hides ones already mine", async () => {
    installFetch({
      mine: [repoRow({ id: "r1", repo_url: "https://github.com/acme/app" })],
      available: [
        ghRepo({ full_name: "acme/app", html_url: "https://github.com/acme/app" }),
        ghRepo({ full_name: "acme/other", html_url: "https://github.com/acme/other" }),
      ],
    });
    renderTab();

    // acme/other is addable; acme/app is already mine → filtered out of available.
    await waitFor(() => {
      expect(screen.getByText("acme/other")).toBeTruthy();
    });
    // "acme/app" appears once (in My repositories), not in the available list.
    expect(screen.getAllByText("acme/app")).toHaveLength(1);
  });

  it("adds a repository from the available list", async () => {
    installFetch({
      mine: [],
      available: [ghRepo({ full_name: "acme/other", html_url: "https://github.com/acme/other" })],
    });
    renderTab();

    const addBtn = await waitFor(() => screen.getByRole("button", { name: /^add$/i }));
    fireEvent.click(addBtn);

    await waitFor(() => {
      expect(screen.getByText(/Added "acme\/other" to your dashboard/i)).toBeTruthy();
    });
  });

  it("removes a repository after confirming", async () => {
    installFetch({ mine: [repoRow()], available: [] });
    renderTab();

    const removeBtn = await waitFor(() => screen.getByRole("button", { name: /^remove$/i }));
    fireEvent.click(removeBtn);

    // Confirm modal appears (title "Remove repository"); click its Confirm button.
    expect(screen.getByText(/^Remove repository$/i)).toBeTruthy();
    const confirmBtn = await waitFor(() => screen.getByRole("button", { name: /^confirm$/i }));
    fireEvent.click(confirmBtn);

    await waitFor(() => {
      expect(screen.getByText(/Removed "acme\/app" from your dashboard/i)).toBeTruthy();
    });
  });

  it("shows the Force purge button only for admins", async () => {
    installFetch({ mine: [repoRow()], available: [] });
    renderTab(true);

    await waitFor(() => {
      expect(screen.getByRole("button", { name: /force purge/i })).toBeTruthy();
    });
  });
});

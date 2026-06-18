// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { ToastProvider } from "../../hooks/useToast";
import { TicketDetailModal } from "./TicketDetailModal";

interface Call {
  url: string;
  method: string;
  body?: Record<string, unknown>;
}

const calls: Call[] = [];

function installFetch() {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init?: RequestInit) => {
      const url = String(input);
      const method = init?.method ?? "GET";
      const body = init?.body ? JSON.parse(String(init.body)) : undefined;
      calls.push({ url, method, body });
      if (url === "/api/repositories" && method === "GET") {
        return new Response(
          JSON.stringify([
            {
              id: "r1",
              name: "quantum-budget",
              repo_url: "https://github.com/acme/quantum-budget",
              local_path: "/data/qb",
              default_branch: "main",
              co_users_count: 0,
            },
          ]),
          { status: 200 },
        );
      }
      if (url.includes("/update-description") && method === "POST") {
        return new Response(null, { status: 200 });
      }
      return new Response("not found", { status: 404 });
    }),
  );
}

beforeEach(() => {
  calls.length = 0;
  installFetch();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderModal() {
  const onStart = vi.fn();
  const { container } = render(
    <ToastProvider>
      <MemoryRouter>
        <TicketDetailModal
          ticketKey="GH-26"
          summary="Original title"
          description="Original body"
          ticketingSystem="github"
          showStartButton
          activeRepoName="quantum-budget"
          onStart={onStart}
          onClose={() => {}}
        />
      </MemoryRouter>
    </ToastProvider>,
  );
  return { onStart, container };
}

async function addButtonEnabled() {
  const btn = (await screen.findByRole("button", { name: /add to dashboard/i })) as HTMLButtonElement;
  await waitFor(() => expect(btn.disabled).toBe(false));
  return btn;
}

describe("TicketDetailModal — Add to Dashboard save-first", () => {
  it("persists unsaved edits before creating the work item", async () => {
    const { onStart, container } = renderModal();
    await addButtonEnabled();

    fireEvent.click(screen.getByRole("button", { name: /^edit$/i }));
    const textarea = await waitFor(() => {
      const el = container.querySelector("textarea");
      if (!el) throw new Error("textarea not mounted");
      return el;
    });
    fireEvent.change(textarea, { target: { value: "Edited body" } });

    fireEvent.click(screen.getByRole("button", { name: /add to dashboard/i }));

    await waitFor(() => expect(onStart).toHaveBeenCalled());
    const saveCall = calls.find((c) => c.url.includes("/update-description"));
    expect(saveCall).toBeTruthy();
    expect(saveCall?.body?.description).toBe("Edited body");
    // The work item is created with the edited (now-saved) content.
    expect(onStart).toHaveBeenCalledWith("Edited body", "Original title", "r1");
  });

  it("does not call save when the description was never edited", async () => {
    const { onStart } = renderModal();
    await addButtonEnabled();

    fireEvent.click(screen.getByRole("button", { name: /add to dashboard/i }));

    await waitFor(() => expect(onStart).toHaveBeenCalledWith("Original body", "Original title", "r1"));
    expect(calls.some((c) => c.url.includes("/update-description"))).toBe(false);
  });

  it("aborts the add when the save fails", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string, init?: RequestInit) => {
        const url = String(input);
        const method = init?.method ?? "GET";
        const body = init?.body ? JSON.parse(String(init.body)) : undefined;
        calls.push({ url, method, body });
        if (url === "/api/repositories" && method === "GET") {
          return new Response(
            JSON.stringify([
              { id: "r1", name: "quantum-budget", repo_url: "", local_path: "", default_branch: "main", co_users_count: 0 },
            ]),
            { status: 200 },
          );
        }
        if (url.includes("/update-description") && method === "POST") {
          return new Response("boom", { status: 500 });
        }
        return new Response("not found", { status: 404 });
      }),
    );

    const onStart = vi.fn();
    const { container } = render(
      <ToastProvider>
        <MemoryRouter>
          <TicketDetailModal
            ticketKey="GH-26"
            summary="Original title"
            description="Original body"
            ticketingSystem="github"
            showStartButton
            activeRepoName="quantum-budget"
            onStart={onStart}
            onClose={() => {}}
          />
        </MemoryRouter>
      </ToastProvider>,
    );
    await addButtonEnabled();

    fireEvent.click(screen.getByRole("button", { name: /^edit$/i }));
    const textarea = await waitFor(() => {
      const el = container.querySelector("textarea");
      if (!el) throw new Error("textarea not mounted");
      return el;
    });
    fireEvent.change(textarea, { target: { value: "Edited body" } });
    fireEvent.click(screen.getByRole("button", { name: /add to dashboard/i }));

    // The save was attempted and failed, so the work item is never created.
    await waitFor(() =>
      expect(calls.some((c) => c.url.includes("/update-description"))).toBe(true),
    );
    expect(onStart).not.toHaveBeenCalled();
  });
});

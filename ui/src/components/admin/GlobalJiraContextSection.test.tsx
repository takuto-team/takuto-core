// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * GlobalJiraContextSection — the deployment-global Jira-context *processing*
 * fields (linked-items mode, the two byte caps, the Mark-as-Done target) saved
 * via `PUT /api/config/jira`. These four are consumed GLOBALLY by the engine;
 * this section is the ONLY UI that edits them (the per-repo polling section does
 * not expose them). That is the desync guard: a user editing these here changes
 * real engine behaviour, and there is no shadow per-repo copy that silently does
 * nothing.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createRef } from "react";
import { render, screen, waitFor, cleanup, fireEvent, act } from "@testing-library/react";
import { GlobalJiraContextSection } from "./GlobalJiraContextSection";
import type { ConfigSectionHandle } from "./configSection";
import { ToastProvider } from "../../hooks/useToast";

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

const CONFIG = {
  jira: {
    linked_items_in_prompt: "full",
    ticket_context_max_description_bytes: 0,
    linked_issue_description_max_bytes: 0,
    done_status: "Done",
  },
};

beforeEach(() => {
  lastPutBody = null;
  fetchMock = vi.fn(async (input: string, init?: RequestInit) => {
    const url = typeof input === "string" ? input : String(input);
    if (url === "/api/config" && (!init || init.method === undefined)) {
      return json(CONFIG);
    }
    if (url === "/api/config/jira" && init?.method === "PUT") {
      lastPutBody = JSON.parse(String(init.body));
      return json({ jira: { ...CONFIG.jira, ...lastPutBody }, persisted: true });
    }
    return json({});
  });
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderSection(ref: React.RefObject<ConfigSectionHandle | null>) {
  return render(
    <ToastProvider>
      <GlobalJiraContextSection ref={ref} />
    </ToastProvider>,
  );
}

describe("GlobalJiraContextSection", () => {
  it("loads the four global jira-context fields from /api/config", async () => {
    const ref = createRef<ConfigSectionHandle>();
    const { container } = renderSection(ref);
    await screen.findByText("Jira context");
    await waitFor(() => {
      expect(
        container.querySelector<HTMLInputElement>("#done-status-input")?.value,
      ).toBe("Done");
    });
    expect(ref.current?.isDirty()).toBe(false);
  });

  it("PUTs all four fields to /api/config/jira when edited", async () => {
    const ref = createRef<ConfigSectionHandle>();
    const { container } = renderSection(ref);
    await waitFor(() => {
      expect(container.querySelector<HTMLInputElement>("#done-status-input")?.value).toBe("Done");
    });

    fireEvent.change(container.querySelector("#linked-items-in-prompt-select")!, {
      target: { value: "summary_only" },
    });
    fireEvent.change(container.querySelector("#ticket-context-max-description-bytes-input")!, {
      target: { value: "4096" },
    });
    fireEvent.change(container.querySelector("#linked-issue-description-max-bytes-input")!, {
      target: { value: "1024" },
    });
    fireEvent.change(container.querySelector("#done-status-input")!, {
      target: { value: "Closed" },
    });

    await act(async () => {
      await ref.current!.save();
    });

    expect(lastPutBody).toEqual({
      linked_items_in_prompt: "summary_only",
      ticket_context_max_description_bytes: 4096,
      linked_issue_description_max_bytes: 1024,
      done_status: "Closed",
    });
  });

  it("omits done_status from the PUT when it is blank (server rejects blank)", async () => {
    const ref = createRef<ConfigSectionHandle>();
    const { container } = renderSection(ref);
    await waitFor(() => {
      expect(container.querySelector<HTMLInputElement>("#done-status-input")?.value).toBe("Done");
    });

    fireEvent.change(container.querySelector("#done-status-input")!, {
      target: { value: "   " },
    });

    await act(async () => {
      await ref.current!.save();
    });

    expect(lastPutBody).not.toHaveProperty("done_status");
    // The other three are still sent.
    expect(lastPutBody).toHaveProperty("linked_items_in_prompt");
    expect(lastPutBody).toHaveProperty("ticket_context_max_description_bytes");
    expect(lastPutBody).toHaveProperty("linked_issue_description_max_bytes");
  });
});

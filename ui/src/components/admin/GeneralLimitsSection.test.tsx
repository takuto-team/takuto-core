// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * GeneralLimitsSection — the deployment-global `[general]` limits that ride the
 * trimmed `PUT /api/config/polling`. Verifies the section loads its values from
 * `/api/config`, exposes the shared `ConfigSectionHandle`, and PUTs only the
 * deployment-wide knobs (NOT the per-repo polling fields, which moved to
 * `/api/me/polling-settings`).
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createRef } from "react";
import { render, screen, waitFor, cleanup, fireEvent, act } from "@testing-library/react";
import { GeneralLimitsSection } from "./GeneralLimitsSection";
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
  general: {
    poll_interval_secs: 60,
    max_concurrent_manual_workflows: 0,
    pr_merge_poll_interval_secs: 90,
    generate_report: false,
    work_item_log_retention_days: 0,
  },
  polling: { max_parallel_per_user: false },
};

beforeEach(() => {
  lastPutBody = null;
  fetchMock = vi.fn(async (input: string, init?: RequestInit) => {
    const url = typeof input === "string" ? input : String(input);
    if (url === "/api/config" && (!init || init.method === undefined)) {
      return json(CONFIG);
    }
    if (url === "/api/config/polling" && init?.method === "PUT") {
      lastPutBody = JSON.parse(String(init.body));
      // Echo a fresh config reflecting the patch.
      return json({
        general: { ...CONFIG.general, ...lastPutBody },
        polling: { max_parallel_per_user: lastPutBody?.max_parallel_per_user ?? false },
        persisted: true,
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

function renderSection(ref: React.RefObject<ConfigSectionHandle | null>) {
  return render(
    <ToastProvider>
      <GeneralLimitsSection ref={ref} />
    </ToastProvider>,
  );
}

describe("GeneralLimitsSection", () => {
  it("loads the deployment limits from /api/config", async () => {
    const ref = createRef<ConfigSectionHandle>();
    const { container } = renderSection(ref);
    await screen.findByText("General limits");
    await waitFor(() => {
      const input = container.querySelector<HTMLInputElement>("#poll-interval-input");
      expect(input?.value).toBe("60");
    });
    // Nothing edited yet → not dirty, save is a no-op that resolves true.
    expect(ref.current?.isDirty()).toBe(false);
  });

  it("PUTs the edited deployment limits to /api/config/polling via the section handle", async () => {
    const ref = createRef<ConfigSectionHandle>();
    const { container } = renderSection(ref);
    await waitFor(() => {
      expect(container.querySelector<HTMLInputElement>("#poll-interval-input")?.value).toBe("60");
    });

    fireEvent.change(container.querySelector("#poll-interval-input")!, {
      target: { value: "120" },
    });
    fireEvent.change(container.querySelector("#max-concurrent-manual-input")!, {
      target: { value: "3" },
    });
    fireEvent.change(container.querySelector("#work-item-log-retention-input")!, {
      target: { value: "14" },
    });

    expect(ref.current?.isDirty()).toBe(true);

    let ok = false;
    await act(async () => {
      ok = (await ref.current!.save()) ?? false;
    });
    expect(ok).toBe(true);

    expect(lastPutBody).toMatchObject({
      poll_interval_secs: 120,
      max_concurrent_manual_workflows: 3,
      work_item_log_retention_days: 14,
    });
    // Per-repo fields must NEVER be part of this global PUT.
    expect(lastPutBody).not.toHaveProperty("project_keys");
    expect(lastPutBody).not.toHaveProperty("auto_polling");
    expect(lastPutBody).not.toHaveProperty("max_parallel_items");
  });
});

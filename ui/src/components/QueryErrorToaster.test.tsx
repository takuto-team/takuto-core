// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, afterEach } from "vitest";
import { render, screen, act, cleanup } from "@testing-library/react";
import { QueryErrorToaster } from "./QueryErrorToaster";
import { ToastProvider, ToastContainer } from "../hooks/useToast";
import { emitFetchError } from "../api/fetchErrorBus";

function renderToaster() {
  return render(
    <ToastProvider>
      <QueryErrorToaster />
      <ToastContainer />
    </ToastProvider>,
  );
}

afterEach(() => {
  cleanup();
});

describe("QueryErrorToaster", () => {
  it("shows a toast when a fetch error is emitted", () => {
    renderToaster();
    act(() => emitFetchError("HTTP 500"));
    expect(screen.getByText(/Couldn't reach the server: HTTP 500/i)).toBeTruthy();
  });

  it("coalesces identical messages emitted in quick succession", () => {
    renderToaster();
    act(() => {
      emitFetchError("network down");
      emitFetchError("network down");
      emitFetchError("network down");
    });
    expect(screen.getAllByText(/Couldn't reach the server: network down/i)).toHaveLength(1);
  });

  it("shows distinct toasts for different messages", () => {
    renderToaster();
    act(() => {
      emitFetchError("first");
      emitFetchError("second");
    });
    expect(screen.getByText(/Couldn't reach the server: first/i)).toBeTruthy();
    expect(screen.getByText(/Couldn't reach the server: second/i)).toBeTruthy();
  });
});

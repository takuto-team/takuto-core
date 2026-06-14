// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, afterEach } from "vitest";
import { render, screen, act, cleanup } from "@testing-library/react";
import { surfaceError } from "./surfaceError";
import { onFetchError } from "../api/fetchErrorBus";
import { QueryErrorToaster } from "../components/QueryErrorToaster";
import { ToastProvider, ToastContainer } from "../hooks/useToast";

afterEach(() => {
  cleanup();
});

describe("surfaceError", () => {
  it("normalizes an Error and emits its message on the fetch-error bus", () => {
    const seen: string[] = [];
    const off = onFetchError((m) => seen.push(m));
    surfaceError(new Error("boom"));
    off();
    expect(seen).toContain("boom");
  });

  it("prefixes the context when provided", () => {
    const seen: string[] = [];
    const off = onFetchError((m) => seen.push(m));
    surfaceError(new Error("HTTP 500"), "Couldn't load users");
    off();
    expect(seen).toContain("Couldn't load users: HTTP 500");
  });

  it("stringifies non-Error values", () => {
    const seen: string[] = [];
    const off = onFetchError((m) => seen.push(m));
    surfaceError("plain string");
    off();
    expect(seen).toContain("plain string");
  });

  it("surfaces through to a visible toast via QueryErrorToaster", () => {
    render(
      <ToastProvider>
        <QueryErrorToaster />
        <ToastContainer />
      </ToastProvider>,
    );
    act(() => surfaceError(new Error("network down"), "Couldn't load users"));
    expect(
      screen.getByText(/Couldn't reach the server: Couldn't load users: network down/i),
    ).toBeTruthy();
  });
});

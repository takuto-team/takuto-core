// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect } from "vitest";
import { pickDefaultRepo } from "./pickDefaultRepo";

describe("pickDefaultRepo", () => {
  it("prefers the active repo when present and accessible", () => {
    expect(pickDefaultRepo(["a", "b"], "b", {})).toBe("b");
    expect(pickDefaultRepo(["a", "b"], "b", { b: true })).toBe("b");
  });

  it("skips the active repo when it is inaccessible, choosing the first accessible", () => {
    expect(pickDefaultRepo(["a", "b"], "b", { b: false })).toBe("a");
  });

  it("falls back to the first accessible when the active repo is absent", () => {
    expect(pickDefaultRepo(["a", "b"], "missing", {})).toBe("a");
    expect(pickDefaultRepo(["a", "b"], null, { a: false })).toBe("b");
  });

  it("falls back to the first repo when none are accessible", () => {
    expect(pickDefaultRepo(["a", "b"], null, { a: false, b: false })).toBe("a");
  });

  it("returns null for an empty list", () => {
    expect(pickDefaultRepo([], "x", {})).toBeNull();
  });
});

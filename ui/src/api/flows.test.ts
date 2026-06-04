// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect } from "vitest";
import { propagateRename, type UserFlow } from "./flows";

function flow(name: string, depends_on: string[] = []): UserFlow {
  return { name, depends_on, steps: [{ name: "s", prompt: "p", skills: [] }] };
}

describe("propagateRename", () => {
  it("rewrites every dependent's depends_on entry", () => {
    const before: UserFlow[] = [
      flow("Implement Ticket"),
      flow("Address Comments", ["Implement Ticket"]),
      flow("Merge", ["Implement Ticket"]),
    ];
    const after = propagateRename(before, "Implement Ticket", "Implement Stuff");
    expect(after[0].depends_on).toEqual([]);
    expect(after[1].depends_on).toEqual(["Implement Stuff"]);
    expect(after[2].depends_on).toEqual(["Implement Stuff"]);
    // Flow names themselves are not touched by this helper — callers rewrite
    // the renamed flow's own `name` field separately.
    expect(after.map((f) => f.name)).toEqual([
      "Implement Ticket",
      "Address Comments",
      "Merge",
    ]);
  });

  it("returns the same reference when the names are equal", () => {
    const before: UserFlow[] = [flow("A"), flow("B", ["A"])];
    expect(propagateRename(before, "A", "A")).toBe(before);
  });

  it("leaves unrelated depends_on entries alone", () => {
    const before: UserFlow[] = [
      flow("A"),
      flow("B"),
      flow("C", ["A", "B"]),
    ];
    const after = propagateRename(before, "A", "Z");
    expect(after[2].depends_on).toEqual(["Z", "B"]);
  });
});

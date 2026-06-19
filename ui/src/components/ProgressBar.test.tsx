// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, afterEach } from "vitest";
import { render, cleanup } from "@testing-library/react";
import { ProgressBar } from "./ProgressBar";

afterEach(cleanup);

const LIGHT_BLUE = "rgb(147, 197, 253)"; // #93c5fd — in progress
const BLUE = "rgb(59, 130, 246)"; // #3b82f6 — completed in a running flow
const GREEN = "rgb(34, 197, 94)"; // #22c55e — completed in a completed flow
const PENDING = "rgb(75, 85, 99)"; // #4b5563 — pending

function segments(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll<HTMLElement>(".flex.gap-0\\.5 > div")).map(
    (el) => el.style.backgroundColor,
  );
}

describe("ProgressBar segmented", () => {
  it("paints the in-progress step light blue and pending steps grey", () => {
    const { container } = render(
      <ProgressBar pct={0} total={6} filled={0} color="blue" activeIndex={0} />,
    );
    const segs = segments(container);
    expect(segs).toHaveLength(6);
    expect(segs[0]).toBe(LIGHT_BLUE);
    expect(segs[1]).toBe(PENDING);
  });

  it("running flow: completed steps blue, current step light blue, rest grey", () => {
    const { container } = render(
      <ProgressBar pct={33} total={6} filled={2} color="blue" activeIndex={2} />,
    );
    const segs = segments(container);
    expect(segs[0]).toBe(BLUE);
    expect(segs[1]).toBe(BLUE);
    expect(segs[2]).toBe(LIGHT_BLUE);
    expect(segs[3]).toBe(PENDING);
  });

  it("completed flow: every step green, no in-progress segment", () => {
    const { container } = render(
      <ProgressBar pct={100} total={3} filled={3} color="green" activeIndex={null} />,
    );
    const segs = segments(container);
    expect(segs).toEqual([GREEN, GREEN, GREEN]);
    expect(segs).not.toContain(LIGHT_BLUE);
  });
});

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * The global Jira auth-failure modal host: shows ONE modal when
 * `emitJiraAuthFailure` fires (de-duped across concurrent failures), routes to
 * Config → Ticketing on the primary CTA, and closes without navigating on
 * Dismiss.
 */

import { describe, it, expect, afterEach } from "vitest";
import { render, screen, cleanup, fireEvent, act } from "@testing-library/react";
import { MemoryRouter, Routes, Route, useLocation } from "react-router-dom";
import { JiraAuthFailedModalHost } from "./JiraAuthFailedModalHost";
import { emitJiraAuthFailure } from "../api/jiraAuthFailure";

function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="loc">{`${loc.pathname}${loc.search}`}</div>;
}

function renderHost() {
  return render(
    <MemoryRouter initialEntries={["/"]}>
      <JiraAuthFailedModalHost />
      <Routes>
        <Route path="*" element={<LocationProbe />} />
      </Routes>
    </MemoryRouter>,
  );
}

afterEach(() => cleanup());

const TITLE = "Jira authentication failed";

describe("JiraAuthFailedModalHost", () => {
  it("is hidden until a Jira auth-failure is emitted", () => {
    renderHost();
    expect(screen.queryByText(TITLE)).toBeNull();
  });

  it("shows the modal on emit and routes to Config → Ticketing on the CTA", async () => {
    renderHost();
    act(() => emitJiraAuthFailure());
    expect(await screen.findByText(TITLE)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Update Jira token" }));

    expect(screen.getByTestId("loc").textContent).toBe("/config.html?tab=ticketing");
    // Modal closes after navigating.
    expect(screen.queryByText(TITLE)).toBeNull();
  });

  it("de-dupes to a single modal when emitted multiple times", () => {
    renderHost();
    act(() => {
      emitJiraAuthFailure();
      emitJiraAuthFailure();
      emitJiraAuthFailure();
    });
    expect(screen.getAllByText(TITLE)).toHaveLength(1);
  });

  it("Dismiss closes the modal without navigating", () => {
    renderHost();
    act(() => emitJiraAuthFailure());
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText(TITLE)).toBeNull();
    expect(screen.getByTestId("loc").textContent).toBe("/");
  });
});

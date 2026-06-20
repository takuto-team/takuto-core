// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * RepoSidebar: accessible repos are clickable; inaccessible ones are sorted to
 * the end, shown with a red "No access" label, and are not clickable. The
 * optional set/none badge renders only when `hasCommands` is provided.
 */

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, cleanup, fireEvent, within } from "@testing-library/react";
import { RepoSidebar, type RepoSidebarItem } from "./RepoSidebar";

afterEach(cleanup);

describe("RepoSidebar", () => {
  it("renders accessible repos as clickable buttons", () => {
    const onSelect = vi.fn();
    render(
      <RepoSidebar
        repos={[{ name: "alpha" }, { name: "beta" }]}
        loading={false}
        selected="alpha"
        onSelect={onSelect}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /beta/i }));
    expect(onSelect).toHaveBeenCalledWith("beta");
  });

  it("disables an inaccessible repo: no button, red 'No access', not clickable", () => {
    const onSelect = vi.fn();
    render(
      <RepoSidebar
        repos={[{ name: "gone", accessible: false }]}
        loading={false}
        selected={null}
        onSelect={onSelect}
      />,
    );
    expect(screen.queryByRole("button", { name: /gone/i })).toBeNull();
    expect(screen.getByText("No access")).toBeTruthy();
    fireEvent.click(screen.getByText("gone"));
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("sorts inaccessible repos to the end, keeping accessible order", () => {
    const repos: RepoSidebarItem[] = [
      { name: "no1", accessible: false },
      { name: "ok1", accessible: true },
      { name: "no2", accessible: false },
      { name: "ok2", accessible: true },
    ];
    render(
      <RepoSidebar repos={repos} loading={false} selected={null} onSelect={() => {}} />,
    );
    const order = within(screen.getByRole("list"))
      .getAllByText(/ok1|ok2|no1|no2/)
      .map((el) => el.textContent);
    expect(order).toEqual(["ok1", "ok2", "no1", "no2"]);
  });
});

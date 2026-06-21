// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Regression: the editor and terminal menus are separate sessions. "Stop
 * editor" must call the editor-close handler and "Stop terminal" the
 * terminal-close handler — they were both wired to `onCloseEditor`, so
 * stopping the terminal silently stopped nothing (it closed the editor's
 * routing instead and left ttyd running).
 */

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, within } from "@testing-library/react";
import { IssueCardFooter } from "./IssueCardFooter";

afterEach(cleanup);

function renderFooter(overrides: Partial<Parameters<typeof IssueCardFooter>[0]> = {}) {
  const handlers = {
    onSetMenu: vi.fn(),
    onOpenEditor: vi.fn(),
    onOpenTerminal: vi.fn(),
    onCloseEditor: vi.fn(),
    onCloseTerminal: vi.fn(),
  };
  render(
    <IssueCardFooter
      canOpenEditor
      editorUrl="https://example.test/s/editor/"
      terminalUrl="https://example.test/s/terminal/"
      ports={[]}
      openMenu={null}
      {...handlers}
      {...overrides}
    />,
  );
  return handlers;
}

/** Open the named menu and click its "Stop …" item. */
function stopVia(menuTitle: "Editor" | "Terminal", stopLabel: string) {
  // The menu panel is keyed by its title heading; find the panel that contains it.
  const heading = screen.getByText(menuTitle);
  const panel = heading.parentElement as HTMLElement;
  fireEvent.click(within(panel).getByText(stopLabel));
}

describe("IssueCardFooter stop wiring", () => {
  it("Stop terminal calls onCloseTerminal, not onCloseEditor", () => {
    const h = renderFooter({ openMenu: "terminal" });
    stopVia("Terminal", "Stop terminal");
    expect(h.onCloseTerminal).toHaveBeenCalledTimes(1);
    expect(h.onCloseEditor).not.toHaveBeenCalled();
  });

  it("Stop editor calls onCloseEditor, not onCloseTerminal", () => {
    const h = renderFooter({ openMenu: "editor" });
    stopVia("Editor", "Stop editor");
    expect(h.onCloseEditor).toHaveBeenCalledTimes(1);
    expect(h.onCloseTerminal).not.toHaveBeenCalled();
  });
});

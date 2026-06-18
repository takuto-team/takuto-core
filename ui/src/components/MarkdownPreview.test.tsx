// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, waitFor, cleanup } from "@testing-library/react";
import { MarkdownPreview } from "./MarkdownPreview";

// mermaid pulls in heavy ESM and measures real layout (unavailable in jsdom),
// so mock the dynamic import with a render() that echoes a deterministic SVG.
const renderMock = vi.fn(async (id: string, source: string) => ({
  svg: `<svg data-id="${id}" data-src="${source.split("\n")[0]}"><text>diagram</text></svg>`,
}));
vi.mock("mermaid", () => ({
  default: { initialize: vi.fn(), render: renderMock },
}));

afterEach(() => {
  cleanup();
  renderMock.mockClear();
});

const MERMAID_DOC = [
  "# Heading",
  "",
  "```mermaid",
  "sequenceDiagram",
  "    participant A",
  "    participant B",
  "    A->>B: hi",
  "```",
  "",
  "Trailing text.",
].join("\n");

describe("MarkdownPreview mermaid rendering", () => {
  it("renders fenced mermaid blocks to SVG (read-only path)", async () => {
    const { container } = render(<MarkdownPreview markdown={MERMAID_DOC} />);

    await waitFor(() => {
      expect(container.querySelector(".mermaid-rendered svg")).toBeTruthy();
    });

    // The diagram source reached mermaid.render with its newlines intact.
    expect(renderMock).toHaveBeenCalledWith(
      expect.stringMatching(/^mmd-\d+-0$/),
      expect.stringContaining("sequenceDiagram"),
    );
    // Surrounding markdown still rendered as normal HTML.
    expect(container.querySelector("h1")?.textContent).toBe("Heading");
  });

  it("falls back to legible source when a diagram fails to parse", async () => {
    renderMock.mockRejectedValueOnce(new Error("bad diagram"));
    const { container } = render(<MarkdownPreview markdown={MERMAID_DOC} />);

    await waitFor(() => {
      expect(container.querySelector(".mermaid-error")).toBeTruthy();
    });
    const fallback = container.querySelector(".mermaid-error") as HTMLElement;
    expect(fallback.textContent).toContain("sequenceDiagram");
    expect(fallback.textContent).toContain("A->>B: hi");
  });
});

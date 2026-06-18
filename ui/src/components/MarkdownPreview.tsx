// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useRef, useMemo, memo } from "react";
import { marked } from "marked";
import DOMPurify from "dompurify";

// Collect mermaid blocks during parse, replace them with placeholder divs.
// After DOMPurify runs (which would mangle raw mermaid syntax), we inject
// the original source into the placeholders and let mermaid.run() render them.
let mermaidBlocks: string[] = [];

marked.use({
  renderer: {
    code({ text, lang }: { text: string; lang?: string }): string | false {
      if (lang === "mermaid") {
        const idx = mermaidBlocks.length;
        mermaidBlocks.push(text);
        return `<div data-mermaid-idx="${idx}" class="mermaid-placeholder"></div>`;
      }
      return false; // fall through to default renderer
    },
  },
});

// Singleton promise so mermaid is loaded and initialised exactly once.
let mermaidReady: Promise<(typeof import("mermaid"))["default"]> | null = null;

function getMermaid() {
  if (!mermaidReady) {
    mermaidReady = import("mermaid").then(({ default: m }) => {
      m.initialize({ startOnLoad: false, theme: "dark", securityLevel: "strict" });
      return m;
    });
  }
  return mermaidReady;
}

// Per-instance counter so the ids handed to mermaid.render() are unique across
// every MarkdownPreview mounted on the page (read-only view + edit preview can
// coexist), avoiding id collisions between their diagrams.
let nextPreviewId = 0;

interface Props {
  markdown: string;
  className?: string;
}

export const MarkdownPreview = memo(function MarkdownPreview({ markdown, className }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const idBaseRef = useRef(0);
  if (idBaseRef.current === 0) idBaseRef.current = ++nextPreviewId;

  const { html, blocks } = useMemo(() => {
    mermaidBlocks = [];
    const raw = marked.parse(markdown) as string;
    const sanitized = DOMPurify.sanitize(raw, {
      ADD_ATTR: ["data-mermaid-idx"],
    });
    return { html: sanitized, blocks: [...mermaidBlocks] };
  }, [markdown]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container || blocks.length === 0) return;

    const placeholders = Array.from(
      container.querySelectorAll<HTMLElement>("[data-mermaid-idx]")
    );
    if (placeholders.length === 0) return;

    let cancelled = false;
    // Render each diagram imperatively with mermaid.render(): unlike
    // mermaid.run(), it does not depend on node attributes surviving, never
    // skips an "already processed" node, and is idempotent across re-renders
    // and remounts — so the read-only view renders identically to the editor
    // preview regardless of mount order or timing.
    getMermaid().then(async (mermaid) => {
      for (const el of placeholders) {
        if (cancelled) return;
        const idx = parseInt(el.getAttribute("data-mermaid-idx") || "", 10);
        const source = isNaN(idx) ? undefined : blocks[idx];
        if (!source) continue;
        try {
          const { svg } = await mermaid.render(`mmd-${idBaseRef.current}-${idx}`, source);
          if (cancelled) return;
          // Inject mermaid's SVG verbatim. DOMPurify strips <foreignObject>,
          // which flowcharts use for their node labels (the labels would
          // vanish). mermaid's securityLevel "strict" already encodes HTML and
          // disables scripts/clicks in the diagram source, so the output is
          // safe to inject — the same posture as mermaid.run() used before.
          el.innerHTML = svg;
          el.className = "mermaid-rendered flex justify-center my-4";
        } catch (err) {
          if (cancelled) return;
          // Fall back to the legible source (newlines preserved) rather than
          // collapsed text, so a bad diagram is still readable.
          el.textContent = source;
          el.className = "mermaid-error whitespace-pre-wrap font-mono text-xs text-red-300 block";
          console.warn("mermaid render failed", err);
        }
      }
    });
    return () => {
      cancelled = true;
    };
  }, [html, blocks]);

  return (
    <div
      ref={containerRef}
      className={`prose prose-invert prose-sm max-w-none ${className ?? ""}`}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
});

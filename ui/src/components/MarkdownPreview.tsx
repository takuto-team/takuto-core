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
      m.initialize({ startOnLoad: false, theme: "dark" });
      return m;
    });
  }
  return mermaidReady;
}

interface Props {
  markdown: string;
  className?: string;
}

export const MarkdownPreview = memo(function MarkdownPreview({ markdown, className }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);

  const { html, blocks } = useMemo(() => {
    mermaidBlocks = [];
    const raw = marked.parse(markdown) as string;
    const sanitized = DOMPurify.sanitize(raw, {
      ADD_ATTR: ["data-mermaid-idx"],
    });
    return { html: sanitized, blocks: [...mermaidBlocks] };
  }, [markdown]);

  useEffect(() => {
    if (!containerRef.current || blocks.length === 0) return;

    // Inject mermaid source into placeholder divs
    const placeholders = containerRef.current.querySelectorAll<HTMLElement>(
      "[data-mermaid-idx]"
    );
    const nodes: HTMLElement[] = [];
    placeholders.forEach((el) => {
      const idx = parseInt(el.getAttribute("data-mermaid-idx") || "", 10);
      if (!isNaN(idx) && blocks[idx]) {
        el.textContent = blocks[idx];
        el.className = "mermaid";
        el.removeAttribute("data-mermaid-idx");
        nodes.push(el);
      }
    });

    if (nodes.length === 0) return;

    let cancelled = false;
    getMermaid().then((mermaid) => {
      if (cancelled) return;
      mermaid.run({ nodes }).catch(console.warn);
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

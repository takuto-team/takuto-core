import { useEffect, useRef, useMemo } from "react";
import { marked } from "marked";
import DOMPurify from "dompurify";

// Override the code block renderer once at module load so mermaid fences
// produce <pre class="mermaid"> elements that the useEffect below can pick up.
marked.use({
  renderer: {
    code({ text, lang }: { text: string; lang?: string }): string | false {
      if (lang === "mermaid") {
        const escaped = text
          .replace(/&/g, "&amp;")
          .replace(/</g, "&lt;")
          .replace(/>/g, "&gt;");
        return `<pre class="mermaid">${escaped}</pre>`;
      }
      return false; // fall through to default renderer for all other code blocks
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

export function MarkdownPreview({ markdown, className }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);

  const html = useMemo(() => {
    const raw = marked.parse(markdown) as string;
    return DOMPurify.sanitize(raw);
  }, [markdown]);

  useEffect(() => {
    if (!containerRef.current) return;
    const nodes = Array.from(
      containerRef.current.querySelectorAll<HTMLElement>("pre.mermaid")
    );
    if (nodes.length === 0) return;

    let cancelled = false;
    getMermaid().then((mermaid) => {
      if (cancelled) return;
      mermaid.run({ nodes }).catch(console.warn);
    });
    return () => {
      cancelled = true;
    };
  }, [html]);

  return (
    <div
      ref={containerRef}
      className={`prose prose-invert prose-sm max-w-none ${className ?? ""}`}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

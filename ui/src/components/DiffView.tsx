// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useMemo } from "react";

type DiffOp =
  | { type: "equal"; text: string }
  | { type: "delete"; text: string }
  | { type: "insert"; text: string };

/** Line-level LCS diff between two texts. */
function diffLines(oldText: string, newText: string): DiffOp[] {
  const a = oldText.split("\n");
  const b = newText.split("\n");
  const m = a.length;
  const n = b.length;

  const dp: number[][] = Array.from({ length: m + 1 }, () =>
    new Array(n + 1).fill(0)
  );
  for (let i = 1; i <= m; i++) {
    for (let j = 1; j <= n; j++) {
      dp[i][j] =
        a[i - 1] === b[j - 1]
          ? dp[i - 1][j - 1] + 1
          : Math.max(dp[i - 1][j], dp[i][j - 1]);
    }
  }

  const ops: DiffOp[] = [];
  let i = m;
  let j = n;
  while (i > 0 || j > 0) {
    if (i > 0 && j > 0 && a[i - 1] === b[j - 1]) {
      ops.unshift({ type: "equal", text: a[i - 1] });
      i--;
      j--;
    } else if (j > 0 && (i === 0 || dp[i][j - 1] >= dp[i - 1][j])) {
      ops.unshift({ type: "insert", text: b[j - 1] });
      j--;
    } else {
      ops.unshift({ type: "delete", text: a[i - 1] });
      i--;
    }
  }
  return ops;
}

interface DiffViewProps {
  oldText: string;
  newText: string;
}

export function DiffView({ oldText, newText }: DiffViewProps) {
  const { leftLines, rightLines } = useMemo(() => {
    const ops = diffLines(oldText, newText);
    const leftLines: { text: string; highlighted: boolean }[] = [];
    const rightLines: { text: string; highlighted: boolean }[] = [];

    for (const op of ops) {
      if (op.type === "equal") {
        leftLines.push({ text: op.text, highlighted: false });
        rightLines.push({ text: op.text, highlighted: false });
      } else if (op.type === "delete") {
        leftLines.push({ text: op.text, highlighted: true });
      } else {
        rightLines.push({ text: op.text, highlighted: true });
      }
    }

    return { leftLines, rightLines };
  }, [oldText, newText]);

  const renderPane = (
    lines: { text: string; highlighted: boolean }[],
    side: "left" | "right"
  ) => (
    <div className="flex-1 flex flex-col overflow-hidden border-r border-gray-800 last:border-r-0">
      <div className="px-4 py-2 border-b border-gray-800 flex-shrink-0 flex items-center gap-2">
        <span className="text-[10px] uppercase tracking-wider text-gray-500">
          {side === "left" ? "Before" : "After"}
        </span>
        <span
          className={`text-[10px] px-1.5 py-0.5 rounded ${
            side === "left"
              ? "bg-red-900/40 text-red-300"
              : "bg-green-900/40 text-green-300"
          }`}
        >
          {side === "left" ? "− removed" : "+ added"}
        </span>
      </div>
      <div className="flex-1 overflow-y-auto p-4">
        <pre className="text-xs font-mono leading-relaxed whitespace-pre-wrap break-words m-0">
          {lines.map((line, idx) => (
            <div
              key={idx}
              className={
                line.highlighted
                  ? side === "left"
                    ? "bg-red-900/40 text-red-200 -mx-4 px-4"
                    : "bg-green-900/40 text-green-200 -mx-4 px-4"
                  : "text-gray-300"
              }
            >
              {line.text || "\u00A0"}
            </div>
          ))}
        </pre>
      </div>
    </div>
  );

  return (
    <div className="flex flex-1 min-h-0 overflow-hidden">
      {renderPane(leftLines, "left")}
      {renderPane(rightLines, "right")}
    </div>
  );
}

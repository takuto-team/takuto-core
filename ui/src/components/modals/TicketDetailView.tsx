// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Read-only view of a ticket's markdown description. Renders the loading
 * placeholder while the description is in flight, or the rendered markdown
 * once it lands. Extracted in Phase 5 step 2 (parallel) — wrapper classes
 * preserved verbatim so the modal content area animates identically.
 */

import { MarkdownPreview } from "../MarkdownPreview";

interface Props {
  markdown: string;
  loading: boolean;
}

export function TicketDetailView({ markdown, loading }: Props) {
  if (loading) {
    return (
      <div className="flex-1 overflow-y-auto p-6">
        <p className="text-gray-500 text-sm">Loading description...</p>
      </div>
    );
  }
  return (
    <div className="flex-1 overflow-y-auto p-6">
      <MarkdownPreview markdown={markdown} />
    </div>
  );
}

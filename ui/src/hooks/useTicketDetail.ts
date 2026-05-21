// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `useTicketDetail` — fetches a ticket's markdown description on mount.
 *
 * Extracted from `TicketDetailModal.tsx` so the modal shell stays focused on
 * layout. The hook short-circuits when the caller already passed an
 * `initialDescription`, or when the ticketing system is `"none"` / `"github"`
 * (the latter is fetched by the caller via a different endpoint family).
 */

import { useEffect, useState } from "react";
import { apiJson } from "../api/client";
import type { TicketPreview } from "../api/types";

interface UseTicketDetailResult {
  markdown: string;
  setMarkdown: (next: string) => void;
  loading: boolean;
}

export function useTicketDetail(
  ticketKey: string,
  initialDescription: string | undefined,
  ticketingSystem: string,
): UseTicketDetailResult {
  const [markdown, setMarkdown] = useState(initialDescription || "");
  const [loading, setLoading] = useState(
    !initialDescription && ticketingSystem !== "none",
  );

  useEffect(() => {
    if (initialDescription || ticketingSystem === "none" || ticketingSystem === "github") {
      return;
    }
    apiJson<TicketPreview>(
      `/api/jira/tickets/${encodeURIComponent(ticketKey)}/preview`,
    )
      .then((data) => setMarkdown(data.description_markdown || ""))
      .catch(() => setMarkdown("*Failed to load description*"))
      .finally(() => setLoading(false));
  }, [ticketKey, initialDescription, ticketingSystem]);

  return { markdown, setMarkdown, loading };
}

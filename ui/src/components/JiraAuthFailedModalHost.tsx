// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * App-level host for the Jira auth-failure modal. Subscribes to
 * `onJiraAuthFailure` (emitted by the shared fetch wrapper on a per-user Jira
 * credential-invalid 401) and shows ONE modal regardless of which request
 * tripped it — concurrent failures collapse into a single instance because the
 * open state is a boolean. The CTA routes to Config → Ticketing.
 */

import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { onJiraAuthFailure } from "../api/jiraAuthFailure";
import { JiraAuthFailedModal } from "./modals/JiraAuthFailedModal";

/** Config → Ticketing deep link (matches the `?tab=` slugs Config parses). */
const TICKETING_TAB_PATH = "/config.html?tab=ticketing";

export function JiraAuthFailedModalHost() {
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);

  useEffect(() => onJiraAuthFailure(() => setOpen(true)), []);

  if (!open) return null;

  return (
    <JiraAuthFailedModal
      onUpdateToken={() => {
        setOpen(false);
        navigate(TICKETING_TAB_PATH);
      }}
      onDismiss={() => setOpen(false)}
    />
  );
}

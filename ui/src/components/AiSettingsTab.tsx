// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Consolidated "AI Settings" tab on `/config.html`.
 *
 * Composes two sections:
 *   - `AiProviderSettingsSection` — admin-only. Picking the active provider,
 *     editing its sub-table, and managing `available_providers`.
 *   - `MyCredentialsSection`      — every authenticated user. Their own AI
 *     provider credential (api_key + optional Claude cli_state). The per-user
 *     GitHub PAT lives on its own "GitHub" tab (`GitHubCredentialsSection`).
 *
 * The admin-only gate is implemented HERE, not inside the section, so the
 * section can stay focused on its single concern and is also reusable as a
 * standalone surface (e.g. an admin-only modal in a future iteration).
 *
 * Server-side enforcement at `PUT /api/config/agent` is the real security
 * boundary — this UI gate only controls visibility.
 */

import { useState } from "react";
import { AiProviderSettingsSection } from "./AiProviderSettingsSection";
import { ShareConversationSwitch } from "./admin/ShareConversationSwitch";
import { StepGuardrailsSection } from "./admin/StepGuardrailsSection";
import { MyCredentialsSection } from "./MyCredentialsSection";

interface Props {
  isAdmin: boolean;
}

export function AiSettingsTab({ isAdmin }: Props) {
  // Bumped when the admin saves a new active provider so the per-user
  // credential card refetches and shows the right provider without a reload.
  const [credRefreshKey, setCredRefreshKey] = useState(0);
  return (
    <div className="flex flex-col gap-10">
      {isAdmin && (
        <AiProviderSettingsSection
          onProviderSaved={() => setCredRefreshKey((k) => k + 1)}
        />
      )}
      {isAdmin && <ShareConversationSwitch />}
      {isAdmin && <StepGuardrailsSection />}
      <MyCredentialsSection refreshKey={credRefreshKey} />
    </div>
  );
}

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Consolidated "AI Settings" tab on `/config.html`.
 *
 * Composes two sections:
 *   - `AiProviderSettingsSection` — admin-only. Picking the active provider,
 *     editing its sub-table, and managing `available_providers`.
 *   - `MyCredentialsSection`      — every authenticated user. Their own AI
 *     provider credential (api_key + optional Claude cli_state) and GitHub
 *     PAT.
 *
 * The admin-only gate is implemented HERE, not inside the section, so the
 * section can stay focused on its single concern and is also reusable as a
 * standalone surface (e.g. an admin-only modal in a future iteration).
 *
 * Server-side enforcement at `PUT /api/config/agent` is the real security
 * boundary — this UI gate only controls visibility.
 */

import { AiProviderSettingsSection } from "./AiProviderSettingsSection";
import { MyCredentialsSection } from "./MyCredentialsSection";

interface Props {
  isAdmin: boolean;
}

export function AiSettingsTab({ isAdmin }: Props) {
  return (
    <div className="flex flex-col gap-10">
      {isAdmin && <AiProviderSettingsSection />}
      <MyCredentialsSection />
    </div>
  );
}

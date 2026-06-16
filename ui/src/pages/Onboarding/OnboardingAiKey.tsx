// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Step-2 inline AI API-key entry. Wraps the shared
 * `components/credentials/AiCredentialPanel` so the onboarding wizard captures
 * the caller's own provider key right after they pick the provider — keyed to
 * the provider currently selected in the step-2 form, not the persisted one.
 *
 * Data hook + pure renderer split (CODING_STANDARDS §3): this component owns
 * the `GET /api/users/me/credentials` fetch and the
 * `POST /api/users/me/credentials/{provider}` save; `AiCredentialPanel` stays
 * a pure rendering surface.
 */

import { forwardRef, useCallback, useEffect, useState } from "react";
import {
  fetchUserCredentials,
  setProviderCredential,
  UserCredentialsError,
} from "../../api/client";
import type { AgentProviderId, UserCredentialsStatus } from "../../api/types";
import {
  AiCredentialPanel,
  type AiCredentialPanelHandle,
} from "../../components/credentials/AiCredentialPanel";
import { PROVIDER_LABEL } from "../../components/credentials/helpers";
import { useToast } from "../../hooks/useToast";

interface Props {
  /** Provider selected in the step-2 form; the key entry is scoped to it. */
  provider: AgentProviderId;
}

export const OnboardingAiKey = forwardRef<AiCredentialPanelHandle, Props>(
  function OnboardingAiKey({ provider }: Props, ref) {
  const { showToast } = useToast();
  const [creds, setCreds] = useState<UserCredentialsStatus | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    const c = await fetchUserCredentials().catch(() => null);
    setCreds(c);
  }, []);

  useEffect(() => {
    let mounted = true;
    refresh().finally(() => {
      if (mounted) setLoading(false);
    });
    return () => {
      mounted = false;
    };
  }, [refresh]);

  const handleSave = useCallback(
    async (body: Parameters<typeof setProviderCredential>[1]) => {
      try {
        await setProviderCredential(provider, body);
        await refresh();
        const label = PROVIDER_LABEL[provider] ?? provider;
        const what = body.kind === "cli_state" ? "session uploaded" : "connected";
        showToast(`${label} ${what}.`, "success");
        return true;
      } catch (e: unknown) {
        const msg =
          e instanceof UserCredentialsError
            ? `${e.message} (code: ${e.code})`
            : e instanceof Error
              ? e.message
              : "Could not save your credential.";
        showToast(msg, "error");
        return false;
      }
    },
    [provider, refresh, showToast],
  );

  if (loading) {
    return <p className="text-sm text-gray-500">Loading your credentials…</p>;
  }

  return (
    <AiCredentialPanel
      ref={ref}
      activeProvider={provider}
      credentials={creds}
      onSave={handleSave}
    />
  );
  },
);

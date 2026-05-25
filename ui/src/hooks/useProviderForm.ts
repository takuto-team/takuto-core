// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback, useEffect, useMemo, useState } from "react";
import { AgentConfigError, apiJson, putAgentConfig } from "../api/client";
import type {
  AgentConfig,
  AgentConfigPatch,
  AgentProviderId,
  ConfigResponse,
} from "../api/types";
import { useToast } from "./useToast";

/**
 * Self-contained provider-form state for the Onboarding wizard's step 2:
 * loads `/api/config` once, exposes the editable fields (provider, base
 * URL, extra args), and ships `save()` which PUTs back through
 * `putAgentConfig`. Surfaces a toast on save success / failure and
 * returns `true` / `false` so the parent flow can gate "Continue".
 *
 * Also returns the cached `ticketingSystem` + `githubAppConfigured`
 * read-only displays the other steps need from the same `/api/config`
 * fetch — saves a duplicate round-trip.
 */
export function useProviderForm() {
  const { showToast } = useToast();
  const [config, setConfig] = useState<ConfigResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [provider, setProvider] = useState<AgentProviderId>("claude");
  const [baseUrl, setBaseUrl] = useState("");
  const [extraArgsText, setExtraArgsText] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    apiJson<ConfigResponse>("/api/config")
      .then((c) => {
        setConfig(c);
        const agent = (c.agent ?? {}) as AgentConfig;
        const p: AgentProviderId = (agent.provider ?? "claude") as AgentProviderId;
        setProvider(p);
        const sub = agent.providers?.[p as keyof typeof agent.providers] as
          | { base_url?: string; extra_args?: string[] }
          | undefined;
        setBaseUrl(sub?.base_url ?? "");
        setExtraArgsText((sub?.extra_args ?? []).join("\n"));
      })
      .catch(() => {
        // Server might still be coming up — wizard remains usable but the
        // step-2 form will save against blank defaults.
      })
      .finally(() => setLoading(false));
  }, []);

  const ticketingSystem = useMemo(
    () => config?.ticketing_system ?? "none",
    [config],
  );
  const githubAppConfigured = useMemo(
    () => Boolean(config?.github_app_configured),
    [config],
  );

  const save = useCallback(async (): Promise<boolean> => {
    setSaving(true);
    const extraArgs = extraArgsText
      .split("\n")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    const sub: Record<string, unknown> = {
      base_url: baseUrl,
      extra_args: extraArgs,
    };
    if (provider === "cursor") {
      // Cursor has no base_url field — drop it to satisfy the server's
      // `deny_unknown_fields` (04_architecture.md §2.2 amendment A1).
      delete sub.base_url;
    }
    const patch: AgentConfigPatch = {
      provider,
      providers: { [provider]: sub } as AgentConfigPatch["providers"],
    };
    try {
      await putAgentConfig(patch);
      showToast("Provider configured.", "success");
      return true;
    } catch (e: unknown) {
      const msg =
        e instanceof AgentConfigError
          ? `${e.message} (code: ${e.code})`
          : e instanceof Error
            ? e.message
            : String(e);
      showToast(msg, "error");
      return false;
    } finally {
      setSaving(false);
    }
  }, [provider, baseUrl, extraArgsText, showToast]);

  return {
    loading,
    saving,
    provider,
    setProvider,
    baseUrl,
    setBaseUrl,
    extraArgsText,
    setExtraArgsText,
    ticketingSystem,
    githubAppConfigured,
    save,
  };
}

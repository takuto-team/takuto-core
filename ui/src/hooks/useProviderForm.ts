// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
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
  const { t } = useTranslation("config");
  const { showToast } = useToast();
  const [config, setConfig] = useState<ConfigResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [provider, setProvider] = useState<AgentProviderId>("claude");
  const [baseUrl, setBaseUrl] = useState("");
  // Self-hosted spec (lore/audits/2026-05-27-opencode-self-hosted-spec.md
  // §2.4): OpenCode validator requires both base_url AND model. We carry
  // `model` in the onboarding form so admins can complete step 2 when
  // they pick OpenCode (otherwise PUT /api/config/agent returns 400
  // `opencode_model_required`).
  const [model, setModel] = useState("");
  const [extraArgsText, setExtraArgsText] = useState("");
  const [saving, setSaving] = useState(false);
  // Seeded values from /api/config, used to compute `isDirty` (the wizard
  // gates "Save and Continue" on it). Updated on load and after a save.
  const [seed, setSeed] = useState({ provider: "claude", baseUrl: "", model: "", extraArgsText: "" });

  useEffect(() => {
    apiJson<ConfigResponse>("/api/config")
      .then((c) => {
        setConfig(c);
        const agent = (c.agent ?? {}) as AgentConfig;
        const p: AgentProviderId = (agent.provider ?? "claude") as AgentProviderId;
        setProvider(p);
        const sub = agent.providers?.[p as keyof typeof agent.providers] as
          | { base_url?: string; model?: string; extra_args?: string[] }
          | undefined;
        const seededBaseUrl = sub?.base_url ?? "";
        const seededModel = sub?.model ?? "";
        const seededExtra = (sub?.extra_args ?? []).join("\n");
        setBaseUrl(seededBaseUrl);
        setModel(seededModel);
        setExtraArgsText(seededExtra);
        setSeed({ provider: p, baseUrl: seededBaseUrl, model: seededModel, extraArgsText: seededExtra });
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
  // Git step (3) seeds from the same `/api/config` fetch — no duplicate
  // round-trip. Fall back to the documented defaults when absent.
  const gitBaseBranch = useMemo(() => config?.git?.base_branch ?? "", [config]);
  const gitRemote = useMemo(() => config?.git?.remote ?? "", [config]);
  // Workflows step (4) step-timeout seed, same shared fetch.
  const stepTimeoutSecs = useMemo(() => {
    const v = config?.agent?.step_timeout_secs;
    return typeof v === "number" ? v : undefined;
  }, [config]);

  const isDirty = useMemo(
    () =>
      provider !== seed.provider ||
      baseUrl !== seed.baseUrl ||
      model !== seed.model ||
      extraArgsText !== seed.extraArgsText,
    [provider, baseUrl, model, extraArgsText, seed],
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
    // Self-hosted spec (2026-05-27 §2.4): OpenCode requires model. We also
    // send `model` for the other providers when set, so users can override
    // the vendor default from the onboarding wizard.
    if (model.trim().length > 0) {
      sub.model = model;
    }
    const patch: AgentConfigPatch = {
      provider,
      providers: { [provider]: sub } as AgentConfigPatch["providers"],
    };
    try {
      await putAgentConfig(patch);
      setSeed({ provider, baseUrl, model, extraArgsText });
      showToast(t("ai.providerConfigured"), "success");
      return true;
    } catch (e: unknown) {
      const msg =
        e instanceof AgentConfigError
          ? t("errors.withCode", { message: e.message, code: e.code })
          : e instanceof Error
            ? e.message
            : String(e);
      showToast(msg, "error");
      return false;
    } finally {
      setSaving(false);
    }
  }, [provider, baseUrl, model, extraArgsText, showToast, t]);

  return {
    loading,
    saving,
    provider,
    setProvider,
    baseUrl,
    setBaseUrl,
    model,
    setModel,
    extraArgsText,
    setExtraArgsText,
    ticketingSystem,
    githubAppConfigured,
    gitBaseBranch,
    gitRemote,
    stepTimeoutSecs,
    isDirty,
    save,
  };
}

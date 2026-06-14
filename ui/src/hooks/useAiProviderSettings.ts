// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * All logic for the admin AI provider settings section: reads `/api/config`
 * through the shared Query cache, owns the controlled form state
 * (selected provider, per-provider draft, the `available_providers`
 * whitelist), and drives the save / provider-switch flow via
 * `PUT /api/config/agent`. The section component is a pure renderer of what
 * this hook returns (CODING_STANDARDS §3 — no component both fetches and
 * renders, no side-effect logic in the component body).
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson, putAgentConfig, AgentConfigError } from "../api/client";
import { queryKeys } from "../api/queryClient";
import { useToast } from "./useToast";
import {
  EMPTY_DRAFT,
  V1_PROVIDERS,
  type ProviderDraft,
} from "../components/admin/ProviderForm";
import type {
  AgentConfig,
  AgentConfigPatch,
  AgentProviderId,
  ConfigResponse,
} from "../api/types";

function draftFromConfig(provider: AgentProviderId, cfg: AgentConfig | undefined): ProviderDraft {
  // Discriminated-union narrowing via the provider key on `cfg.providers`. The
  // table indexes each provider with its own typed sub-table, so accessing
  // `providers.claude` gives the exact shape — no casts.
  const providers = cfg?.providers;
  if (!providers) return { ...EMPTY_DRAFT };
  switch (provider) {
    case "claude": {
      const sub = providers.claude;
      if (!sub) return { ...EMPTY_DRAFT };
      return {
        ...EMPTY_DRAFT,
        model: sub.model ?? "",
        extra_args_text: (sub.extra_args ?? []).join("\n"),
        allow_shared_default: sub.allow_shared_default ?? false,
        base_url: sub.base_url ?? "",
      };
    }
    case "cursor": {
      const sub = providers.cursor;
      if (!sub) return { ...EMPTY_DRAFT };
      return {
        ...EMPTY_DRAFT,
        model: sub.model ?? "",
        extra_args_text: (sub.extra_args ?? []).join("\n"),
        allow_shared_default: sub.allow_shared_default ?? false,
        cli: sub.cli ?? "agent",
      };
    }
    case "codex": {
      const sub = providers.codex;
      if (!sub) return { ...EMPTY_DRAFT };
      return {
        ...EMPTY_DRAFT,
        model: sub.model ?? "",
        extra_args_text: (sub.extra_args ?? []).join("\n"),
        allow_shared_default: sub.allow_shared_default ?? false,
        base_url: sub.base_url ?? "",
        provider_name: sub.provider_name ?? "",
      };
    }
    case "opencode": {
      const sub = providers.opencode;
      if (!sub) return { ...EMPTY_DRAFT };
      return {
        ...EMPTY_DRAFT,
        model: sub.model ?? "",
        extra_args_text: (sub.extra_args ?? []).join("\n"),
        allow_shared_default: sub.allow_shared_default ?? false,
        base_url: sub.base_url ?? "",
        context_limit: sub.context_limit != null ? String(sub.context_limit) : "",
        output_limit: sub.output_limit != null ? String(sub.output_limit) : "",
      };
    }
    case "gemini": {
      const sub = providers.gemini;
      if (!sub) return { ...EMPTY_DRAFT };
      return {
        ...EMPTY_DRAFT,
        model: sub.model ?? "",
        extra_args_text: (sub.extra_args ?? []).join("\n"),
        allow_shared_default: sub.allow_shared_default ?? false,
        base_url: sub.base_url ?? "",
      };
    }
    case "none":
      return { ...EMPTY_DRAFT };
  }
}

/**
 * Parse a token-limit input. Empty / non-positive / non-numeric → `null`
 * (clears the limit server-side); a positive integer → that number. Sending
 * an explicit value every save lets the double-option endpoint distinguish
 * "clear" (null) from "set" (number).
 */
function parseLimit(raw: string): number | null {
  const t = raw.trim();
  if (t === "") return null;
  const n = Number.parseInt(t, 10);
  return Number.isFinite(n) && n > 0 ? n : null;
}

/** Build the API patch from the user's draft for a single provider. */
function patchFromDraft(
  provider: AgentProviderId,
  draft: ProviderDraft,
): AgentConfigPatch["providers"] {
  const extraArgs = draft.extra_args_text
    .split("\n")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  const common = {
    model: draft.model,
    extra_args: extraArgs,
    allow_shared_default: draft.allow_shared_default,
  };
  switch (provider) {
    case "claude":
      return { claude: { ...common, base_url: draft.base_url } };
    case "cursor":
      return { cursor: { ...common, cli: draft.cli } };
    case "codex":
      return { codex: { ...common, base_url: draft.base_url, provider_name: draft.provider_name } };
    case "opencode":
      return {
        opencode: {
          ...common,
          base_url: draft.base_url,
          context_limit: parseLimit(draft.context_limit),
          output_limit: parseLimit(draft.output_limit),
        },
      };
    default:
      return undefined;
  }
}

export interface ProviderSwitch {
  from: AgentProviderId;
  to: AgentProviderId;
}

export interface UseAiProviderSettingsResult {
  loading: boolean;
  error: string;
  saving: boolean;
  selectedProvider: AgentProviderId;
  draft: ProviderDraft;
  availableProviders: AgentProviderId[];
  pendingProviderSwitch: ProviderSwitch | null;
  selectProvider: (next: AgentProviderId) => void;
  setDraft: (draft: ProviderDraft) => void;
  toggleAvailable: (p: AgentProviderId) => void;
  requestSave: () => void;
  confirmSwitch: () => void;
  cancelSwitch: () => void;
}

export function useAiProviderSettings(
  opts: { onProviderSaved?: () => void } = {},
): UseAiProviderSettingsResult {
  const { onProviderSaved } = opts;
  const { showToast } = useToast();
  const queryClient = useQueryClient();

  const query = useQuery({
    queryKey: queryKeys.config,
    queryFn: () => apiJson<ConfigResponse>("/api/config"),
  });
  const config = query.data ?? null;

  const [selectedProvider, setSelectedProvider] = useState<AgentProviderId>("claude");
  const [draft, setDraft] = useState<ProviderDraft>(EMPTY_DRAFT);
  const [availableProviders, setAvailableProviders] = useState<AgentProviderId[]>([]);
  const [pendingProviderSwitch, setPendingProviderSwitch] = useState<ProviderSwitch | null>(null);

  // Seed the controlled form from the loaded config exactly once — later
  // config updates (e.g. after a save) must not clobber an in-progress edit.
  const initializedRef = useRef(false);
  useEffect(() => {
    if (initializedRef.current || !config) return;
    initializedRef.current = true;
    const agent = (config.agent ?? {}) as AgentConfig;
    const provider: AgentProviderId = (agent.provider ?? "claude") as AgentProviderId;
    setSelectedProvider(provider);
    setDraft(draftFromConfig(provider, agent));
    setAvailableProviders(
      Array.isArray(agent.available_providers) && agent.available_providers.length > 0
        ? (agent.available_providers as AgentProviderId[])
        : V1_PROVIDERS,
    );
  }, [config]);

  const savedProvider = useMemo<AgentProviderId>(
    () => (config?.agent?.provider as AgentProviderId) ?? "claude",
    [config],
  );

  const mutation = useMutation({
    mutationFn: (patch: AgentConfigPatch) => putAgentConfig(patch),
    onSuccess: (updated) => {
      queryClient.setQueryData<ConfigResponse>(queryKeys.config, updated);
    },
  });

  const persist = useCallback(
    async (patch: AgentConfigPatch) => {
      try {
        const updated = await mutation.mutateAsync(patch);
        onProviderSaved?.();
        // Backend returns `persisted: false` + `persist_warning` when the
        // in-memory patch succeeded but the on-disk write failed. Strict
        // `=== false` so a legacy server (undefined) is treated as "assume OK".
        if (updated.persisted === false) {
          const reason = updated.persist_warning ?? "unknown error";
          showToast(
            `AI provider settings applied in memory but NOT persisted to disk: ${reason}. The change will be lost on next restart — fix the config volume and save again.`,
            "error",
          );
        } else {
          showToast("AI provider settings saved.", "success");
        }
      } catch (e: unknown) {
        if (e instanceof AgentConfigError) {
          showToast(`${e.message} (code: ${e.code})`, "error");
        } else {
          showToast(e instanceof Error ? e.message : String(e), "error");
        }
      }
    },
    [mutation, showToast, onProviderSaved],
  );

  const selectProvider = useCallback(
    (next: AgentProviderId) => {
      setSelectedProvider(next);
      setDraft(draftFromConfig(next, (config?.agent ?? {}) as AgentConfig));
    },
    [config],
  );

  const toggleAvailable = useCallback((p: AgentProviderId) => {
    setAvailableProviders((prev) => (prev.includes(p) ? prev.filter((x) => x !== p) : [...prev, p]));
  }, []);

  const buildPatch = useCallback(
    (): AgentConfigPatch => ({
      provider: selectedProvider,
      available_providers: availableProviders,
      providers: patchFromDraft(selectedProvider, draft),
    }),
    [selectedProvider, availableProviders, draft],
  );

  const requestSave = useCallback(() => {
    if (selectedProvider !== savedProvider) {
      setPendingProviderSwitch({ from: savedProvider, to: selectedProvider });
      return;
    }
    void persist(buildPatch());
  }, [selectedProvider, savedProvider, persist, buildPatch]);

  const confirmSwitch = useCallback(() => {
    setPendingProviderSwitch(null);
    void persist(buildPatch());
  }, [persist, buildPatch]);

  const cancelSwitch = useCallback(() => setPendingProviderSwitch(null), []);

  return {
    loading: query.isPending,
    error: query.isError ? (query.error instanceof Error ? query.error.message : String(query.error)) : "",
    saving: mutation.isPending,
    selectedProvider,
    draft,
    availableProviders,
    pendingProviderSwitch,
    selectProvider,
    setDraft,
    toggleAvailable,
    requestSave,
    confirmSwitch,
    cancelSwitch,
  };
}

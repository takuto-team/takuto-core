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
import { useTranslation } from "react-i18next";
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
        privacy_mode: sub.privacy_mode ?? true,
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
      return { cursor: { ...common, cli: draft.cli, privacy_mode: draft.privacy_mode } };
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
  /** True when the draft / selected provider / available list differ from the
   *  loaded config — i.e. there are unsaved edits. */
  isDirty: boolean;
  selectedProvider: AgentProviderId;
  draft: ProviderDraft;
  availableProviders: AgentProviderId[];
  pendingProviderSwitch: ProviderSwitch | null;
  selectProvider: (next: AgentProviderId) => void;
  setDraft: (draft: ProviderDraft) => void;
  toggleAvailable: (p: AgentProviderId) => void;
  requestSave: () => void;
  /** Awaitable save used by the consolidated tab Save button. Resolves `true`
   *  on a successful persist, `false` on error or when the user cancels the
   *  provider-switch confirm. */
  saveAsync: () => Promise<boolean>;
  confirmSwitch: () => void;
  cancelSwitch: () => void;
}

/** Order-insensitive equality for the available-providers list. */
function sameProviderSet(a: AgentProviderId[], b: AgentProviderId[]): boolean {
  if (a.length !== b.length) return false;
  const sb = new Set(b);
  return a.every((x) => sb.has(x));
}

export function useAiProviderSettings(
  opts: { onProviderSaved?: () => void } = {},
): UseAiProviderSettingsResult {
  const { onProviderSaved } = opts;
  const { t } = useTranslation("config");
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
  // `initialized` gates dirty detection so the empty initial form state isn't
  // mistaken for unsaved edits before the seed runs.
  const initializedRef = useRef(false);
  const [initialized, setInitialized] = useState(false);
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
    setInitialized(true);
  }, [config]);

  const savedProvider = useMemo<AgentProviderId>(
    () => (config?.agent?.provider as AgentProviderId) ?? "claude",
    [config],
  );

  // Unsaved-edits detection: the selected provider, its draft sub-table, or the
  // available-providers whitelist differs from the loaded config. Computed
  // against the persisted config so a save (which updates the query cache)
  // flips this back to false.
  const isDirty = useMemo<boolean>(() => {
    if (!config || !initialized) return false;
    const agent = (config.agent ?? {}) as AgentConfig;
    if (selectedProvider !== savedProvider) return true;
    const savedAvailable =
      Array.isArray(agent.available_providers) && agent.available_providers.length > 0
        ? (agent.available_providers as AgentProviderId[])
        : V1_PROVIDERS;
    if (!sameProviderSet(availableProviders, savedAvailable)) return true;
    const savedDraft = draftFromConfig(selectedProvider, agent);
    return JSON.stringify(draft) !== JSON.stringify(savedDraft);
  }, [config, initialized, selectedProvider, savedProvider, availableProviders, draft]);

  // Resolver for the awaitable save path when a provider switch needs the
  // confirm modal: `saveAsync` returns a promise that settles when the user
  // confirms (→ persist result) or cancels (→ false).
  const switchResolverRef = useRef<((ok: boolean) => void) | null>(null);

  const mutation = useMutation({
    mutationFn: (patch: AgentConfigPatch) => putAgentConfig(patch),
    onSuccess: (updated) => {
      queryClient.setQueryData<ConfigResponse>(queryKeys.config, updated);
    },
  });

  const persist = useCallback(
    async (patch: AgentConfigPatch): Promise<boolean> => {
      try {
        const updated = await mutation.mutateAsync(patch);
        onProviderSaved?.();
        // Backend returns `persisted: false` + `persist_warning` when the
        // in-memory patch succeeded but the on-disk write failed. Strict
        // `=== false` so a legacy server (undefined) is treated as "assume OK".
        if (updated.persisted === false) {
          const reason = updated.persist_warning ?? t("ai.unknownError");
          showToast(t("ai.persistWarning", { reason }), "error");
        } else {
          showToast(t("ai.savedToast"), "success");
        }
        return true;
      } catch (e: unknown) {
        if (e instanceof AgentConfigError) {
          showToast(t("errors.withCode", { message: e.message, code: e.code }), "error");
        } else {
          showToast(e instanceof Error ? e.message : String(e), "error");
        }
        return false;
      }
    },
    [mutation, showToast, onProviderSaved, t],
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

  // Awaitable save for the consolidated tab Save button. When the provider
  // changed, opens the switch-confirm and resolves once the user confirms
  // (persist result) or cancels (false); otherwise persists directly.
  const saveAsync = useCallback((): Promise<boolean> => {
    if (!isDirty) return Promise.resolve(true);
    // OpenCode requires a base_url + model (validator returns 400 otherwise).
    // Guard here so the consolidated Save surfaces the requirement without a
    // server bounce; ProviderForm shows the inline message.
    if (
      selectedProvider === "opencode" &&
      (draft.base_url.trim() === "" || draft.model.trim() === "")
    ) {
      showToast(t("ai.form.opencodeRequires"), "error");
      return Promise.resolve(false);
    }
    if (selectedProvider !== savedProvider) {
      setPendingProviderSwitch({ from: savedProvider, to: selectedProvider });
      return new Promise<boolean>((resolve) => {
        switchResolverRef.current = resolve;
      });
    }
    return persist(buildPatch());
  }, [isDirty, selectedProvider, savedProvider, draft, persist, buildPatch, showToast, t]);

  const confirmSwitch = useCallback(() => {
    setPendingProviderSwitch(null);
    const resolve = switchResolverRef.current;
    switchResolverRef.current = null;
    void persist(buildPatch()).then((ok) => resolve?.(ok));
  }, [persist, buildPatch]);

  const cancelSwitch = useCallback(() => {
    setPendingProviderSwitch(null);
    const resolve = switchResolverRef.current;
    switchResolverRef.current = null;
    resolve?.(false);
  }, []);

  return {
    loading: query.isPending,
    error: query.isError ? (query.error instanceof Error ? query.error.message : String(query.error)) : "",
    saving: mutation.isPending,
    isDirty,
    selectedProvider,
    draft,
    availableProviders,
    pendingProviderSwitch,
    selectProvider,
    setDraft,
    toggleAvailable,
    requestSave,
    saveAsync,
    confirmSwitch,
    cancelSwitch,
  };
}

// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * AI provider card — paste-an-API-key surface for every v1 provider plus
 * the Claude-only `cli_state` (session) upload. Extracted from
 * `MyCredentialsSection.tsx` (CODING_STANDARDS §3 "extract a sub-component
 * when a component exceeds ~150 lines").
 */

import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { ConnectedStatusPill } from "../ConnectedStatusPill";
import { CredentialPasteField } from "../CredentialPasteField";
import { parseClaudeSessionBlob } from "../../utils/claudeSession";
import type {
  ProviderCredentialKind,
  SetProviderCredentialRequest,
  UserCredentialsStatus,
} from "../../api/types";
import { PROVIDER_LABEL, providerHelper } from "./helpers";
import { ClaudeAuthTabButton, ClaudeSessionField } from "./ClaudeSessionField";

interface AiCredentialPanelProps {
  activeProvider: string;
  credentials: UserCredentialsStatus | null;
  /** Persist the entered credential. Returns `true` on success, `false` on
   *  failure (the caller renders the error toast). */
  onSave: (body: SetProviderCredentialRequest) => Promise<boolean>;
  /**
   * Hard-delete the stored credential for the current provider + the given
   * slot. Returns `true` on success. When omitted, no Delete button renders
   * (e.g. the onboarding wizard, where deletion is meaningless). Always
   * scoped to the panel's `activeProvider` — never another provider.
   */
  onDelete?: (kind: ProviderCredentialKind) => Promise<boolean>;
  /**
   * Reports `true` when the api-key / session field holds a typed-but-unsaved
   * value, so the parent can warn before the user navigates away and loses it.
   * Informational only — the credential still saves via its own Save button.
   */
  onDirtyChange?: (dirty: boolean) => void;
  /** When true, hide the panel's own Save buttons — the credential is persisted
   *  by a single page-level Save that calls `saveIfDirty`. Defaults to false. */
  deferSave?: boolean;
}

/**
 * Imperative handle the onboarding wizard drives on "Continue" so the
 * currently-typed credential is persisted as part of advancing the step,
 * without the user having to click the panel's own Save button.
 */
export interface AiCredentialPanelHandle {
  /**
   * Submit the entered credential if the active field is non-blank. Reuses
   * the same submit logic (incl. Claude api_key/cli_state tab + session
   * validation) as the panel's own Save button. A blank field is a no-op
   * that resolves `true` (the user is skipping / using a deployment default).
   * Resolves `false` only when a non-blank submit fails validation or the save.
   */
  saveIfDirty: () => Promise<boolean>;
}

type ClaudeAuthMethod = "api_key" | "cli_state";

export const AiCredentialPanel = forwardRef<
  AiCredentialPanelHandle,
  AiCredentialPanelProps
>(function AiCredentialPanel(
  { activeProvider, credentials, onSave, onDelete, onDirtyChange, deferSave = false }: AiCredentialPanelProps,
  ref,
) {
  const { t } = useTranslation("credentials");
  const [apiKey, setApiKey] = useState("");
  const [sessionJson, setSessionJson] = useState("");
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  // Inline pre-flight validation error for the session blob. Set by the
  // Save path so the user sees structured feedback BEFORE the server
  // round-trip (#40 T-CLAUDE-UI-006). Cleared on each edit.
  const [sessionError, setSessionError] = useState<string | null>(null);
  const [claudeTab, setClaudeTab] = useState<ClaudeAuthMethod>("api_key");

  // Report a typed-but-unsaved value so the tab can warn before navigation.
  useEffect(() => {
    onDirtyChange?.(apiKey.trim() !== "" || sessionJson.trim() !== "");
  }, [apiKey, sessionJson, onDirtyChange]);

  // Reset all input state whenever the active provider changes. Without this,
  // a value typed for provider A survives a switch to provider B and would be
  // persisted under B on the next Save / wizard "Continue" — the exact bug
  // that created an orphaned credential row for a provider the user never
  // entered a key for. The save is always scoped to `activeProvider`, so the
  // field MUST be empty after a switch.
  useEffect(() => {
    setApiKey("");
    setSessionJson("");
    setSessionError(null);
    setClaudeTab("api_key");
  }, [activeProvider]);

  const isClaude = activeProvider === "claude";

  // Wire-format note: the GET response now carries a bundle (api_key +
  // cli_state slots) per task #39. See
  // `routes/credentials.rs::ProviderCredentialBundle`.
  const bundle = credentials?.provider ?? null;
  const bundleMatches = !!bundle && bundle.provider === activeProvider;
  const apiKeyActive = bundleMatches && !!bundle?.api_key?.active;
  const cliStateActive = bundleMatches && !!bundle?.cli_state?.active;
  // The card is "connected" if EITHER slot has an active row.
  const hasMatchingCredential = apiKeyActive || cliStateActive;

  const label = PROVIDER_LABEL[activeProvider] ?? activeProvider;
  const apiKeyHelper = useMemo(
    () => providerHelper(activeProvider, "api_key"),
    [activeProvider],
  );
  const sessionHelper = useMemo(
    () => providerHelper(activeProvider, "cli_state"),
    [activeProvider],
  );

  /**
   * Pick the most informative status pill label.
   *   - Both kinds connected   → "API key + Session"
   *   - Only api_key connected → "API key"
   *   - Only cli_state         → "Session"
   *   - Neither                → undefined (pill shows base copy)
   */
  const pillLabel = useMemo(() => {
    if (apiKeyActive && cliStateActive) return t("my.ai.pill.both");
    if (apiKeyActive) return t("my.ai.pill.apiKey");
    if (cliStateActive) return t("my.ai.pill.session");
    return undefined;
  }, [apiKeyActive, cliStateActive, t]);

  const submitApiKey = useCallback(async (): Promise<boolean> => {
    setSaving(true);
    try {
      const ok = await onSave({ api_key: apiKey });
      if (ok) setApiKey("");
      return ok;
    } finally {
      setSaving(false);
    }
  }, [apiKey, onSave]);

  const deleteKind = useCallback(
    (kind: ProviderCredentialKind): void => {
      if (!onDelete) return;
      setDeleting(true);
      void onDelete(kind).finally(() => setDeleting(false));
    },
    [onDelete],
  );

  const submitSession = useCallback(async (): Promise<boolean> => {
    // #40 T-CLAUDE-UI-006: client-side validation BEFORE the POST. Surface
    // structured errors inline so the user can correct without a round-trip.
    const result = parseClaudeSessionBlob(sessionJson);
    if (!result.ok) {
      setSessionError(result.message);
      return false;
    }
    setSessionError(null);
    setSaving(true);
    try {
      const ok = await onSave({
        kind: "cli_state",
        claude_session_json: sessionJson,
      });
      if (ok) setSessionJson("");
      return ok;
    } finally {
      setSaving(false);
    }
  }, [sessionJson, onSave]);

  useImperativeHandle(
    ref,
    () => ({
      saveIfDirty: async () => {
        if (isClaude && claudeTab === "cli_state") {
          if (sessionJson.trim() === "") return true;
          return submitSession();
        }
        if (apiKey.trim() === "") return true;
        return submitApiKey();
      },
    }),
    [isClaude, claudeTab, apiKey, sessionJson, submitApiKey, submitSession],
  );

  return (
    <section
      aria-labelledby="ai-card-title"
      className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-4"
    >
      <div className="flex items-center justify-between gap-3 flex-wrap">
        <h3 id="ai-card-title" className="text-lg font-semibold text-white">
          {t("my.ai.cardTitle", { provider: label })}
        </h3>
        <ConnectedStatusPill
          state={hasMatchingCredential ? "token" : "missing"}
          label={pillLabel}
        />
      </div>

      {isClaude ? (
        <>
          {/* #40: Claude is the only provider that accepts cli_state today.
              Render an "Auth method" segmented control so users on Claude
              Code Pro/Team can upload their ~/.claude.json blob in addition
              to (or instead of) a bearer API key. */}
          <div
            role="tablist"
            aria-label={t("my.ai.authMethod")}
            className="flex gap-1 p-1 bg-gray-950/60 border border-gray-800 rounded-lg w-fit"
          >
            <ClaudeAuthTabButton
              isActive={claudeTab === "api_key"}
              connected={apiKeyActive}
              onClick={() => setClaudeTab("api_key")}
              label={t("my.ai.tabApiKey")}
            />
            <ClaudeAuthTabButton
              isActive={claudeTab === "cli_state"}
              connected={cliStateActive}
              onClick={() => setClaudeTab("cli_state")}
              label={t("my.ai.tabSession")}
            />
          </div>

          {claudeTab === "api_key" ? (
            <CredentialPasteField
              label={t("my.ai.apiKeyLabel", { provider: label })}
              value={apiKey}
              onChange={setApiKey}
              onSubmit={submitApiKey}
              hideSave={deferSave}
              saving={saving}
              placeholder={t("my.ai.claudeApiKeyPlaceholder")}
              helper={apiKeyHelper}
              saveLabel={apiKeyActive ? t("actions.replace") : t("actions.save")}
              canDelete={apiKeyActive}
              deleting={deleting}
              onDelete={onDelete ? () => deleteKind("api_key") : undefined}
            />
          ) : (
            <ClaudeSessionField
              value={sessionJson}
              onChange={(v) => {
                setSessionJson(v);
                if (sessionError) setSessionError(null);
              }}
              onSubmit={submitSession}
              hideSave={deferSave}
              saving={saving}
              error={sessionError}
              connected={cliStateActive}
              helper={sessionHelper}
            />
          )}
        </>
      ) : (
        // Issues A + B from #31: no Rotate / Disconnect buttons.
        // The single Replace/Save button covers rotation; revocation lives
        // on the provider side (anthropic.com / cursor.com / github.com).
        // To wipe the local row, the user pastes a different key.
        //
        // OpenCode self-hosted spec (2026-05-27 §2.5): the "API key" wording
        // is misleading for OpenCode (it's an optional bearer for a
        // self-hosted endpoint). Re-label per provider.
        <CredentialPasteField
          label={
            activeProvider === "opencode"
              ? t("my.ai.bearerLabel")
              : t("my.ai.apiKeyLabel", { provider: label })
          }
          value={apiKey}
          onChange={setApiKey}
          onSubmit={submitApiKey}
          hideSave={deferSave}
          saving={saving}
          placeholder={
            activeProvider === "opencode"
              ? t("my.ai.opencodePlaceholder")
              : t("my.ai.apiKeyPlaceholder", { provider: label })
          }
          helper={apiKeyHelper}
          saveLabel={apiKeyActive ? t("actions.replace") : t("actions.save")}
          canDelete={apiKeyActive}
          deleting={deleting}
          onDelete={onDelete ? () => deleteKind("api_key") : undefined}
        />
      )}
    </section>
  );
});

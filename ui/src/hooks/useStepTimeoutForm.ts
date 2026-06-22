// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { AgentConfigError, putAgentConfig } from "../api/agentConfig";
import { useToast } from "./useToast";

interface Config {
  /** Current saved `agent.step_timeout_secs` from `/api/config`, used to seed
   *  the field once the parent's config fetch resolves. */
  initialSecs: number | undefined;
  /** Flips to `true` once the parent finished loading `/api/config`. */
  ready: boolean;
}

const DEFAULT_STEP_TIMEOUT = "1800";

/**
 * Onboarding step-4 "step timeout" state: a single positive-integer seconds
 * value, seeded from `/api/config` and saved via `PUT /api/config/agent`
 * (`step_timeout_secs`).
 *
 * `save()` blocks and returns `false` when the value is blank or non-positive —
 * the step renders the inline validation message off `invalid` — so the wizard
 * flow can gate "Finish setup".
 */
export function useStepTimeoutForm({ initialSecs, ready }: Config) {
  const { t } = useTranslation("config");
  const { showToast } = useToast();
  const [value, setValue] = useState(DEFAULT_STEP_TIMEOUT);
  const [seedValue, setSeedValue] = useState(DEFAULT_STEP_TIMEOUT);
  const [seeded, setSeeded] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (ready && !seeded) {
      const v =
        typeof initialSecs === "number" && initialSecs > 0
          ? String(initialSecs)
          : DEFAULT_STEP_TIMEOUT;
      setValue(v);
      setSeedValue(v);
      setSeeded(true);
    }
  }, [ready, seeded, initialSecs]);

  const parsed = Number.parseInt(value.trim(), 10);
  const invalid = !(Number.isFinite(parsed) && parsed > 0);
  const isDirty = value !== seedValue;

  const save = useCallback(async (): Promise<boolean> => {
    if (invalid) {
      // Inline validation message is already visible off `invalid`; block
      // forward navigation.
      return false;
    }
    setSaving(true);
    try {
      await putAgentConfig({ step_timeout_secs: parsed });
      setSeedValue(value);
      showToast(t("ai.guardrails.savedToast"), "success");
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
  }, [parsed, invalid, value, showToast, t]);

  return { value, setValue, invalid, saving, isDirty, save };
}

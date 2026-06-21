// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { GitConfigError, putGitConfig } from "../api/gitConfig";
import { useToast } from "./useToast";

interface Config {
  /** Current saved `git.base_branch` from `/api/config`, used to seed the
   *  field once the parent's config fetch resolves. */
  initialBaseBranch: string;
  /** Current saved `git.remote` from `/api/config`. */
  initialRemote: string;
  /** Flips to `true` once the parent finished loading `/api/config`, so the
   *  fields seed themselves from the persisted values exactly once. */
  ready: boolean;
  /** Whether the caller may persist git settings. `PUT /api/config/git` is
   *  admin-only (403 otherwise), so when `false` `save()` is a no-op that lets
   *  the wizard advance without an API call — same as the admin-gated polling
   *  section. Defaults to `true`. */
  canSave?: boolean;
}

const DEFAULT_BASE_BRANCH = "main";
const DEFAULT_REMOTE = "origin";

/**
 * Onboarding step-3 git settings state: base branch + remote, seeded from
 * `/api/config` and saved via `PUT /api/config/git`.
 *
 * Both fields are required (the server stores non-empty strings). `save()`
 * blocks and returns `false` when either is blank — the step renders the inline
 * validation message off `baseBranchInvalid` / `remoteInvalid` — so the wizard
 * flow can gate "Continue". The admin-gated 403 surfaces as a friendly toast.
 */
export function useGitForm({
  initialBaseBranch,
  initialRemote,
  ready,
  canSave = true,
}: Config) {
  const { t } = useTranslation("config");
  const { showToast } = useToast();
  const [baseBranch, setBaseBranch] = useState(DEFAULT_BASE_BRANCH);
  const [remote, setRemote] = useState(DEFAULT_REMOTE);
  const [seeded, setSeeded] = useState(false);
  const [saving, setSaving] = useState(false);

  // Seed once the config has loaded. Guarded by `seeded` so a later re-render
  // of the parent doesn't clobber an edit the user has since made.
  useEffect(() => {
    if (ready && !seeded) {
      setBaseBranch(initialBaseBranch.trim() || DEFAULT_BASE_BRANCH);
      setRemote(initialRemote.trim() || DEFAULT_REMOTE);
      setSeeded(true);
    }
  }, [ready, seeded, initialBaseBranch, initialRemote]);

  const baseBranchInvalid = baseBranch.trim() === "";
  const remoteInvalid = remote.trim() === "";

  const save = useCallback(async (): Promise<boolean> => {
    // Git settings are deployment-level and admin-gated server-side. A
    // non-admin reaching this step advances without an API call rather than
    // tripping a 403 (the inputs are read-only for them).
    if (!canSave) {
      return true;
    }
    if (baseBranchInvalid || remoteInvalid) {
      // Inline validation message is already visible off the *Invalid flags;
      // just block forward navigation.
      return false;
    }
    setSaving(true);
    try {
      await putGitConfig({ base_branch: baseBranch.trim(), remote: remote.trim() });
      showToast(t("git.saved"), "success");
      return true;
    } catch (e: unknown) {
      let msg: string;
      if (e instanceof GitConfigError) {
        msg =
          e.status === 403
            ? t("git.adminOnly")
            : t("errors.withCode", { message: e.message, code: e.code });
      } else if (e instanceof Error) {
        msg = e.message;
      } else {
        msg = String(e);
      }
      showToast(msg, "error");
      return false;
    } finally {
      setSaving(false);
    }
  }, [canSave, baseBranch, remote, baseBranchInvalid, remoteInvalid, showToast, t]);

  return {
    baseBranch,
    setBaseBranch,
    remote,
    setRemote,
    baseBranchInvalid,
    remoteInvalid,
    saving,
    save,
  };
}

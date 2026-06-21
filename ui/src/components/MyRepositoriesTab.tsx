// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * My Repositories tab.
 *
 * Two sections:
 *   1. My repositories       — repos the caller has added; Remove / Force purge.
 *   2. Available repositories — repos the deployment's GitHub App / PAT can see
 *      that aren't on the caller's dashboard yet, with a search input. Click
 *      "Add" → clones the repo (if not yet on disk) and associates it with
 *      the caller. The set of addable repos is dictated by the GitHub
 *      installation/PAT scope — there is no free-form URL paste.
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { type RepositoryRow } from "../api/client";
import { useRepositoryAdmin } from "../hooks/useRepositoryAdmin";
import { ConfirmModal } from "./modals/ConfirmModal";
import { MyRepositoriesList } from "./MyRepositories/MyRepositoriesList";
import { AvailableRepositoriesList } from "./MyRepositories/AvailableRepositoriesList";

interface Props {
  isAdmin?: boolean;
}

export function MyRepositoriesTab({ isAdmin }: Props) {
  const { t } = useTranslation("config");
  const {
    mine,
    loadingMine,
    filteredAvailable,
    loadingAvailable,
    availableError,
    search,
    setSearch,
    busy,
    error,
    success,
    addFromGitHub,
    remove,
  } = useRepositoryAdmin();

  const [removeTarget, setRemoveTarget] = useState<{
    repo: RepositoryRow;
    forcePurge: boolean;
  } | null>(null);

  const handleRemove = () => {
    if (!removeTarget) return;
    const { repo, forcePurge } = removeTarget;
    setRemoveTarget(null);
    remove(repo, forcePurge);
  };

  const removeConfirmMessage = removeTarget
    ? (() => {
        const { repo, forcePurge } = removeTarget;
        const co = repo.co_users_count ?? 0;
        if (forcePurge) {
          return t("repositories.forcePurgeMessage", {
            count: co + 1,
            name: repo.name,
            path: repo.local_path,
          });
        }
        if (co === 0) {
          return t("repositories.lastUserMessage", {
            name: repo.name,
            path: repo.local_path,
          });
        }
        return t("repositories.removeMessage", { count: co, name: repo.name });
      })()
    : "";

  return (
    <div className="space-y-6">
      <header>
        <h2 className="text-base font-semibold text-gray-300 mb-1">{t("repositories.title")}</h2>
        <p className="text-sm text-gray-500">
          {t("repositories.description")}
        </p>
      </header>

      {error && <p className="text-sm text-red-400">{error}</p>}
      {success && <p className="text-sm text-green-400">{success}</p>}

      <MyRepositoriesList
        repos={mine}
        loading={loadingMine}
        busy={busy}
        onRemove={(repo, forcePurge) => setRemoveTarget({ repo, forcePurge })}
        isAdmin={isAdmin}
      />

      <AvailableRepositoriesList
        repos={filteredAvailable}
        loading={loadingAvailable}
        error={availableError}
        search={search}
        busy={busy}
        onSearchChange={setSearch}
        onAdd={addFromGitHub}
      />

      {removeTarget && (
        <ConfirmModal
          title={removeTarget.forcePurge ? t("repositories.forcePurgeTitle") : t("repositories.removeTitle")}
          message={removeConfirmMessage}
          onConfirm={handleRemove}
          onCancel={() => setRemoveTarget(null)}
        />
      )}
    </div>
  );
}

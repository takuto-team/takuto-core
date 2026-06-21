// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { listMyRepositories, type RepositoryRow } from "../../api/client";

interface Props {
  onSubmit: (name: string, description: string, repositoryId: string) => void;
  onClose: () => void;
}

/**
 * A repository dropdown is required above the description input. Without a
 * repo association the workflow has no worktree base and the engine can't
 * bootstrap it.
 */
export function PasteDescriptionModal({ onSubmit, onClose }: Props) {
  const { t } = useTranslation("modals");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [repos, setRepos] = useState<RepositoryRow[]>([]);
  const [repositoryId, setRepositoryId] = useState("");
  const [loadingRepos, setLoadingRepos] = useState(true);

  useEffect(() => {
    listMyRepositories()
      .then((rs) => {
        setRepos(rs);
        if (rs.length > 0) setRepositoryId(rs[0].id);
      })
      .catch(() => setRepos([]))
      .finally(() => setLoadingRepos(false));
  }, []);

  const noRepos = !loadingRepos && repos.length === 0;
  const canSubmit = !!description.trim() && !!repositoryId;

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl w-full max-w-3xl mx-4 max-h-[90vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between p-4 border-b border-gray-800">
          <h3 className="text-lg font-medium text-white">{t("paste.title")}</h3>
          <button onClick={onClose} className="text-gray-500 hover:text-gray-300 cursor-pointer">&times;</button>
        </div>

        <div className="p-6 flex flex-col gap-4 overflow-y-auto flex-1">
          {/* Repository selector */}
          <div>
            <label className="block text-xs text-gray-400 mb-1">{t("paste.repository")}</label>
            {loadingRepos ? (
              <p className="text-sm text-gray-500">{t("paste.loadingRepos")}</p>
            ) : noRepos ? (
              <div className="border border-amber-800/50 bg-amber-900/20 rounded-lg px-3 py-2 text-sm text-amber-200">
                {t("paste.noReposLead")}{" "}
                <Link
                  to="/config.html?tab=repositories"
                  className="text-amber-100 underline hover:text-white"
                >
                  {t("paste.noReposLink")}
                </Link>{" "}
                {t("paste.noReposTail")}
              </div>
            ) : repos.length === 1 ? (
              <div className="text-sm text-gray-200 bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 font-mono">
                {repos[0].name}
              </div>
            ) : (
              <select
                value={repositoryId}
                onChange={(e) => setRepositoryId(e.target.value)}
                className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono"
              >
                {repos.map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.name}
                  </option>
                ))}
              </select>
            )}
          </div>

          <div>
            <label className="block text-xs text-gray-400 mb-1">{t("paste.nameLabel")}</label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("paste.namePlaceholder")}
              className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
            />
          </div>
          <div className="flex-1 min-h-0">
            <label className="block text-xs text-gray-400 mb-1">{t("paste.descriptionLabel")}</label>
            <textarea
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder={t("paste.descriptionPlaceholder")}
              className="w-full h-64 bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono resize-y"
            />
          </div>
        </div>

        <div className="flex justify-end gap-3 p-4 border-t border-gray-800">
          <button
            onClick={onClose}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
          >
            {t("common.cancel")}
          </button>
          <button
            onClick={() => onSubmit(name, description, repositoryId)}
            disabled={!canSubmit}
            className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 cursor-pointer"
          >
            {t("paste.submit")}
          </button>
        </div>
      </div>
    </div>
  );
}

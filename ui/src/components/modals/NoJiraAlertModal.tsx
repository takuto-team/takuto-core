// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useTranslation } from "react-i18next";

interface Props {
  onClose: () => void;
}

export function NoJiraAlertModal({ onClose }: Props) {
  const { t } = useTranslation("modals");
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="bg-gray-900 border border-amber-700/50 rounded-xl p-6 max-w-md w-full mx-4"
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="text-lg font-medium text-amber-400 mb-2">{t("noJira.title")}</h3>
        <p className="text-sm text-gray-400 mb-4">
          {t("noJira.bodyPrefix")} <code className="text-amber-300">[general] ticketing_system</code> {t("noJira.bodyIn")}{" "}
          <code className="text-amber-300">config.toml</code> {t("noJira.bodySuffix")}
        </p>
        <div className="flex justify-end">
          <button
            onClick={onClose}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
          >
            {t("noJira.gotIt")}
          </button>
        </div>
      </div>
    </div>
  );
}

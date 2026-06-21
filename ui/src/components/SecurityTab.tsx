// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { ConfirmModal } from "./modals/ConfirmModal";
import { copyToClipboard } from "../utils/clipboard";

interface Props {
  onChangePassword: (
    currentPassword: string,
    newPassword: string,
  ) => Promise<{ error?: string }>;
  onRegenerateRecoveryCodes: () => Promise<{ recovery_codes?: string[]; error?: string }>;
}

const MIN_PASSWORD_LENGTH = 12;

function EyeIcon({ open }: { open: boolean }) {
  if (open) {
    return (
      <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4.5 h-4.5">
        <path fillRule="evenodd" d="M3.28 2.22a.75.75 0 0 0-1.06 1.06l14.5 14.5a.75.75 0 1 0 1.06-1.06l-1.745-1.745a10.029 10.029 0 0 0 3.3-4.38 1.651 1.651 0 0 0 0-1.185A10.004 10.004 0 0 0 9.999 3a9.956 9.956 0 0 0-4.744 1.194L3.28 2.22ZM7.752 6.69l1.092 1.092a2.5 2.5 0 0 1 3.374 3.373l1.092 1.092a4 4 0 0 0-5.558-5.558Z" clipRule="evenodd" />
        <path d="M10.748 13.93l2.523 2.523A9.987 9.987 0 0 1 10 17c-4.257 0-7.893-2.66-9.336-6.41a1.651 1.651 0 0 1 0-1.186A10.007 10.007 0 0 1 4.818 5.88l1.426 1.426A4 4 0 0 0 10.748 13.93Z" />
      </svg>
    );
  }
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4.5 h-4.5">
      <path d="M10 12.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z" />
      <path fillRule="evenodd" d="M.664 10.59a1.651 1.651 0 0 1 0-1.186A10.004 10.004 0 0 1 10 3c4.257 0 7.893 2.66 9.336 6.41.147.381.146.804 0 1.186A10.004 10.004 0 0 1 10 17c-4.257 0-7.893-2.66-9.336-6.41ZM14 10a4 4 0 1 1-8 0 4 4 0 0 1 8 0Z" clipRule="evenodd" />
    </svg>
  );
}

function PasswordInput({
  value,
  onChange,
  placeholder,
  autoComplete,
  showPassword,
  onToggleShow,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
  autoComplete: string;
  showPassword: boolean;
  onToggleShow: () => void;
}) {
  const { t } = useTranslation("config");
  return (
    <div className="relative">
      <input
        type={showPassword ? "text" : "password"}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        autoComplete={autoComplete}
        className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 pr-10 text-base text-gray-200"
      />
      <button
        type="button"
        onClick={onToggleShow}
        className="absolute right-2.5 top-1/2 -translate-y-1/2 text-gray-500 hover:text-gray-300 cursor-pointer"
        title={showPassword ? t("actions.hidePassword") : t("actions.showPassword")}
      >
        <EyeIcon open={showPassword} />
      </button>
    </div>
  );
}

export function SecurityTab({ onChangePassword, onRegenerateRecoveryCodes }: Props) {
  const { t } = useTranslation("config");
  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [showCurrent, setShowCurrent] = useState(false);
  const [showNew, setShowNew] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [success, setSuccess] = useState(false);
  const [confirmRegenerate, setConfirmRegenerate] = useState(false);
  const [recoveryCodes, setRecoveryCodes] = useState<string[] | null>(null);
  const [regenLoading, setRegenLoading] = useState(false);
  const [regenError, setRegenError] = useState("");
  const [codesCopied, setCodesCopied] = useState(false);

  const passwordsMatch = newPassword === confirmPassword;
  const passwordLongEnough = newPassword.length >= MIN_PASSWORD_LENGTH;
  const formValid = currentPassword.length > 0 && passwordLongEnough && passwordsMatch;

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!formValid) return;
    setError("");
    setSuccess(false);
    setLoading(true);
    try {
      const result = await onChangePassword(currentPassword, newPassword);
      if (result.error) {
        setError(result.error);
      } else {
        setSuccess(true);
        setCurrentPassword("");
        setNewPassword("");
        setConfirmPassword("");
        setShowCurrent(false);
        setShowNew(false);
      }
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="space-y-10">
      {/* Password change section */}
      <section>
        <h2 className="text-base font-semibold text-gray-300 mb-1">{t("security.changePassword")}</h2>
        <p className="text-sm text-gray-500 mb-5">
          {t("security.changePasswordHelp")}
        </p>
        <form onSubmit={handleSubmit} className="space-y-4 max-w-md">
          <div>
            <label className="block text-sm font-medium text-gray-400 mb-1.5">
              {t("security.currentPassword")}
            </label>
            <PasswordInput
              value={currentPassword}
              onChange={setCurrentPassword}
              placeholder={t("security.currentPasswordPlaceholder")}
              autoComplete="current-password"
              showPassword={showCurrent}
              onToggleShow={() => setShowCurrent(!showCurrent)}
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-400 mb-1.5">
              {t("security.newPassword")}
            </label>
            <PasswordInput
              value={newPassword}
              onChange={setNewPassword}
              placeholder={t("security.newPasswordPlaceholder")}
              autoComplete="new-password"
              showPassword={showNew}
              onToggleShow={() => setShowNew(!showNew)}
            />
            {newPassword && !passwordLongEnough && (
              <p className="text-sm text-red-400 mt-1">
                {t("security.minChars", { min: MIN_PASSWORD_LENGTH })}
              </p>
            )}
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-400 mb-1.5">
              {t("security.confirmNewPassword")}
            </label>
            <PasswordInput
              value={confirmPassword}
              onChange={setConfirmPassword}
              placeholder={t("security.confirmNewPasswordPlaceholder")}
              autoComplete="new-password"
              showPassword={showNew}
              onToggleShow={() => setShowNew(!showNew)}
            />
            {confirmPassword && !passwordsMatch && (
              <p className="text-sm text-red-400 mt-1">{t("security.passwordsNoMatch")}</p>
            )}
          </div>

          {error && <p className="text-sm text-red-400">{error}</p>}
          {success && (
            <p className="text-sm text-green-400">{t("security.passwordChanged")}</p>
          )}

          <button
            type="submit"
            disabled={loading || !formValid}
            className="px-5 py-2 rounded-lg bg-blue-600 text-white text-sm font-medium hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
          >
            {loading ? t("security.savingPassword") : t("security.updatePassword")}
          </button>
        </form>
      </section>

      <hr className="border-gray-800" />

      {/* Recovery codes section */}
      <section>
        <h2 className="text-base font-semibold text-gray-300 mb-1">{t("security.recoveryCodes")}</h2>
        <p className="text-sm text-gray-500 mb-4">
          {t("security.recoveryCodesHelp")}
        </p>

        {recoveryCodes ? (
          <div className="space-y-3 max-w-md">
            <div className="bg-amber-950 border border-amber-700 rounded-lg p-4">
              <p className="text-xs text-amber-200/80 mb-3">
                {t("security.saveCodesNow")}
              </p>
              <div className="grid grid-cols-2 gap-2 mb-3 font-mono text-sm">
                {recoveryCodes.map((code) => (
                  <div
                    key={code}
                    className="bg-gray-950 border border-gray-700 rounded px-3 py-1.5 text-gray-200 text-center"
                  >
                    {code}
                  </div>
                ))}
              </div>
              <button
                type="button"
                onClick={async () => {
                  const ok = await copyToClipboard(recoveryCodes.join("\n"));
                  if (ok) { setCodesCopied(true); setTimeout(() => setCodesCopied(false), 2000); }
                }}
                className="w-full py-1.5 rounded-lg bg-gray-800 text-gray-300 text-xs font-medium hover:bg-gray-700 cursor-pointer"
              >
                {codesCopied ? t("actions.copied") : t("actions.copyAllCodes")}
              </button>
            </div>
            <button
              type="button"
              onClick={() => { setRecoveryCodes(null); setCodesCopied(false); }}
              className="text-sm text-gray-500 hover:text-gray-300 cursor-pointer"
            >
              {t("actions.done")}
            </button>
          </div>
        ) : (
          <div>
            {regenError && <p className="text-sm text-red-400 mb-2">{regenError}</p>}
            <button
              type="button"
              disabled={regenLoading}
              onClick={() => setConfirmRegenerate(true)}
              className="px-5 py-2 rounded-lg bg-gray-800 text-gray-300 text-sm font-medium border border-gray-700 hover:bg-gray-700 disabled:opacity-50 cursor-pointer"
            >
              {regenLoading ? t("security.regenerating") : t("security.regenerate")}
            </button>
          </div>
        )}

        {confirmRegenerate && (
          <ConfirmModal
            title={t("security.regenerateTitle")}
            message={t("security.regenerateMessage")}
            onConfirm={async () => {
              setConfirmRegenerate(false);
              setRegenError("");
              setRegenLoading(true);
              try {
                const result = await onRegenerateRecoveryCodes();
                if (result.error) {
                  setRegenError(result.error);
                } else {
                  setRecoveryCodes(result.recovery_codes ?? null);
                }
              } finally {
                setRegenLoading(false);
              }
            }}
            onCancel={() => setConfirmRegenerate(false)}
          />
        )}
      </section>

      <hr className="border-gray-800" />

      {/* Passkeys section */}
      <section>
        <h2 className="text-base font-semibold text-gray-300 mb-1">{t("security.passkeys")}</h2>
        <p className="text-sm text-gray-500">
          {t("security.passkeysHelp")}
        </p>
        <div className="mt-4 rounded-lg border border-gray-800 bg-gray-900/50 px-4 py-3">
          <p className="text-sm text-gray-500">{t("security.comingSoon")}</p>
        </div>
      </section>
    </div>
  );
}

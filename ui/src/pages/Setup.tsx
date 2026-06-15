// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, type FormEvent } from "react";
import { copyToClipboard } from "../utils/clipboard";

interface RegisterResponse {
  user_id: string;
  username: string;
  role: string;
  recovery_codes: string[];
}

interface Props {
  onSetupComplete: () => void;
  onLogin: (username: string, password: string) => Promise<boolean>;
}

const MIN_PASSWORD_LENGTH = 12;

function PasswordStrengthBar({ password }: { password: string }) {
  const length = password.length;
  let strength = 0;
  if (length >= MIN_PASSWORD_LENGTH) strength++;
  if (length >= 16) strength++;
  if (/[A-Z]/.test(password) && /[a-z]/.test(password)) strength++;
  if (/\d/.test(password)) strength++;
  if (/[^A-Za-z0-9]/.test(password)) strength++;

  const colors = ["bg-red-500", "bg-orange-500", "bg-yellow-500", "bg-lime-500", "bg-green-500"];
  const labels = ["Very weak", "Weak", "Fair", "Good", "Strong"];
  const idx = Math.min(strength, 4);

  if (!password) return null;

  return (
    <div className="mt-1">
      <div className="flex gap-1 mb-1">
        {Array.from({ length: 5 }, (_, i) => (
          <div
            key={i}
            className={`h-1 flex-1 rounded-full ${i <= idx ? colors[idx] : "bg-gray-700"}`}
          />
        ))}
      </div>
      <p className="text-xs text-gray-400">{labels[idx]}</p>
    </div>
  );
}

export function Setup({ onSetupComplete, onLogin }: Props) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [recoveryCodes, setRecoveryCodes] = useState<string[] | null>(null);
  const [codesCopied, setCodesCopied] = useState(false);
  const [codesAcknowledged, setCodesAcknowledged] = useState(false);

  const passwordsMatch = password === confirmPassword;
  const passwordLongEnough = password.length >= MIN_PASSWORD_LENGTH;
  const formValid =
    username.trim().length > 0 && passwordLongEnough && passwordsMatch;

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!formValid) return;

    setError("");
    setLoading(true);
    try {
      const res = await fetch("/api/auth/register", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "same-origin",
        body: JSON.stringify({ username: username.trim(), password }),
      });

      if (!res.ok) {
        const body = await res.json().catch(() => null);
        setError(body?.error ?? `Registration failed (${res.status})`);
        return;
      }

      const data = (await res.json()) as RegisterResponse;
      setRecoveryCodes(data.recovery_codes);
    } catch {
      setError("Registration failed — could not reach the server.");
    } finally {
      setLoading(false);
    }
  };

  const handleCopyCodes = async () => {
    if (!recoveryCodes) return;
    const ok = await copyToClipboard(recoveryCodes.join("\n"));
    if (ok) { setCodesCopied(true); setTimeout(() => setCodesCopied(false), 2000); }
  };

  const handleContinue = async () => {
    // Auto-login with the credentials just created
    await onLogin(username.trim(), password);
    onSetupComplete();
    // Phase 1 (auth-overhaul): the just-created admin lands in the 4-step
    // onboarding wizard instead of the empty dashboard. A full navigation
    // makes the session-cookie-aware re-bootstrap unambiguous; Setup is
    // rendered outside the Router so we can't useNavigate() here.
    window.location.replace("/onboarding");
  };

  // Recovery codes screen — shown after successful registration
  if (recoveryCodes) {
    return (
      <div className="min-h-screen flex items-center justify-center">
        <div className="w-full max-w-md mx-4">
          <h1 className="text-2xl font-bold text-white text-center mb-2">Takuto</h1>
          <p className="text-sm text-gray-400 text-center mb-6">Account created successfully</p>
          <div className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-4">
            <div className="bg-amber-950 border border-amber-700 rounded-lg p-4">
              <h2 className="text-sm font-semibold text-amber-300 mb-2">
                Save your recovery codes
              </h2>
              <p className="text-xs text-amber-200/80 mb-3">
                These codes are the only way to recover your account if you lose your password.
                Each code can only be used once. Store them in a secure location.
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
                onClick={handleCopyCodes}
                className="w-full py-1.5 rounded-lg bg-gray-800 text-gray-300 text-xs font-medium hover:bg-gray-700 cursor-pointer"
              >
                {codesCopied ? "Copied" : "Copy all codes"}
              </button>
            </div>

            <label className="flex items-start gap-2 text-xs text-gray-400 cursor-pointer select-none">
              <input
                type="checkbox"
                checked={codesAcknowledged}
                onChange={(e) => setCodesAcknowledged(e.target.checked)}
                className="mt-0.5 accent-blue-500"
              />
              I have saved my recovery codes in a secure location
            </label>

            <button
              type="button"
              disabled={!codesAcknowledged}
              onClick={handleContinue}
              className="w-full py-2 rounded-lg bg-blue-600 text-white text-sm font-medium hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
            >
              Continue to dashboard
            </button>
          </div>
        </div>
      </div>
    );
  }

  // Registration form
  return (
    <div className="min-h-screen flex items-center justify-center">
      <div className="w-full max-w-sm mx-4">
        <h1 className="text-2xl font-bold text-white text-center mb-2">Takuto</h1>
        <p className="text-sm text-gray-400 text-center mb-6">Create your admin account</p>
        <form
          onSubmit={handleSubmit}
          className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-4"
        >
          <div>
            <label className="block text-xs text-gray-400 mb-1">Username</label>
            <input
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              autoFocus
              autoComplete="username"
              className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
            />
          </div>
          <div>
            <label className="block text-xs text-gray-400 mb-1">Password</label>
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoComplete="new-password"
              className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
            />
            <PasswordStrengthBar password={password} />
            {password && !passwordLongEnough && (
              <p className="text-xs text-red-400 mt-1">
                Minimum {MIN_PASSWORD_LENGTH} characters
              </p>
            )}
          </div>
          <div>
            <label className="block text-xs text-gray-400 mb-1">Confirm password</label>
            <input
              type="password"
              value={confirmPassword}
              onChange={(e) => setConfirmPassword(e.target.value)}
              autoComplete="new-password"
              className={`w-full bg-gray-950 border rounded-lg px-3 py-2 text-sm text-gray-200 ${
                confirmPassword && !passwordsMatch ? "border-red-500" : "border-gray-700"
              }`}
            />
            {confirmPassword && !passwordsMatch && (
              <p className="text-xs text-red-400 mt-1">Passwords do not match</p>
            )}
          </div>
          {error && <p className="text-xs text-red-400">{error}</p>}
          <button
            type="submit"
            disabled={loading || !formValid}
            className="w-full py-2 rounded-lg bg-blue-600 text-white text-sm font-medium hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
          >
            {loading ? "Creating account..." : "Create admin account"}
          </button>
          <p className="text-xs text-gray-500 text-center">
            This is a one-time setup. Additional users can be created from the admin panel.
          </p>
        </form>
      </div>
    </div>
  );
}

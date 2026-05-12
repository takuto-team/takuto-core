// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, type FormEvent } from "react";

interface Props {
  onLogin: (username: string, password: string) => Promise<boolean>;
}

function RecoveryForm({ onDone }: { onDone: () => void }) {
  const [username, setUsername] = useState("");
  const [recoveryCode, setRecoveryCode] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [success, setSuccess] = useState(false);

  const passwordsMatch = newPassword === confirmPassword;
  const passwordLongEnough = newPassword.length >= 12;
  const formValid =
    username.trim().length > 0 &&
    recoveryCode.trim().length > 0 &&
    passwordLongEnough &&
    passwordsMatch;

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!formValid) return;
    setError("");
    setLoading(true);
    try {
      const res = await fetch("/api/auth/recover", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "same-origin",
        body: JSON.stringify({
          username: username.trim(),
          recovery_code: recoveryCode.trim(),
          new_password: newPassword,
        }),
      });
      if (res.ok) {
        setSuccess(true);
      } else {
        const body = await res.json().catch(() => null);
        setError(body?.error ?? "Recovery failed");
      }
    } catch {
      setError("Could not reach the server.");
    } finally {
      setLoading(false);
    }
  };

  if (success) {
    return (
      <div className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-4">
        <p className="text-sm text-green-400">
          Password reset successfully. You can now sign in with your new password.
        </p>
        <button
          type="button"
          onClick={onDone}
          className="w-full py-2 rounded-lg bg-blue-600 text-white text-sm font-medium hover:bg-blue-500 cursor-pointer"
        >
          Back to sign in
        </button>
      </div>
    );
  }

  return (
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
        <label className="block text-xs text-gray-400 mb-1">Recovery code</label>
        <input
          type="text"
          value={recoveryCode}
          onChange={(e) => setRecoveryCode(e.target.value)}
          placeholder="XXXX-XXXX"
          autoComplete="off"
          className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono tracking-wider"
        />
      </div>
      <div>
        <label className="block text-xs text-gray-400 mb-1">New password</label>
        <input
          type="password"
          value={newPassword}
          onChange={(e) => setNewPassword(e.target.value)}
          autoComplete="new-password"
          className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
        />
        {newPassword && !passwordLongEnough && (
          <p className="text-xs text-red-400 mt-1">Minimum 12 characters</p>
        )}
      </div>
      <div>
        <label className="block text-xs text-gray-400 mb-1">Confirm new password</label>
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
        {loading ? "Resetting..." : "Reset password"}
      </button>
      <button
        type="button"
        onClick={onDone}
        className="text-xs text-gray-500 hover:text-gray-300 text-center cursor-pointer"
      >
        Back to sign in
      </button>
    </form>
  );
}

export function Login({ onLogin }: Props) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [showRecovery, setShowRecovery] = useState(false);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      const ok = await onLogin(username, password);
      if (!ok) setError("Invalid credentials");
    } catch {
      setError("Login failed");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="min-h-screen flex items-center justify-center">
      <div className="w-full max-w-sm mx-4">
        <h1 className="text-2xl font-bold text-white text-center mb-8">Maestro</h1>
        {showRecovery ? (
          <RecoveryForm onDone={() => setShowRecovery(false)} />
        ) : (
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
                className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
              />
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Password</label>
              <input
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200"
              />
            </div>
            {error && <p className="text-xs text-red-400">{error}</p>}
            <button
              type="submit"
              disabled={loading}
              className="w-full py-2 rounded-lg bg-blue-600 text-white text-sm font-medium hover:bg-blue-500 disabled:opacity-50 cursor-pointer"
            >
              {loading ? "Signing in..." : "Sign in"}
            </button>
            <button
              type="button"
              onClick={() => setShowRecovery(true)}
              className="text-xs text-gray-500 hover:text-gray-300 text-center cursor-pointer"
            >
              Forgot password?
            </button>
          </form>
        )}
      </div>
    </div>
  );
}

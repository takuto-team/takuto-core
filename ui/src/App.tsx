import { Routes, Route, Navigate } from "react-router-dom";
import { Dashboard } from "./pages/Dashboard";
import { Login } from "./pages/Login";
import { Config } from "./pages/Config";
import { useAuth } from "./hooks/useAuth";

export function App() {
  const { authEnabled, loggedIn, loading, login, logout } = useAuth();

  if (loading) {
    return (
      <div className="flex items-center justify-center min-h-screen">
        <span className="text-gray-500 text-sm">Loading...</span>
      </div>
    );
  }

  if (authEnabled && !loggedIn) {
    return <Login onLogin={login} />;
  }

  return (
    <Routes>
      <Route path="/" element={<Dashboard onLogout={logout} authEnabled={authEnabled} />} />
      <Route path="/login.html" element={<Login onLogin={login} />} />
      <Route path="/config.html" element={<Config onLogout={logout} authEnabled={authEnabled} />} />
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  );
}

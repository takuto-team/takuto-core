import { Link } from "react-router-dom";

interface Props {
  connected: boolean;
  authEnabled: boolean;
  onLogout: () => void;
}

export function Header({ connected, authEnabled, onLogout }: Props) {
  return (
    <header className="border-b border-gray-800 bg-gray-950/80 backdrop-blur-sm sticky top-0 z-40">
      <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
        <div className="flex items-center justify-between h-14">
          <div className="flex items-center gap-3">
            <span className="text-lg font-bold tracking-tight text-white">Maestro</span>
            <span className="text-xs px-2 py-0.5 rounded-full bg-gray-800 text-gray-400 border border-gray-700">
              Dashboard
            </span>
          </div>

          <div className="flex items-center gap-4">
            <span className="flex items-center gap-1.5 text-xs text-gray-400">
              <span
                className={`inline-block w-2 h-2 rounded-full ${
                  connected ? "bg-green-500 animate-pulse" : "bg-gray-600"
                }`}
              />
              {connected ? "Connected" : "Disconnected"}
            </span>

            <Link
              to="/config.html"
              className="text-xs text-gray-400 hover:text-gray-200 transition-colors"
            >
              Configuration
            </Link>

            {authEnabled && (
              <button
                onClick={onLogout}
                className="text-xs text-gray-500 hover:text-gray-300 transition-colors cursor-pointer"
              >
                Log out
              </button>
            )}
          </div>
        </div>
      </div>
    </header>
  );
}

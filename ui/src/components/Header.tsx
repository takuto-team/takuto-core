// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { Link } from "react-router-dom";

interface Props {
  connected: boolean;
  authEnabled: boolean;
  githubAppConfigured: boolean;
  githubAppInstallationId?: number;
  onLogout: () => void;
}

export function Header({ connected, authEnabled, githubAppConfigured, githubAppInstallationId, onLogout }: Props) {
  return (
    <header className="border-b border-gray-800 bg-gray-950/80 backdrop-blur-sm sticky top-0 z-40">
      <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
        <div className="flex items-center justify-between h-14">
          <div className="flex items-center gap-3">
            <span className="text-lg font-bold tracking-tight text-white">Maestro</span>
            <span className="text-xs px-2 py-0.5 rounded-full bg-gray-800 text-gray-400 border border-gray-700">
              Dashboard
            </span>
            {githubAppConfigured && (
              <span
                className="inline-flex items-center gap-1.5 text-xs bg-violet-950/60 border border-violet-700/50 text-violet-300 px-2 py-0.5 rounded-full"
                title="GitHub App bot connected"
              >
                {githubAppInstallationId ? (
                  <img
                    src={`https://avatars.githubusercontent.com/in/${githubAppInstallationId}?s=28`}
                    alt=""
                    className="w-3.5 h-3.5 rounded-full"
                  />
                ) : (
                  <GitHubIcon />
                )}
                Bot Connected
              </span>
            )}
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

function GitHubIcon() {
  return (
    <svg className="w-3.5 h-3.5 flex-shrink-0" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M12 0C5.374 0 0 5.373 0 12c0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576C20.566 21.797 24 17.3 24 12c0-6.627-5.373-12-12-12z" />
    </svg>
  );
}

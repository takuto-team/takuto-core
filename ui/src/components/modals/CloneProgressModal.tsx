// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

interface Props {
  repoName: string;
  status: "cloning" | "success" | "error";
  error?: string;
  onDone: () => void;
  onRetry: () => void;
  onCancel?: () => void;
}

export function CloneProgressModal({
  repoName,
  status,
  error,
  onDone,
  onRetry,
  onCancel,
}: Props) {
  return (
    <div className="modal-backdrop">
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl max-w-md w-full mx-4 p-6"
        onClick={(e) => e.stopPropagation()}
      >
        {status === "cloning" && (
          <div className="text-center">
            <div className="inline-block w-8 h-8 border-2 border-blue-500 border-t-transparent rounded-full animate-spin mb-4" />
            <h3 className="text-lg font-medium text-white mb-2">
              Cloning Repository
            </h3>
            <p className="text-sm text-gray-400">
              Cloning <span className="font-mono text-blue-400">{repoName}</span>...
            </p>
            <p className="text-xs text-gray-500 mt-2">
              This may take a moment depending on the repository size.
            </p>
            {onCancel && (
              <button
                onClick={onCancel}
                className="mt-4 text-xs text-gray-500 hover:text-gray-300 transition-colors cursor-pointer"
              >
                Cancel
              </button>
            )}
          </div>
        )}

        {status === "success" && (
          <div className="text-center">
            <div className="text-4xl mb-4">
              <span className="text-emerald-400">&#10003;</span>
            </div>
            <h3 className="text-lg font-medium text-white mb-2">
              Repository Cloned
            </h3>
            <p className="text-sm text-gray-400 mb-6">
              <span className="font-mono text-blue-400">{repoName}</span> has been
              cloned successfully.
            </p>
            <button
              onClick={onDone}
              className="text-sm px-6 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 transition-colors cursor-pointer"
            >
              Continue
            </button>
          </div>
        )}

        {status === "error" && (
          <div className="text-center">
            <div className="text-4xl mb-4">
              <span className="text-red-400">&#10007;</span>
            </div>
            <h3 className="text-lg font-medium text-white mb-2">
              Clone Failed
            </h3>
            <p className="text-sm text-red-400 mb-4">
              {error || "An unknown error occurred while cloning the repository."}
            </p>
            <div className="flex gap-3 justify-center">
              <button
                onClick={onRetry}
                className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 transition-colors cursor-pointer"
              >
                Retry
              </button>
              <button
                onClick={onDone}
                className="text-sm px-4 py-2 rounded-lg bg-gray-700 text-gray-300 hover:bg-gray-600 transition-colors cursor-pointer"
              >
                Close
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

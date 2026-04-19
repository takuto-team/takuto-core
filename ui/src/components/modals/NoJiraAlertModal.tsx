interface Props {
  onClose: () => void;
}

export function NoJiraAlertModal({ onClose }: Props) {
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="bg-gray-900 border border-amber-700/50 rounded-xl p-6 max-w-md w-full mx-4"
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="text-lg font-medium text-amber-400 mb-2">No Ticketing System</h3>
        <p className="text-sm text-gray-400 mb-4">
          No ticketing system is configured. Workflows can only be started manually via the
          dashboard. Configure <code className="text-amber-300">[general] ticketing_system</code> in{" "}
          <code className="text-amber-300">config.toml</code> to enable Jira or GitHub polling.
        </p>
        <div className="flex justify-end">
          <button
            onClick={onClose}
            className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
          >
            Got it
          </button>
        </div>
      </div>
    </div>
  );
}

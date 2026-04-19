interface Props {
  paused: boolean;
  toggling: boolean;
  ticketingSystem: string;
  onToggle: () => void;
}

export function PollingLabel({ paused, toggling, ticketingSystem, onToggle }: Props) {
  if (ticketingSystem === "none") return null;

  return (
    <div className="w-full bg-gray-900/60 border-b border-gray-800 px-4 py-1.5 flex items-center justify-center">
      {paused ? (
        <button
          onClick={onToggle}
          disabled={toggling}
          className="text-xs text-amber-400/80 hover:text-amber-300 transition-colors cursor-pointer disabled:opacity-50"
        >
          Polling: paused &mdash; click to resume
        </button>
      ) : (
        <button
          onClick={onToggle}
          disabled={toggling}
          className="text-xs text-emerald-500/70 hover:text-emerald-400 transition-colors cursor-pointer disabled:opacity-50"
        >
          Polling: active
        </button>
      )}
    </div>
  );
}

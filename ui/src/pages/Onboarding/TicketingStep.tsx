// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

export function TicketingStep({ ticketingSystem }: { ticketingSystem: string }) {
  return (
    <div className="bg-gray-950/60 border border-gray-800 rounded-lg p-4 text-sm text-gray-300">
      <p>
        Current ticketing system: <strong>{ticketingSystem || "none"}</strong>
      </p>
      <p className="text-xs text-gray-500 mt-2">
        Change this by editing{" "}
        <code className="text-gray-400">[general] ticketing_system</code> in{" "}
        <code className="text-gray-400">config.toml</code>. A dashboard editor
        ships in a later phase.
      </p>
    </div>
  );
}

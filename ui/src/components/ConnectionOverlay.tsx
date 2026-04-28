// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect, useRef } from "react";
import { ComputerIcon } from "./icons";

const DOT_COUNT = 7;
const STEP_MS = 220;
const PAUSE_MS = 500;

export function ConnectionOverlay({ message }: { message: string }) {
  const [lit, setLit] = useState(0);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    const tick = () => {
      setLit((prev) => {
        if (prev >= DOT_COUNT) {
          // All lit — pause then reset
          timer.current = setTimeout(tick, PAUSE_MS);
          return 0;
        }
        timer.current = setTimeout(tick, STEP_MS);
        return prev + 1;
      });
    };
    timer.current = setTimeout(tick, STEP_MS);
    return () => clearTimeout(timer.current);
  }, []);

  return (
    <div className="flex flex-col items-center gap-4">
      <span className="text-sm text-gray-300">{message}</span>
      <div className="flex items-center gap-0">
        <ComputerIcon />
        <div className="flex items-center gap-1.5 px-3">
          {Array.from({ length: DOT_COUNT }, (_, i) => (
            <span
              key={i}
              className="connection-dot"
              style={{ backgroundColor: i < lit ? "#22c55e" : undefined }}
            />
          ))}
        </div>
        <ComputerIcon />
      </div>
    </div>
  );
}

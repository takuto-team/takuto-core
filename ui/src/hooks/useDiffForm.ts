// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback, useMemo, useState } from "react";

/**
 * Generic diff-aware form state.
 *
 * Tracks an editable `value` plus its `original` baseline. `dirty` is
 * `true` whenever the two diverge under the supplied equality.
 *
 * Designed for forms that round-trip to a server (load → edit → save):
 *   `setValue` mutates the editing copy;
 *   `replaceOriginal(v)` after a successful save resets both to `v`;
 *   `reset()` after a cancel returns the editing copy to the baseline.
 *
 * Equality defaults to JSON.stringify so plain-data shapes (string lists,
 * arrays of POJOs) work out of the box. Callers with non-JSON-clean
 * shapes (functions, classes, Maps) must supply their own comparator.
 */
export function useDiffForm<T>(
  initial: T,
  isEqual: (a: T, b: T) => boolean = defaultEqual,
) {
  const [value, setValue] = useState<T>(initial);
  const [original, setOriginal] = useState<T>(initial);

  const dirty = useMemo(() => !isEqual(value, original), [value, original, isEqual]);

  const replaceOriginal = useCallback((next: T) => {
    setValue(next);
    setOriginal(next);
  }, []);

  const reset = useCallback(() => setValue(original), [original]);

  return { value, original, dirty, setValue, replaceOriginal, reset };
}

function defaultEqual<T>(a: T, b: T): boolean {
  if (a === b) return true;
  return JSON.stringify(a) === JSON.stringify(b);
}

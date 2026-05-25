// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { RunCommand } from "../../api/client";

export const MAX_COMMANDS = 50;
export const MAX_COMMAND_LEN = 2000;
export const MAX_NAME_LEN = 100;

/**
 * Pure pre-flight validation for the Worktree Settings editor. Returns
 * the first error message (string) or `null` when both lists pass.
 *
 * Mirrors the server-side `user_worktree_commands::upsert` checks so the
 * Save button can be disabled before the round-trip; the server still
 * re-validates on PUT.
 */
export function validateCommands(
  init: string[],
  run: RunCommand[],
): string | null {
  if (init.length > MAX_COMMANDS) {
    return `Too many init commands (limit ${MAX_COMMANDS}).`;
  }
  for (let i = 0; i < init.length; i += 1) {
    const trimmed = init[i].trim();
    if (trimmed.length === 0) {
      return `Init command #${i + 1} is empty.`;
    }
    if (trimmed.length > MAX_COMMAND_LEN) {
      return `Init command #${i + 1} exceeds ${MAX_COMMAND_LEN} characters.`;
    }
    if (init[i].includes("\0")) {
      return `Init command #${i + 1} contains a NUL byte.`;
    }
  }

  if (run.length > MAX_COMMANDS) {
    return `Too many run commands (limit ${MAX_COMMANDS}).`;
  }
  const seenNames = new Set<string>();
  for (let i = 0; i < run.length; i += 1) {
    const rc = run[i];
    const name = rc.name.trim();
    const cmd = rc.command.trim();
    if (name.length === 0) {
      return `Run command #${i + 1}: name is empty.`;
    }
    if (name.length > MAX_NAME_LEN) {
      return `Run command #${i + 1}: name exceeds ${MAX_NAME_LEN} characters.`;
    }
    if (cmd.length === 0) {
      return `Run command #${i + 1}: command is empty.`;
    }
    if (cmd.length > MAX_COMMAND_LEN) {
      return `Run command #${i + 1}: command exceeds ${MAX_COMMAND_LEN} characters.`;
    }
    if (rc.name.includes("\0") || rc.command.includes("\0")) {
      return `Run command #${i + 1}: contains a NUL byte.`;
    }
    if (seenNames.has(name)) {
      return `Run command #${i + 1}: duplicate name "${name}".`;
    }
    seenNames.add(name);
  }
  return null;
}

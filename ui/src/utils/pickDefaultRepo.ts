// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Default repo selection for the Repository Settings / Workflows sidebars:
 * prefer the active repo when it's present AND accessible, else the first
 * accessible repo, else the first repo (so something is always selected even
 * when every repo is inaccessible). `access[name] === false` means "no access".
 */
export function pickDefaultRepo(
  repoNames: string[],
  activeRepoName: string | null,
  access: Record<string, boolean>,
): string | null {
  const accessible = (name: string) => access[name] !== false;
  if (activeRepoName && repoNames.includes(activeRepoName) && accessible(activeRepoName)) {
    return activeRepoName;
  }
  return repoNames.find(accessible) ?? repoNames[0] ?? null;
}

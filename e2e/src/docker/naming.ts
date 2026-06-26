import { randomBytes } from 'node:crypto';

/** Supported database backends. One Playwright project runs per backend. */
export type Backend = 'sqlite' | 'postgres' | 'mysql';

export const BACKENDS: readonly Backend[] = ['sqlite', 'postgres', 'mysql'];

/** Label applied to every Docker resource the suite creates. */
export const LABEL_E2E = 'com.takuto.e2e';
/** Label carrying the per-run id, used by the global teardown sweep. */
export const LABEL_RUN = 'com.takuto.e2e.run';
/** Label carrying the per-stack id, used to tear down a single stack. */
export const LABEL_STACK = 'com.takuto.e2e.stack';

/**
 * Stable id for the whole test run. Taken from `TAKUTO_E2E_RUN_ID` when set
 * (so an external orchestrator can sweep afterwards) or generated once per
 * process. Every resource is labelled with this so teardown can find them all.
 */
export const RUN_ID: string = process.env.TAKUTO_E2E_RUN_ID || `r${rand(8)}`;

/** Roles a container/volume/network can play inside one stack. */
export type ResourceRole = 'net' | 'db' | 'app' | 'data' | 'dbdata';

/** Random lowercase-hex token of `len` characters. */
export function rand(len: number): string {
  return randomBytes(Math.ceil(len / 2))
    .toString('hex')
    .slice(0, len);
}

/** A unique id for one stack instance, e.g. `postgres-1a2b3c4d`. */
export function newStackId(backend: Backend): string {
  return `${backend}-${rand(8)}`;
}

/** Resource name `takuto-e2e-<backend>-<role>-<rand8>` for a given stack. */
export function resourceName(stackId: string, role: ResourceRole): string {
  return `takuto-e2e-${stackId}-${role}-${rand(8)}`;
}

/** Labels every resource in a stack carries (`docker run --label ...`). */
export function stackLabels(stackId: string): Record<string, string> {
  return {
    [LABEL_E2E]: 'true',
    [LABEL_RUN]: RUN_ID,
    [LABEL_STACK]: stackId,
  };
}

/** Flatten a label map into repeated `--label key=value` CLI arguments. */
export function labelArgs(labels: Record<string, string>): string[] {
  return Object.entries(labels).flatMap(([k, v]) => ['--label', `${k}=${v}`]);
}

import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { execa } from 'execa';
import { docker, imageExists } from './cli.js';

const HERE = dirname(fileURLToPath(import.meta.url));
/** Repository root: e2e/src/docker → ../../.. */
export const REPO_ROOT = resolve(HERE, '../../..');

/**
 * Paths whose content defines the built image. Changing anything outside this
 * set (e.g. the `e2e/` workspace itself) must NOT invalidate the build cache.
 */
const SOURCE_PATHS = [
  'crates',
  'ui/src',
  'ui/public',
  'ui/index.html',
  'ui/package.json',
  'ui/package-lock.json',
  'ui/vite.config.ts',
  'ui/tsconfig.json',
  'ui/tsconfig.app.json',
  'ui/tsconfig.node.json',
  'Cargo.toml',
  'Cargo.lock',
  'Dockerfile',
  '.dockerignore',
  'config.toml.example',
];

const BUILD_TARGET = process.env.TAKUTO_E2E_BUILD_TARGET || 'runtime-base';

async function git(args: string[]): Promise<string> {
  const { stdout } = await execa('git', args, { cwd: REPO_ROOT, reject: false });
  return stdout;
}

/**
 * Hash the working-tree content of the source paths. `git ls-files -s` gives
 * the committed blob hashes; `git diff` captures uncommitted modifications, so
 * the digest tracks the actual working tree, not just HEAD.
 */
async function sourceHash(): Promise<string> {
  const tracked = await git(['ls-files', '-s', '--', ...SOURCE_PATHS]);
  const dirty = await git(['diff', 'HEAD', '--', ...SOURCE_PATHS]);
  return createHash('sha256')
    .update(BUILD_TARGET)
    .update('\0')
    .update(tracked)
    .update('\0')
    .update(dirty)
    .digest('hex')
    .slice(0, 16);
}

/**
 * Resolve the image tag to use for this run without building. Returns the
 * `TAKUTO_E2E_IMAGE` override verbatim when set, otherwise `takuto-e2e:<hash>`.
 */
export async function resolveImageTag(): Promise<string> {
  const override = process.env.TAKUTO_E2E_IMAGE;
  if (override) {
    return override;
  }
  return `takuto-e2e:${await sourceHash()}`;
}

/**
 * Ensure the Takuto image exists locally, building it from the working tree if
 * needed. Honors `TAKUTO_E2E_IMAGE` (skip build, use as-is) and the source-hash
 * cache (skip build when the tag already exists). Returns the resolved tag.
 */
export async function ensureImage(log: (msg: string) => void = () => {}): Promise<string> {
  const override = process.env.TAKUTO_E2E_IMAGE;
  if (override) {
    log(`Using pre-built image ${override} (TAKUTO_E2E_IMAGE)`);
    return override;
  }

  const tag = `takuto-e2e:${await sourceHash()}`;
  if (await imageExists(tag)) {
    log(`Reusing cached image ${tag}`);
    return tag;
  }

  log(`Building image ${tag} from working tree (target ${BUILD_TARGET})…`);
  await docker(
    [
      'build',
      '--target',
      BUILD_TARGET,
      '--build-arg',
      'TAKUTO_BUILD_CONFIG=config.toml.example',
      '--tag',
      tag,
      '.',
    ],
    { cwd: REPO_ROOT, stdout: 'inherit', stderr: 'inherit' },
  );
  log(`Built ${tag}`);
  return tag;
}

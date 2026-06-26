import { execFileSync } from 'node:child_process';
import { pull } from './src/docker/cli.js';
import { ensureImage } from './src/docker/image.js';
import { LABEL_RUN, RUN_ID } from './src/docker/naming.js';

const DB_IMAGES = ['postgres:16', 'mariadb:11'];

/**
 * Synchronously remove every container/network/volume carrying this run's
 * label. Used only from the signal handler below, where async teardown can't
 * be awaited before the process dies — so we shell out with blocking
 * `execFileSync`. Best-effort: every step is wrapped, a failure never aborts
 * the rest. (`execFileSync` takes an argv array — never a shell string — so the
 * random ids can't be misinterpreted.)
 */
function sweepSync(label: string): void {
  const list = (kind: 'container' | 'network' | 'volume'): string[] => {
    const args =
      kind === 'container'
        ? ['ps', '-aq', '--filter', `label=${label}`]
        : [kind, 'ls', '-q', '--filter', `label=${label}`];
    try {
      return execFileSync('docker', args, { encoding: 'utf8' })
        .split('\n')
        .map((s) => s.trim())
        .filter(Boolean);
    } catch {
      return [];
    }
  };
  const remove = (args: string[]): void => {
    try {
      execFileSync('docker', args, { stdio: 'ignore' });
    } catch {
      /* best-effort */
    }
  };
  const containers = list('container');
  if (containers.length) remove(['rm', '-f', ...containers]);
  const networks = list('network');
  if (networks.length) remove(['network', 'rm', ...networks]);
  const volumes = list('volume');
  if (volumes.length) remove(['volume', 'rm', '-f', ...volumes]);
}

/**
 * Install a one-shot SIGINT/SIGTERM handler that sweeps this run's resources
 * before exiting. Without it, a Ctrl-C / kill mid-run skips both the per-worker
 * fixture teardown and `globalTeardown`, orphaning containers. (SIGKILL / `kill
 * -9` cannot be trapped by any process — recover those manually by label; see
 * AGENTS.md.)
 */
function installSignalSweep(): void {
  let swept = false;
  const handler = (sig: NodeJS.Signals): void => {
    if (swept) return;
    swept = true;
    console.log(`[e2e:setup] ${sig} — sweeping resources for run ${RUN_ID} before exit`);
    sweepSync(`${LABEL_RUN}=${RUN_ID}`);
    process.exit(sig === 'SIGINT' ? 130 : 143);
  };
  process.once('SIGINT', handler);
  process.once('SIGTERM', handler);
}

/**
 * Build the Takuto image from the working tree once per run (cached by source
 * hash) and pre-pull the database images so per-worker stack bring-up is fast
 * and not racing on a shared `docker pull`.
 */
export default async function globalSetup(): Promise<void> {
  const log = (msg: string): void => {
    console.log(`[e2e:setup] ${msg}`);
  };
  log(`run id ${RUN_ID}`);
  installSignalSweep();

  const tag = await ensureImage(log);
  log(`takuto image: ${tag}`);

  await Promise.all(
    DB_IMAGES.map(async (image) => {
      log(`pulling ${image}…`);
      await pull(image);
    }),
  );
  log('database images ready');
}

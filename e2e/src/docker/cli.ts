import { execa, type Options, type Result } from 'execa';
import { labelArgs } from './naming.js';

/** Options for `docker run -d`. */
export interface RunOptions {
  image: string;
  name: string;
  labels: Record<string, string>;
  env?: Record<string, string>;
  network?: string;
  networkAlias?: string;
  /** Map container port -> host port. Use `0` to let Docker pick a free port. */
  publish?: Array<{ container: number; host?: number; bindIp?: string }>;
  /** Map volume name -> container mount path. */
  volumes?: Record<string, string>;
  /** Extra raw arguments appended before the image (e.g. `--health-cmd`). */
  extraArgs?: string[];
  /** Command + args passed to the container after the image. */
  command?: string[];
}

const DEFAULT_OPTS: Options = { stdout: 'pipe', stderr: 'pipe' };

/** Run an arbitrary `docker` subcommand. Throws on non-zero exit. */
export function docker(args: string[], opts: Options = {}): Promise<Result> {
  return execa('docker', args, { ...DEFAULT_OPTS, ...opts });
}

/** `docker run -d ...` → returns the started container id. */
export async function run(options: RunOptions): Promise<string> {
  const args = ['run', '-d', '--name', options.name, ...labelArgs(options.labels)];

  if (options.network) {
    args.push('--network', options.network);
    if (options.networkAlias) {
      args.push('--network-alias', options.networkAlias);
    }
  }
  for (const [key, value] of Object.entries(options.env ?? {})) {
    args.push('--env', `${key}=${value}`);
  }
  for (const p of options.publish ?? []) {
    const hostPart = p.host === undefined ? '' : String(p.host);
    const bind = p.bindIp ? `${p.bindIp}:` : '';
    args.push('--publish', `${bind}${hostPart}:${p.container}`);
  }
  for (const [volume, mount] of Object.entries(options.volumes ?? {})) {
    args.push('--volume', `${volume}:${mount}`);
  }
  args.push(...(options.extraArgs ?? []));
  args.push(options.image);
  args.push(...(options.command ?? []));

  const { stdout } = await docker(args);
  return String(stdout).trim();
}

/** `docker exec <container> <cmd...>`. Does not throw on non-zero by default. */
export function exec(
  container: string,
  command: string[],
  opts: { reject?: boolean } = {},
): Promise<Result> {
  return docker(['exec', container, ...command], { reject: opts.reject ?? false });
}

/** `docker stop` (graceful, with timeout in seconds). */
export async function stop(container: string, timeoutSecs = 10): Promise<void> {
  await docker(['stop', '--time', String(timeoutSecs), container], { reject: false });
}

/** `docker start`. */
export async function start(container: string): Promise<void> {
  await docker(['start', container]);
}

/** `docker rm -f` (never throws — best-effort cleanup). */
export async function rm(container: string): Promise<void> {
  await docker(['rm', '-f', '--volumes=false', container], { reject: false });
}

/** `docker pull`. */
export async function pull(image: string): Promise<void> {
  await docker(['pull', image]);
}

/** `docker image inspect` → true if the image exists locally. */
export async function imageExists(image: string): Promise<boolean> {
  const result = await docker(['image', 'inspect', image], { reject: false });
  return result.exitCode === 0;
}

/** `docker logs <container>` → combined stdout+stderr text. */
export async function logs(container: string): Promise<string> {
  const result = await docker(['logs', container], { reject: false, all: true });
  return String(result.all ?? `${result.stdout}\n${result.stderr}`);
}

/** Look up the host port mapped to a container's published port. */
export async function hostPort(container: string, containerPort: number): Promise<number> {
  const { stdout } = await docker(['port', container, String(containerPort)]);
  // Output like `127.0.0.1:54321` (one line per binding). Take the last field.
  const line = String(stdout).trim().split('\n')[0] ?? '';
  const port = Number.parseInt(line.slice(line.lastIndexOf(':') + 1), 10);
  if (!Number.isInteger(port) || port <= 0) {
    throw new Error(`could not parse host port for ${container}:${containerPort} from "${stdout}"`);
  }
  return port;
}

export interface NetworkApi {
  create(name: string, labels: Record<string, string>): Promise<void>;
  rm(name: string): Promise<void>;
}

export const network: NetworkApi = {
  async create(name, labels) {
    await docker(['network', 'create', ...labelArgs(labels), name]);
  },
  async rm(name) {
    await docker(['network', 'rm', name], { reject: false });
  },
};

export interface VolumeApi {
  create(name: string, labels: Record<string, string>): Promise<void>;
  rm(name: string): Promise<void>;
}

export const volume: VolumeApi = {
  async create(name, labels) {
    await docker(['volume', 'create', ...labelArgs(labels), name]);
  },
  async rm(name) {
    await docker(['volume', 'rm', '--force', name], { reject: false });
  },
};

/** List resource ids/names of a kind carrying `label` (e.g. `com.takuto.e2e.run=r1`). */
export async function listByLabel(
  kind: 'container' | 'network' | 'volume',
  label: string,
): Promise<string[]> {
  const base =
    kind === 'container'
      ? ['ps', '-aq']
      : kind === 'network'
        ? ['network', 'ls', '-q']
        : ['volume', 'ls', '-q'];
  const { stdout } = await docker([...base, '--filter', `label=${label}`], { reject: false });
  return String(stdout)
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean);
}

/** Remove every container/network/volume carrying `label`. Best-effort. */
export async function removeByLabel(label: string): Promise<void> {
  const containers = await listByLabel('container', label);
  for (const id of containers) {
    await rm(id);
  }
  const networks = await listByLabel('network', label);
  for (const id of networks) {
    await network.rm(id);
  }
  const volumes = await listByLabel('volume', label);
  for (const id of volumes) {
    await volume.rm(id);
  }
}

import { rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execa } from 'execa';
import * as docker from './cli.js';

/**
 * Part-B (workflow) infrastructure helpers: a Docker-in-Docker sidecar plus a
 * mock LM Studio that real `opencode` workers talk to.
 *
 * Networking constraint (verified): workers are spawned by the DinD daemon with
 * NO `--network` flag, so they attach to DinD's default bridge, which has no
 * name-based DNS. A worker therefore cannot resolve a sibling container by name
 * — only by IP. So the mock runs INSIDE the DinD daemon and `opencode`'s
 * `base_url` points at the mock's bridge IP (not a hostname). See
 * `seedDindMock` / `mockContainerIp`.
 */

/** Port the mock LM Studio listens on inside the DinD network. */
export const MOCK_PORT = 1234;

/**
 * Minimal OpenAI-compatible mock. Serves non-streaming
 * `POST /v1/chat/completions` with a canned assistant completion — all
 * `opencode run` needs to finish a step with exit 0 (a `type:"text"` event,
 * no `type:"error"`). Also answers `GET /v1/models` benignly. Logs each request
 * line to stdout so a spec can assert the worker reached it via
 * `docker exec <dind> docker logs <mock>`. Authored with NO single quotes so it
 * survives being passed as one `node -e` argv element.
 */
export const MOCK_SERVER_JS = [
  'const http=require("http");',
  'const PORT=1234;',
  'let hits=0;',
  'const s=http.createServer((req,res)=>{',
  '  let body="";',
  '  req.on("data",c=>{body+=c;});',
  '  req.on("end",()=>{',
  '    const u=req.url||"";',
  '    if(req.method==="POST"&&u.indexOf("/chat/completions")>=0){',
  '      hits++;',
  '      let model="mock-model";',
  '      try{const j=JSON.parse(body||"{}");if(j&&j.model)model=j.model;}catch(e){}',
  '      console.log("MOCK_HIT "+hits+" POST "+u+" model="+model);',
  '      const out={id:"chatcmpl-mock",object:"chat.completion",created:0,model:model,',
  '        choices:[{index:0,message:{role:"assistant",content:"Done."},finish_reason:"stop"}],',
  '        usage:{prompt_tokens:1,completion_tokens:1,total_tokens:2}};',
  '      res.writeHead(200,{"Content-Type":"application/json"});',
  '      res.end(JSON.stringify(out));',
  '      return;',
  '    }',
  '    if(req.method==="GET"&&u.indexOf("/models")>=0){',
  '      res.writeHead(200,{"Content-Type":"application/json"});',
  '      res.end(JSON.stringify({object:"list",data:[{id:"mock-model",object:"model"}]}));',
  '      return;',
  '    }',
  '    res.writeHead(200,{"Content-Type":"application/json"});',
  '    res.end("{}");',
  '  });',
  '});',
  's.listen(PORT,"0.0.0.0",()=>{console.log("MOCK_READY listening on "+PORT);});',
].join('\n');

/** Run a `docker` subcommand against the DinD daemon (via `docker exec`). */
export async function dindDocker(
  dindName: string,
  args: string[],
): Promise<{ stdout: string; exitCode: number }> {
  const res = await docker.exec(dindName, ['docker', ...args], { reject: false });
  return { stdout: String(res.stdout ?? '').trim(), exitCode: res.exitCode ?? -1 };
}

/**
 * Make the built e2e image available to the DinD daemon's image store so worker
 * discovery (`runner.rs::discover_worker_image`) can launch workers from it.
 * Transfers via a temp tarball (`save -o` → `docker cp` → `load -i`) rather than
 * a `save | load` pipe: piping the multi-GB stream through a JS subprocess
 * boundary corrupts the binary tar (`invalid tar header`), so the file path is
 * the reliable route.
 */
export async function loadWorkerImageIntoDind(
  dindName: string,
  image: string,
  log: (msg: string) => void,
): Promise<void> {
  log(`Loading worker image ${image} into DinD ${dindName}…`);
  const tar = join(tmpdir(), `takuto-e2e-worker-${dindName}.tar`);
  const inDind = '/worker-image.tar';
  try {
    await execa('docker', ['save', '-o', tar, image]);
    await execa('docker', ['cp', tar, `${dindName}:${inDind}`]);
    const load = await dindDocker(dindName, ['load', '-i', inDind]);
    if (load.exitCode !== 0) {
      throw new Error(`docker load in DinD failed: ${load.stdout}`);
    }
    // Drop the ~2.6 GB tar copy inside the DinD container right after load.
    // Teardown reclaims it eventually, but keeping it for the whole run
    // inflates each stack's live disk footprint (a hard crash skips teardown).
    await execa('docker', ['exec', dindName, 'rm', '-f', inDind]).catch(() => undefined);
  } finally {
    await rm(tar, { force: true }).catch(() => undefined);
  }
  log(`Worker image ${image} present in DinD`);
}

/**
 * Start the mock LM Studio inside the DinD daemon (so it shares the workers'
 * bridge) and return its bridge IP. Reuses `image` (which already ships Node) so
 * no extra image pull is needed inside DinD.
 */
export async function startDindMock(
  dindName: string,
  mockName: string,
  image: string,
  log: (msg: string) => void,
): Promise<string> {
  await dindDocker(dindName, ['rm', '-f', mockName]);
  // `--entrypoint node` bypasses the image's entrypoint.sh (which expects
  // takuto-server args); without it the `node -e …` command is swallowed and the
  // mock never listens.
  const run = await dindDocker(dindName, [
    'run',
    '-d',
    '--name',
    mockName,
    '--entrypoint',
    'node',
    image,
    '-e',
    MOCK_SERVER_JS,
  ]);
  if (run.exitCode !== 0) {
    throw new Error(`failed to start mock LM Studio in DinD: ${run.stdout}`);
  }
  // Wait until the server logs that it is listening — proves it is actually
  // serving (and didn't crash on startup) before the stack is declared ready.
  const deadline = Date.now() + 30_000;
  for (;;) {
    const logs = await dindDocker(dindName, ['logs', mockName]);
    if (logs.stdout.includes('MOCK_READY')) {
      break;
    }
    const state = await dindDocker(dindName, ['inspect', '-f', '{{.State.Running}}', mockName]);
    if (state.stdout.trim() !== 'true') {
      throw new Error(`mock LM Studio exited during startup; logs:\n${logs.stdout}`);
    }
    if (Date.now() >= deadline) {
      throw new Error(`mock LM Studio did not become ready in 30s; logs:\n${logs.stdout}`);
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  const ip = await mockContainerIp(dindName, mockName);
  log(`Mock LM Studio running in DinD at ${ip}:${MOCK_PORT}`);
  return ip;
}

/** Resolve the mock container's bridge IP inside the DinD daemon. */
export async function mockContainerIp(dindName: string, mockName: string): Promise<string> {
  const res = await dindDocker(dindName, [
    'inspect',
    '-f',
    '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}',
    mockName,
  ]);
  const ip = res.stdout.trim();
  if (!/^\d+\.\d+\.\d+\.\d+$/.test(ip)) {
    throw new Error(`could not resolve mock LM Studio IP (got "${ip}")`);
  }
  return ip;
}

/**
 * Seed a fixture git repo into the shared `workspaces` volume so startup
 * reconciliation (`repo_reconcile.rs`, scans `/workspaces`) auto-registers it.
 * Copies the host fixture (minus `node_modules`/`.git`), `git init -b main`,
 * an initial commit, and chowns the tree to the runtime uid (999) so the server
 * can create worktrees under it. Returns the in-container repo path.
 */
export async function seedFixtureRepo(opts: {
  image: string;
  workspacesVolume: string;
  fixtureHostPath: string;
  repoName: string;
  takutoUid: number;
  labels: Record<string, string>;
  log: (msg: string) => void;
}): Promise<string> {
  const { image, workspacesVolume, fixtureHostPath, repoName, takutoUid, labels, log } = opts;
  const dest = `/workspaces/${repoName}`;
  const script = [
    'set -eu',
    `rm -rf ${dest}`,
    `mkdir -p ${dest}`,
    `cp -a /fixture/. ${dest}/`,
    `rm -rf ${dest}/node_modules ${dest}/.git`,
    `cd ${dest}`,
    'git init -b main -q',
    'git config user.email takuto-e2e@example.com',
    'git config user.name "Takuto E2E"',
    'git add -A',
    'git -c commit.gpgsign=false commit -q -m "fixture: initial commit"',
    // Worktree bootstrap fetches the base branch from a remote
    // (`git fetch origin main`, then branches from `origin/main`) and HARD-FAILS
    // without one. With no GitHub, point `origin` at the repo's own on-disk path
    // (reachable at runtime via the shared /workspaces mount) so the fetch and
    // `origin/main` resolve for a purely local repo.
    `git remote add origin ${dest}`,
    'git fetch -q origin main',
    `chown -R ${takutoUid}:${takutoUid} ${dest}`,
  ].join(' && ');
  log(`Seeding fixture repo into ${workspacesVolume} at ${dest}…`);
  await execa('docker', [
    'run',
    '--rm',
    ...Object.entries(labels).flatMap(([k, v]) => ['--label', `${k}=${v}`]),
    '-v',
    `${workspacesVolume}:/workspaces`,
    '-v',
    `${fixtureHostPath}:/fixture:ro`,
    '--entrypoint',
    'bash',
    image,
    '-c',
    script,
  ]);
  log(`Fixture repo seeded at ${dest}`);
  return dest;
}

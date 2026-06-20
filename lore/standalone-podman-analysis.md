# Analysis: standalone Takuto + Docker/Podman (no DinD)

Status: **analysis only — not started.** Captures the evaluation of running Takuto
as a standalone host app that (a) verifies + installs dependencies on startup with
user confirmation, and (b) uses host **docker or podman** (not Docker-in-Docker)
to spawn item / command / terminal / IDE containers.

## Verdict
- **Podman / host-docker (no DinD): moderate, mostly mechanical.** A non-DinD code
  path already exists but is gated shut. Real costs: the hardcoded `docker` binary
  (~50 sites) and rootless-Podman user namespaces.
- **Standalone binary + interactive dep-installer: separate, larger but additive.**
  Mostly packaging + a new host-prerequisites flow; the workflow engine and the
  container model (items still run in the worker image) are unchanged.

## Already in place (half-built non-DinD path)
`is_dind_mode()` = `DOCKER_HOST` is set (`container/mod.rs:121`). Non-DinD already:
- skips `--network=host`, publishes per-port (`workspace.rs:164`);
- binds `127.0.0.1:host:container` (`editor/urls.rs:95`);
- `dind_paths.rs` translation is a **no-op** unless `DOCKER_HOST=tcp://…`;
- socat port-forwarding runs **inside** the container (`port_scanner.rs`), so the
  editor/terminal dynamic-port proxy works both ways.
**Blocker:** `runner.rs:88` hard-fails when `DOCKER_HOST` is unset ("DinD is
required") — a few lines, but it shuts the local path.

## Workstream 1 — Docker OR Podman, no DinD
- **Container binary hardcoded `"docker"` in ~50 call sites** (runner.rs, workspace.rs,
  terminal.rs, run_command.rs, port_scanner.rs, editor/*, reap.rs). No seam for the
  binary name (the `ContainerRuntime`/`ContainerRunner`/`ContainerSpawner` traits abstract
  process-spawning, not the CLI). **Medium, mechanical:** add a `container_cli()` resolver
  (config/env `TAKUTO_CONTAINER_RUNTIME=docker|podman`) and replace every
  `Command::new("docker")`.
- **Un-gate non-DinD** (`runner.rs::is_available`) to accept a local socket. **Low.**
- **Flags** (`--network=host`, `--cap-add=NET_ADMIN`, `--user`, `--entrypoint ""`, `-p`,
  `-v`, `--label`) are Podman-compatible. **Low.**
- **Reverse proxy → editor/terminal:** confirm the proxy targets the published
  `127.0.0.1:hostport` in non-DinD (vs DinD shared-netns localhost). **Low–Med.**
- **Rootless Podman user namespaces (sharp risk):** `--user takuto:takuto` + bind-mounted
  worktree / `.git` / `/shared-auth` dirs behave differently under UID remapping (already
  caused `git worktree remove` perms issues on Docker). Needs `:U` / `--userns=keep-id`
  + testing. **Med–High.**
- Drop DinD compose / `network_mode: service:dind`. **Low.**

## Workstream 2 — Standalone binary (not image)
Container model unchanged (items/commands/IDE/terminal run in worker-image containers
via host docker/podman). The image currently provides, host must supply:
- **Orchestrator on PATH:** `git`, `gh`, `docker`/`podman`, `mise`, `jq`, `socat`
  (iptables egress is container-only — droppable on host).
- **Worker image** must exist on the host engine (pull `TAKUTO_REGISTRY_IMAGE`). Unchanged.
- **Volumes → host dirs:** `/workspace`, `/workspaces`, `/shared-auth/*`, `/opt/takuto-tools`,
  data_dir. `resolve_data_dir()` already falls back to `$HOME/.takuto`; sqlite/config/
  master-key already host-friendly.
- **entrypoint duties:** chowns/setpriv (N/A on host), provisioning + `agent_install`
  (already CLI+server flows), preflight/`SystemStatus` (portable), DinD poll (N/A).
**Medium**, additive.

## Workstream 3 — Verify + install deps on startup, with confirmation
- **Reuse:** `collect_system_status()` (typed warnings) + `agent_install` `ProgressSink`
  / `GET /api/system/dependencies` overlay are the pattern.
- **Net-new:** a `takuto doctor`/bootstrap that detects `docker`/`podman`, `git`, `gh`,
  `mise` (via `--version`/`which`), and on miss prints remediation or prompts y/n then
  installs what it can (cross-distro system-package auto-install is unreliable; install via
  `mise`/known binaries, guide for the OS package manager). CLI prompt is trivial; web flow
  reuses the overlay + a confirm modal. **Medium**, additive.

## Recommended phasing
1. Container-runtime seam (`container_cli()` + config) — unlocks Podman *and* local Docker.
2. Un-gate non-DinD + verify editor/terminal proxy on loopback.
3. Rootless-Podman userns hardening (the real testing cost).
4. Standalone packaging (binary + host-path defaults + drop container-only entrypoint steps).
5. Host deps doctor + confirm/install (reuse `SystemStatus` + `ProgressSink`).

## Key files
- Runtime/binary: `container/runner.rs`, `container/runtime.rs`, `container/docker_args.rs`,
  `container/workspace.rs`, `container/terminal.rs`, `container/run_command.rs`,
  `container/port_scanner.rs`, `container/editor/*`, `container/reap.rs`,
  `takuto-web/src/container_spawner.rs`.
- DinD specifics: `container/dind_paths.rs`, `container/secrets_bundle.rs`,
  `container/editor/urls.rs`, `docker-compose.dind.yml`, `Makefile` (`DIND`).
- Deps/host: `Dockerfile`, `docker/entrypoint.sh`, `docker/egress-rules.sh`,
  `agent_install.rs`, `dependency_status.rs`, `docker_hooks/status*.rs`,
  `workflow/snapshot.rs::resolve_data_dir`, `auth/master_key.rs`.

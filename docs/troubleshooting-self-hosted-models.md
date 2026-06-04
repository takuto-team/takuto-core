# Troubleshooting self-hosted models (LM Studio / Ollama) on macOS

This guide is for operators pointing the OpenCode provider at a model
server running on the same Mac as Docker Desktop (LM Studio, Ollama,
vLLM, etc.). It captures the exact failure modes we hit when wiring
up LM Studio + Docker Desktop, with the diagnostic commands that
isolate each one, so a future operator does not have to re-derive
them.

> **All settings below are editable from Configuration → AI Settings
> in the dashboard.** There is no need to edit `config.toml` by hand.

---

## 1. The model identifier must include the provider prefix

OpenCode's `-m` flag expects `<providerId>/<modelId>`. The provider
in Maestro's generated `opencode.json` is always named
`self_hosted`, so the **Model** field in AI Settings must look like:

```
self_hosted/<model-id-as-reported-by-the-server>
```

Examples that work with LM Studio:

| LM Studio model identifier (under "API Usage") | Set in Maestro |
|---|---|
| `qwen/qwen2.5-vl-7b` | `self_hosted/qwen/qwen2.5-vl-7b` |
| `lmstudio-community/Llama-3.1-8B-Instruct-GGUF` | `self_hosted/lmstudio-community/Llama-3.1-8B-Instruct-GGUF` |

### How to confirm this is the problem

The console output on the work-item card shows:

```
Performing one time database migration, may take a few minutes...
sqlite-migration:done
Database migration complete.
OpenCode error: unknown error
```

Inside the maestro container, opencode's stderr is more verbose:

```bash
docker exec <maestro-container> sh -c '
  mkdir -p /tmp/oc && cat > /tmp/oc/opencode.json <<EOF
  {
    "\$schema": "https://opencode.ai/config.json",
    "provider": {
      "self_hosted": {
        "npm": "@ai-sdk/openai-compatible",
        "name": "Self-hosted (Maestro)",
        "options": {"baseURL": "http://host.docker.internal:1234/v1", "apiKey": "lm-studio"},
        "models": {"qwen/qwen2.5-vl-7b": {}}
      }
    }
  }
EOF
  mkdir -p /tmp/oc/.config/opencode
  cp /tmp/oc/opencode.json /tmp/oc/.config/opencode/opencode.json
  HOME=/tmp/oc XDG_CONFIG_HOME=/tmp/oc/.config opencode run \
    -m qwen/qwen2.5-vl-7b --print-logs --log-level WARN "hi" 2>&1 | head -20
'
```

A wrong prefix surfaces as:

```
ERROR ... cause=ProviderModelNotFoundError ...
{"providerID":"qwen","modelID":"qwen2.5-vl-7b","suggestions":[],"_tag":"ProviderModelNotFoundError"}
Model not found: qwen/qwen2.5-vl-7b.
```

---

## 2. `allow_shared_default` must be true when no user bearer is saved

Self-hosted servers (LM Studio, Ollama) generally do not require a
real API key. Maestro will inject the placeholder `"lm-studio"` as the
bearer **only when the per-workflow secrets bundle is built**. The
bundle build is skipped when:

- the user has not saved an OpenCode credential, AND
- `agent.providers.opencode.allow_shared_default` is `false`.

When the bundle is skipped, no `opencode.json` gets bind-mounted into
the worker, opencode starts with no provider config, and exits with
code 1 right after its first-run database migration — which the
dashboard surfaces as `OpenCode error: unknown error`.

**Fix:** in Configuration → AI Settings → OpenCode, tick
**"Allow shared default token"**.

### How to confirm this is the problem

In the maestro logs, look for:

```
build worker secrets bundle failed — falling back to legacy PASSTHROUGH_ENV path
error: provider_credential_missing: user … has no opencode credential
       and allow_shared_default = false
```

When the toggle is on, the next attempt logs `Worker secrets bundle
attached … provider=opencode` instead.

---

## 3. LM Studio must accept connections from the Docker subnet

Set both of these on the Mac running LM Studio:

- **LM Studio → Developer (sidebar) → Local Server →
  "Serve on Local Network" ON.** Without it LM Studio only binds to
  `127.0.0.1` and no container can reach it.
- macOS Application Firewall: either off, or with LM Studio
  explicitly allowed (`/usr/libexec/ApplicationFirewall/socketfilterfw
  --listapps | grep -i lm`).

### How to confirm this is the problem

On the Mac:

```bash
lsof -nP -iTCP -sTCP:LISTEN | grep 1234
# Expected: LM\x20Stu  ... TCP *:1234 (LISTEN)   ← bound to all interfaces
# If it shows 127.0.0.1:1234, "Serve on Local Network" is OFF.
```

---

## 4. Docker Desktop networkType matters: gVisor breaks `host.docker.internal` on some Macs

This is the failure mode that bit us last and the hardest to
diagnose, because **all of the configuration above can be correct**
and connections still time out.

Docker Desktop 4.34+ defaults the macOS VM's network stack to
gVisor:

```bash
ps ax | grep com.docker.virtualization | grep -o 'networkType [a-z]*'
# Output we hit: networkType gvisor
```

With gVisor, traffic from containers to **private** IPs on the Mac's
LAN — including `host.docker.internal`, which resolves inside the
container to `192.168.65.254` (Docker Desktop's VM gateway) — can
silently time out, while traffic to **public** IPs (e.g. `1.1.1.1`)
works fine.

This is a Docker-side bug surfaced on some Mac network setups (most
commonly: multi-interface laptops, machines with several active VPN
`utun*` tunnels, machines with non-default routes set by corp
profiles).

### How to confirm this is the problem

```bash
# 1. LM Studio is listening on all interfaces.
lsof -nP -iTCP -sTCP:LISTEN | grep 1234
# *:1234 (LISTEN)  ← good

# 2. macOS firewall isn't blocking it.
/usr/libexec/ApplicationFirewall/socketfilterfw --getglobalstate
# Firewall is disabled. (State = 0)

# 3. Public internet works from a container.
docker exec <maestro-container> sh -c 'wget -q -O - -T 3 https://1.1.1.1 | head -3'
# (prints HTML)

# 4. host.docker.internal resolves but connections time out.
docker exec <maestro-container> sh -c 'getent ahosts host.docker.internal | head -1'
# 192.168.65.254  STREAM host.docker.internal

docker exec <maestro-container> sh -c 'curl -v -m 3 http://host.docker.internal:1234/v1/models 2>&1 | tail -5'
#   Trying 192.168.65.254:1234...
# * Connection timed out after 3001 milliseconds

# 5. Even the Mac's LAN IP times out (gVisor doesn't route private subnets
#    that aren't on Docker Desktop's vmnet).
docker exec <maestro-container> sh -c 'curl -v -m 3 http://<mac-LAN-IP>:1234/v1/models 2>&1 | tail -5'
# * Connection timed out
```

If all five of those check out, the issue is the gVisor stack.

### Fix: the `lm-bridge` socat sidecar

Docker Desktop 4.34+ defaulted to gVisor and we have empirically
confirmed that the **`vpnkit` switch no longer takes effect on
4.70** — the `defaults write com.docker.docker NetworkMode` knob is
read but the VM is launched with `--networkType gvisor` regardless.
Restarting Docker Desktop / disconnecting VPNs do not change this.

The only Docker Desktop quirk that we can rely on is this: **the
default `bridge` network IS forwarded to the host correctly** under
gVisor; user-defined networks and DinD-nested networks are not.

So the supported workaround in Maestro is a tiny socat container
attached to BOTH networks:

- the default `bridge` network — so it can actually reach
  `host.docker.internal:<port>` on the Mac;
- the Maestro compose network — so DinD-nested workers can route to
  it (DinD's outbound NAT into the compose network was verified
  end-to-end).

It ships as `docker-compose.lm-bridge.yml` plus a single Makefile
flag:

```bash
make start BACKEND=postgres LM_BRIDGE=1
```

That brings up `maestro-lm-bridge` (image `alpine/socat:1.8.0.0`)
with a **pinned IP of `172.20.0.250`** on the Maestro compose
network. It forwards TCP/1234 → `host.docker.internal:${LM_HOST_PORT:-1234}`.

Then in **Configuration → AI Settings → OpenCode**, set the
**Base URL** to:

```
http://172.20.0.250:1234/v1
```

instead of `http://host.docker.internal:1234/v1`. Everything else in
this guide (model prefix, `allow_shared_default`, LM Studio bind)
stays the same.

If your LM Studio listens on a non-default port, set the env var
before `make start`:

```bash
LM_HOST_PORT=11434 make start BACKEND=postgres LM_BRIDGE=1
```

(`11434` is the Ollama default.)

### One-line smoke test once `LM_BRIDGE=1` is up

```bash
docker exec maestro-core-dind-1 docker run --rm --entrypoint /bin/bash \
  maestro:latest -c \
  'exec 3<>/dev/tcp/172.20.0.250/1234;
   printf "GET /v1/models HTTP/1.0\r\nHost: x\r\n\r\n" >&3;
   timeout 5 cat <&3 | head -5'
```

A `200 OK` followed by JSON means the worker path is healthy. From
there, any flow that goes through the OpenCode session will reach
LM Studio cleanly.

---

## TL;DR checklist for "OpenCode error: unknown error"

When the dashboard shows the cryptic `OpenCode error: unknown error`,
walk these in order — each one is a real failure mode we've hit:

1. **Model field** in Configuration → AI Settings → OpenCode starts with
   `self_hosted/`.
2. **Allow shared default token** is ticked (unless you've saved a
   per-user OpenCode bearer).
3. **Base URL** is `http://host.docker.internal:<port>/v1` (the
   trailing `/v1` is required for OpenAI-compat clients).
4. LM Studio (or your model server) has **"Serve on Local Network"**
   on — confirm with `lsof -nP -iTCP -sTCP:LISTEN | grep <port>` and
   look for `*:<port>` not `127.0.0.1:<port>`.
5. **Docker Desktop networking is healthy** — confirm with the
   one-line smoke test above. If it times out, restart Docker Desktop
   or switch to vpnkit networking.

If all five are green and the run still fails, capture the failure
inside the container with the manual `opencode run` command in §1 and
the underlying error will be in opencode's own stderr instead of
hidden behind "unknown error".

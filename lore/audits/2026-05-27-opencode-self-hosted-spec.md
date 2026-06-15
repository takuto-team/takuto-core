# Refactor spec — OpenCode as the self-hosted-only provider

Source: post-Phase-4 cleanup of the multi-AI work in `tmp/multi-agents/`. The amended architecture (`04_architecture.md §A.2`) framed OpenCode as a generic OpenAI-compatible adapter that could front Anthropic / OpenAI / OpenRouter as a "power-user" recipe, with LM Studio as the documented case. In practice the only OpenCode use case that pays for itself is **self-hosted endpoints** (LM Studio, Ollama, vLLM, private gateways): Claude / Codex / Cursor have native adapters that are strictly better when the upstream is the vendor.

This spec collapses OpenCode to that single role, removes the generic-OpenAI plumbing that points users at the wrong tool, and lands the missing `opencode.json` shim that actually makes self-hosted work end-to-end.

The hot-switch contract (`PUT /api/config/agent` flips the active provider with no takuto restart, new workflows pick it up immediately, in-flight runs finish on their `auth_pin`) is preserved verbatim.

## 1. Problem

Today's OpenCode wiring is half-built and mis-framed:

1. **`OPENCODE_PROVIDER_BASE_URL` is exported but ignored.** `crates/takuto-core/src/auth/bundle/assembler.rs:144` sets the env var from `[agent.providers.opencode].base_url`, but OpenCode reads providers from `~/.config/opencode/opencode.json` (or `~/.local/share/opencode/auth.json`) — it has no env-var fallback. `crates/takuto-core/src/opencode/mod.rs:19-30` documents this explicitly: "until the integration agent emits a matching `auth.json` (or `opencode.json`), the CLI will likely report 'no provider configured'."
2. **The per-user OpenCode secret is sourced into `ANTHROPIC_API_KEY`.** `docker/worker-entrypoint.sh:52-60` and `crates/takuto-core/src/container/wrap_command.rs:93-96` map `/run/takuto-secrets/opencode` → `ANTHROPIC_API_KEY`. That only makes sense in the "OpenCode-as-generic-OpenAI" branch and is actively misleading for the LM Studio case (LM Studio doesn't take an Anthropic key; users who do want to talk to Anthropic should pick the Claude provider, which has its own dedicated adapter).
3. **Admin can save `provider = "opencode"` with empty `base_url`.** There's no validator guard; the workflow will fail at first agent step with a generic "no provider configured" message instead of a 400 at save time. There is no sensible default — every OpenCode deployment is per-customer self-hosted.
4. **UI copy on the OpenCode credentials tab says "API key"** with no hint about what endpoint it's pointing at. Users assume it's an Anthropic / OpenAI key.

## 2. Changes

Four small edits — three deletions and one new module.

### 2.1 Drop the `ANTHROPIC_API_KEY` mapping for OpenCode

| File | Edit |
|---|---|
| `docker/worker-entrypoint.sh:52-60` | Delete the `if [ -f /run/takuto-secrets/opencode ]` block in its current form (mapping to `ANTHROPIC_API_KEY`). |
| `crates/takuto-core/src/container/wrap_command.rs:93-96` | Delete the matching lines from `BUNDLE_SOURCING_SH`. |
| `crates/takuto-core/src/container/wrap_command.rs:176-188` | Update the drift-detection test pair list so `("/run/takuto-secrets/opencode", "ANTHROPIC_API_KEY")` is removed; the test continues to enforce parity for the remaining four (`claude`, `cursor`, `codex`, `gh`). |

The bundle still **writes** `/run/takuto-secrets/opencode` (assembler unchanged) — but it's read by the new init-shim (§2.3), not by the entrypoint as an env-var source.

### 2.2 Drop `OPENCODE_PROVIDER_BASE_URL` plumbing

| File | Edit |
|---|---|
| `crates/takuto-core/src/auth/bundle/assembler.rs:138-145` | Remove the `OPENCODE_PROVIDER_BASE_URL` env var emission. The other three providers (`ANTHROPIC_BASE_URL`, `OPENAI_BASE_URL`, Cursor's no-op) keep their existing wiring because their CLIs do read env-var base URLs. |
| `crates/takuto-core/src/auth/bundle/mod.rs` test fixtures | Remove the `OPENCODE_PROVIDER_BASE_URL` assertion from `build_emits_*` tests; add a new assertion (under §2.3) that the init-shim renders the URL into JSON. |
| `AGENTS.md` lines 477, 478 | Strike `OPENCODE_PROVIDER_BASE_URL` from the bundle env-var list; describe the new shim in its place. |

### 2.3 New: `opencode.json` init-shim

A new module materialises `~/.config/opencode/opencode.json` inside the worker container at workflow start, with the admin's `base_url` + `model` and (optionally) the user's bearer token.

- **New file**: `crates/takuto-core/src/auth/bundle/opencode_config.rs` (~120 LOC + tests).
- **Public surface**: `pub(super) fn write_opencode_config(dir: &Path, base_url: &str, model: &str, bearer: Option<&[u8]>) -> Result<PathBuf, AuthError>`.
- **Output shape** (one provider `self_hosted`, one model with id == admin-config value):

```json
{
  "$schema": "https://opencode.ai/config.json",
  "provider": {
    "self_hosted": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "Self-hosted",
      "options": { "baseURL": "<admin base_url>", "apiKey": "<user bearer or 'lm-studio'>" },
      "models": { "<admin model id>": {} }
    }
  }
}
```

- **`apiKey` rules**:
  - When the user's saved secret is non-empty → use it verbatim.
  - When the user has no row OR the row's plaintext is empty → use the literal string `"lm-studio"`. (LM Studio's OpenAI-compat server requires a non-empty placeholder; this is the documented dummy from `lmstudio.ai/docs/developer/openai-compat`.)
- **Mount path**: the bundle's existing `TempDir` (under `${TAKUTO_DATA_DIR}/runtime/opencode/users/<uid>/<wid>/`) gains a `config/` subdir; the worker bind-mounts it at `/home/takuto/.config/opencode:ro`. RAII cleanup is identical to the existing secrets-bundle pattern (`auth/bundle/tempdir.rs`).
- **Model resolution**: the shim reads `AgentConfig::effective_opencode_model()` — a new helper on `AgentConfig` mirroring `effective_claude_model()` (`config/agent.rs`). Resolution: `[agent.providers.opencode].model` → return as-is. Empty → reject (caught by §2.4 validator before we get here, but the shim returns `AuthError::OpenCodeModelMissing` defensively).
- **Wiring in**: `auth/bundle/assembler.rs::build` calls the shim when `provider == OpenCode`. The returned config-dir path is held on `WorkerSecretsBundle` and bind-mounted by `container/runner.rs::with_secrets_bundle`.
- **Tests** (in the new file's `#[cfg(test)] mod tests`):
  - happy path — admin `base_url + model`, user bearer → JSON contains all three.
  - empty bearer → JSON contains `"apiKey": "lm-studio"`.
  - model empty → returns `Err(OpenCodeModelMissing)`.
  - serialised JSON validates against the OpenCode schema (parsed back with `serde_json::Value` + key existence asserts; we don't fetch the live schema in tests).

### 2.4 Validator: reject empty `base_url` when `provider = "opencode"`

| File | Edit |
|---|---|
| `crates/takuto-core/src/config/agent.rs::validate` (or wherever the cross-field validator lives — currently `config::validate` walks per-provider sub-tables) | Add: `if provider == OpenCode && providers.opencode.base_url.trim().is_empty() { return Err(ConfigError::OpenCodeBaseUrlRequired) }`. |
| `crates/takuto-core/src/config/error.rs` | Add `OpenCodeBaseUrlRequired` variant (stable code `opencode_base_url_required`). |
| `crates/takuto-web/src/routes/config_agent.rs::apply_patch` | Surface the new error as a 400 with the stable code. Existing `extra_args_denied` / `provider_missing_subtable` error paths are the model. |

The validator runs on `Config::load` (boot) **and** on every `PUT /api/config/agent`, so existing on-disk configs that already have `provider = "opencode"` + empty `base_url` would fail to load. A one-paragraph entry in the changelog notes the breaking change and the fix (set `base_url`). Pre-existing deployments that picked OpenCode without `base_url` were already broken at workflow time, so no working deployment regresses.

### 2.5 UI copy on `/admin/ai` and `/me/credentials`

| File | Edit |
|---|---|
| `ui/src/pages/Onboarding/ProviderStep.tsx` (OpenCode branch of step 2) and the equivalent in `/admin/ai` | "Base URL" field placeholder: `http://lm-studio:1234/v1`. Helper text: "Endpoint for your self-hosted OpenAI-compatible model server (LM Studio, Ollama, vLLM, etc)." Disable the "Activate OpenCode" / "Save" button while base URL is empty (client-side guard; the validator from §2.4 is the source of truth). |
| `ui/src/pages/Credentials.tsx` OpenCode tab | Rename "API key" → "Bearer token (optional)". Helper text: "Leave blank for LM Studio or other unauthenticated endpoints. For authenticated self-hosted servers, paste the bearer expected by your gateway." |

## 3. Hot-switch invariant — preserved

The four cleanup items deliberately don't touch the runtime switch path:

- `PUT /api/config/agent` keeps writing through `ConfigWriter` (atomic temp+rename) and updating the live `state.config` lock. No process restart.
- `provider_changed` WS event still fires (see `routes/config_agent.rs::apply_patch`); banners and def-buttons re-render without reload.
- The `WorkerSecretsBundle` is rebuilt per workflow from the live config; the new `opencode_config_shim` runs inside that same `build` call, so config edits land on the next workflow start.
- `auth_pin` semantics unchanged: in-flight workflows finish on the credentials and provider they were pinned to (T-SWITCH-004 stays green).

One new ordering rule, enforced by §2.4: **the admin must set `base_url` before (or in the same patch as) flipping `provider` to `opencode`**. The endpoint accepts both keys in one body, so the UI submits them together; the client-side guard from §2.5 prevents the bounce.

## 4. Acceptance criteria

- [ ] `cargo build --workspace` clean; `cargo test --workspace --lib --tests` matches baseline + the new shim tests.
- [ ] `BUNDLE_SOURCING_SH` ↔ `worker-entrypoint.sh` drift-detection test passes after the `opencode` pair is removed from both halves in lockstep.
- [ ] On a deployment with `provider = "opencode"` + `base_url = "http://lm-studio:1234/v1"` + `model = "lmstudio/qwen3-coder"` and **no per-user OpenCode credential row**, a workflow starts cleanly and the worker's `/home/takuto/.config/opencode/opencode.json` contains the expected JSON with `apiKey: "lm-studio"`.
- [ ] Same setup with a per-user bearer saved → the bearer appears in `apiKey`, the saved secret file is no longer left lying around in the worker (RAII drop verified by `ls /home/takuto/.config/opencode/` after workflow teardown).
- [ ] `docker inspect <worker>` shows no `ANTHROPIC_API_KEY` env var on OpenCode workflows; the secret-leak suite (`takuto-web/tests/secret_leak.rs`) passes.
- [ ] `PUT /api/config/agent` with `provider="opencode"` and empty `[agent.providers.opencode].base_url` returns 400 `opencode_base_url_required`. With `base_url` set in the same body → 200.
- [ ] Hot-switch from `claude` to `opencode` in a single `PUT /api/config/agent` (one body containing both `provider` and the populated `[agent.providers.opencode]` sub-table) — `provider_changed` WS event observed; next workflow start picks OpenCode without restart.
- [ ] In-flight Claude workflow at the time of the switch finishes on Claude per its `auth_pin` (T-SWITCH-004 regression check).

## 5. Risks & non-goals

1. **OpenCode upstream `opencode.json` schema drift.** The shape we emit is the documented `@ai-sdk/openai-compatible` provider entry; OpenCode 1.x has been stable on this surface, but the project moves fast (blind spot B.15 in `06_qa_and_blind_spots.md`). Mitigation: the shim's JSON is built from a typed Rust struct + `serde_json::to_string_pretty`, not a format string, so a schema bump becomes a typed-error change rather than a stringly-typed silent failure. The audit-doc-noted weekly upstream-diff CI job is still a P1 follow-up.
2. **Reverting customers who depended on the broken plumbing.** Nobody can — the previous wiring never produced a working OpenCode workflow against any real backend (per `opencode/mod.rs:19-30`). So there is no production-positive behaviour to preserve; this is pure cleanup.
3. **Admin who picks OpenCode and points it at api.anthropic.com.** Technically still works via `base_url = "https://api.anthropic.com"` + a real Anthropic key as the per-user bearer. Documented as unsupported in `AGENTS.md` (the right answer is `provider = "claude"`); not blocked by the validator because we can't tell apart "weird self-hosted gateway whose URL happens to look like Anthropic's" from "user who picked the wrong provider", and over-policing the URL is worse than the misuse it prevents.
4. **No new audit suite for the shim.** The existing `takuto-core/tests/` covers `WorkerSecretsBundle` end-to-end; the shim slots into that path and inherits the coverage. A targeted integration test against a stub LM Studio container (`T-OC-005` from `06_qa_and_blind_spots.md §A.9`) was a P0 in the plan but never landed — recommended P1 follow-up alongside this work; not a blocker for the structural cleanup.
5. **GPG / signed-commit follow-ups out of scope.** Same boundary as amendment A3 in `04_architecture.md`; this spec is OpenCode-only.
6. **No change to `auth_pin` shape.** The pin still references `provider_credential_row_id` from `user_provider_credentials`. The shim reads through the existing unseal path (`auth/bundle/unseal.rs`); no schema or migration impact.

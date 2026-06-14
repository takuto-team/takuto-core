// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `sh -c` payload assembled for agent commands that bypass
//! `worker-entrypoint.sh` (`ContainerRunner::wrap_command`).
//!
//! The four inline shell snippets that the old `wrap_command` body
//! constructed in-line are lifted to named `const &str` blocks at the
//! top of this file. The [`build_sh_payload`] helper concatenates them
//! around the user's `exec …` line, with the bundle-sourcing block
//! omitted when no secrets bundle is attached.

use super::shell_escape;

/// Restore `~/.claude.json` from the most-recent backup when it is missing.
///
/// The file lives outside the shared volume and is missing in fresh
/// worker containers. Runs first so any later bundle-merge step has a
/// real file to merge into.
pub(crate) const RESTORE_SNIPPET: &str = r#"if [ ! -f "$HOME/.claude.json" ]; then b=$(ls -t "$HOME/.claude/backups/.claude.json.backup."* 2>/dev/null | head -1); [ -n "$b" ] && cp "$b" "$HOME/.claude.json"; fi"#;

/// Take ownership of npm/mise dirs.
///
/// Shared volumes start root-owned; without this `chown` the worker
/// user (uid:gid from `id -u`/`id -g`) cannot write to them. Uses
/// passwordless `sudo bash` (granted in `/etc/sudoers.d/maestro-hook-bash`).
pub(crate) const FIX_PERMS_SNIPPET: &str = r#"sudo -n bash -c 'for d in "$HOME/.npm" "$HOME/.npm-global" "$HOME/.cache/mise" "$HOME/.local/share/mise"; do [ -d "$d" ] && chown -R "$(id -u):$(id -g)" "$d"; done' 2>/dev/null || true"#;

/// Source the centralized GitHub App token file when present, so `gh`
/// and git operations use a fresh token. The file is refreshed by
/// Maestro's background service; when it does not exist (local dev /
/// no GitHub App) this is a no-op and the legacy `GH_TOKEN` passthrough
/// from `PASSTHROUGH_ENV` carries the value instead.
pub(crate) const GH_TOKEN_SNIPPET: &str = r#"[ -f "$HOME/.config/gh/gh-app-token" ] && export GH_TOKEN="${GH_TOKEN:-$(cat "$HOME/.config/gh/gh-app-token")}";"#;

/// Configure the HTTPS `github.com` git credential helper so the agent's
/// `git push` / `git fetch` authenticate non-interactively.
///
/// Agent-step workers run with `--entrypoint ""` and build their own inline
/// `sh -c`; they neither run `worker-entrypoint.sh` nor inherit the main
/// container's `~/.gitconfig`, where [`crate::github_app::configure_git_and_gh_auth`]
/// installs this helper. WITHOUT this snippet the worker's git has no
/// credential helper, so `git push` falls back to an interactive username
/// prompt and dies with `could not read Username for 'https://github.com'` —
/// which a weak model then loops on. Mirrors
/// [`crate::github_app::git_credential_helper_script`] (GH_TOKEN-first, with
/// the shared App-token file as fallback). `GH_TOKEN` is exported by
/// [`GH_TOKEN_SNIPPET`] / [`BUNDLE_SOURCING_SH`] earlier in the payload, and
/// git re-reads it from the environment at credential time. Non-fatal.
pub(crate) const GIT_CRED_HELPER_SNIPPET: &str = r#"git config --global 'credential.https://github.com.helper' '!f() { echo protocol=https; echo host=github.com; echo username=x-access-token; echo "password=${GH_TOKEN:-$(cat /home/maestro/.config/gh/gh-app-token 2>/dev/null)}"; }; f' 2>/dev/null || true;"#;

/// Shell snippet that sources every `/run/maestro-secrets/*` file into the
/// matching env var, then `rm -f`s the on-disk copy. **Single source of
/// truth** for both:
///
///   1. `worker-entrypoint.sh` — used when the worker container is spawned
///      WITH `--entrypoint /usr/local/bin/worker-entrypoint.sh` (e.g.
///      `wrap_shell_command`, `start_editor`, `start_run_command`).
///   2. `ContainerRunner::wrap_command` — used by agent invocations
///      (claude, cursor, codex, opencode) which pass no entrypoint and
///      build their own inline `sh -c`. WITHOUT this block, the bundle's
///      tmpfs files are mounted but NEVER sourced, so the agent CLI sees
///      no token and reports "Not logged in".
///
/// The snippet is self-gated on `MAESTRO_AUTH_BUNDLE=1` so it is a no-op
/// when the legacy passthrough path is active. It mirrors the env-mapping
/// of `worker-entrypoint.sh` lines 24-58 exactly; a unit test asserts the
/// snippet contains every documented (file → env) mapping so the two
/// can't drift silently.
///
/// The snippet does NOT include a trailing newline so it composes cleanly
/// inside a `;`-joined command string.
pub(crate) const BUNDLE_SOURCING_SH: &str = concat!(
    r#"if [ "${MAESTRO_AUTH_BUNDLE:-0}" = "1" ] && [ -d /run/maestro-secrets ]; then"#,
    // Observability breadcrumb. When the bundle's discriminator env var
    // is set but the bind-mounted directory has no files, the bundle's
    // host-side TempDir has dropped out from under us — almost certainly
    // because nothing held the Arc alive long enough. Emit a single
    // grep-friendly stderr line so future regressions surface in the
    // workflow / editor terminal instead of silently falling back to the
    // deployment default. Without this breadcrumb, the only symptom is
    // "claude says I'm not logged in".
    r#" __bundle_present=$(ls -A /run/maestro-secrets 2>/dev/null | wc -l);"#,
    r#" if [ "${__bundle_present:-0}" = "0" ]; then"#,
    r#" echo "[maestro-bundle] MAESTRO_AUTH_BUNDLE=1 but /run/maestro-secrets/ is empty -- secret files vanished (host TempDir dropped). Check WorkerSecretsBundle lifetime in AppState." >&2;"#,
    r#" fi;"#,
    r#" if [ -f /run/maestro-secrets/claude ]; then"#,
    r#" CLAUDE_CODE_OAUTH_TOKEN="$(cat /run/maestro-secrets/claude)";"#,
    r#" export CLAUDE_CODE_OAUTH_TOKEN;"#,
    r#" rm -f /run/maestro-secrets/claude 2>/dev/null || true;"#,
    r#" fi;"#,
    r#" if [ -f /run/maestro-secrets/cursor ]; then"#,
    r#" CURSOR_API_KEY="$(cat /run/maestro-secrets/cursor)";"#,
    r#" export CURSOR_API_KEY;"#,
    r#" rm -f /run/maestro-secrets/cursor 2>/dev/null || true;"#,
    r#" fi;"#,
    r#" if [ -f /run/maestro-secrets/codex ]; then"#,
    r#" OPENAI_API_KEY="$(cat /run/maestro-secrets/codex)";"#,
    r#" export OPENAI_API_KEY;"#,
    r#" rm -f /run/maestro-secrets/codex 2>/dev/null || true;"#,
    r#" fi;"#,
    // OpenCode self-hosted spec (lore/audits/2026-05-27-opencode-self-hosted-spec.md):
    // No /run/maestro-secrets/opencode handling — the provider config is
    // a file, not an env var. The bundle's opencode_config_dir is staged
    // read-only at /run/maestro-opencode-config and copied into the
    // user's writable ~/.config/opencode below: opencode writes a
    // .gitignore into its config dir at startup and aborts config loading
    // when that write fails, so a direct RO mount at the lookup path
    // breaks every run. The previous ANTHROPIC_API_KEY mapping was the
    // wrong-tool footgun (use the Claude provider for Anthropic) and is
    // intentionally absent.
    r#" if [ -f /run/maestro-opencode-config/opencode.json ]; then"#,
    r#" mkdir -p "$HOME/.config/opencode";"#,
    r#" cp /run/maestro-opencode-config/opencode.json "$HOME/.config/opencode/opencode.json" || true;"#,
    r#" chmod 600 "$HOME/.config/opencode/opencode.json" 2>/dev/null || true;"#,
    r#" fi;"#,
    // Claude session-state (`~/.claude.json`). The bundle ships ONLY the
    // keys the user pasted (typically just `oauthAccount` for team-plan
    // users on a custom proxy). A naive `cp` would wipe whatever the
    // legacy backups-restore step put on disk — including
    // `hasCompletedOnboarding`, `userID`, accumulated state — and Claude
    // Code checks those fields too. We do a shallow JSON merge: existing
    // keys win when bundle blob is silent on them; bundle keys
    // (oauthAccount, etc.) win when present. `jq -s '.[0] * .[1]'` is the
    // canonical jq incantation for this. jq is in the image (Dockerfile
    // line 62). When jq is somehow missing OR there's no existing
    // `.claude.json` to merge into, fall back to a plain overwrite.
    // Placed AFTER the legacy backups-restore so per-user session always
    // wins over stale state.
    r#" if [ -f /run/maestro-secrets/claude_session.json ]; then"#,
    r#" if [ -f "$HOME/.claude.json" ] && command -v jq >/dev/null 2>&1; then"#,
    r#" __mtmp=$(mktemp);"#,
    r#" if jq -s '.[0] * .[1]' "$HOME/.claude.json" /run/maestro-secrets/claude_session.json > "$__mtmp" 2>/dev/null; then"#,
    r#" mv "$__mtmp" "$HOME/.claude.json";"#,
    r#" else"#,
    r#" rm -f "$__mtmp";"#,
    r#" cp /run/maestro-secrets/claude_session.json "$HOME/.claude.json" || true;"#,
    r#" fi;"#,
    r#" else"#,
    r#" cp /run/maestro-secrets/claude_session.json "$HOME/.claude.json" || true;"#,
    r#" fi;"#,
    r#" rm -f /run/maestro-secrets/claude_session.json 2>/dev/null || true;"#,
    r#" fi;"#,
    r#" if [ -f /run/maestro-secrets/gh ]; then"#,
    r#" GH_TOKEN="$(cat /run/maestro-secrets/gh)";"#,
    r#" export GH_TOKEN;"#,
    r#" rm -f /run/maestro-secrets/gh 2>/dev/null || true;"#,
    r#" fi;"#,
    r#" fi"#,
);

/// Build the `sh -c` payload for `ContainerRunner::wrap_command`:
/// `<restore>; <fix_perms>; <gh_token> [<bundle_source>;] exec <program> <args…>`.
///
/// When `has_bundle` is `false` the bundle-sourcing block is omitted
/// entirely, keeping the legacy path's argv clean and matching the
/// pre-bundle behaviour byte-for-byte.
pub(crate) fn build_sh_payload(has_bundle: bool, program: &str, args: &[&str]) -> String {
    let mut shell_parts: Vec<String> = Vec::with_capacity(1 + args.len());
    shell_parts.push(shell_escape(program));
    for a in args {
        shell_parts.push(shell_escape(a));
    }
    let user_exec = shell_parts.join(" ");
    if has_bundle {
        format!(
            "{RESTORE_SNIPPET}; {FIX_PERMS_SNIPPET}; {GH_TOKEN_SNIPPET} {BUNDLE_SOURCING_SH}; {GIT_CRED_HELPER_SNIPPET} exec {user_exec}"
        )
    } else {
        format!(
            "{RESTORE_SNIPPET}; {FIX_PERMS_SNIPPET}; {GH_TOKEN_SNIPPET} {GIT_CRED_HELPER_SNIPPET} exec {user_exec}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundle-sourcing snippet must cover every
    /// `/run/maestro-secrets/<file>` → env-var mapping documented in
    /// `worker-entrypoint.sh` (lines 24-58). If the entrypoint adds a new
    /// provider mapping, this constant must be updated in lockstep.
    #[test]
    fn bundle_sourcing_snippet_covers_every_documented_mapping() {
        // Self-gated on the discriminator so it's a no-op when the bundle
        // is absent (worker-entrypoint.sh's pre-bundle path).
        assert!(
            BUNDLE_SOURCING_SH.contains(r#"if [ "${MAESTRO_AUTH_BUNDLE:-0}" = "1" ]"#),
            "snippet must self-gate on MAESTRO_AUTH_BUNDLE=1"
        );
        // Every file → env-var mapping from worker-entrypoint.sh.
        // OpenCode is intentionally absent — it consumes opencode.json,
        // not env vars (spec lore/audits/2026-05-27-opencode-self-hosted-spec.md).
        for (file, env_var) in [
            ("/run/maestro-secrets/claude", "CLAUDE_CODE_OAUTH_TOKEN"),
            ("/run/maestro-secrets/cursor", "CURSOR_API_KEY"),
            ("/run/maestro-secrets/codex", "OPENAI_API_KEY"),
            ("/run/maestro-secrets/gh", "GH_TOKEN"),
        ] {
            assert!(
                BUNDLE_SOURCING_SH.contains(&format!("[ -f {file} ]")),
                "snippet must source-test {file}"
            );
            assert!(
                BUNDLE_SOURCING_SH.contains(&format!("export {env_var};")),
                "snippet must export {env_var}"
            );
            assert!(
                BUNDLE_SOURCING_SH.contains(&format!("rm -f {file}")),
                "snippet must rm -f {file} after sourcing"
            );
        }
        // OpenCode spec invariant: NO opencode file/env mapping.
        assert!(
            !BUNDLE_SOURCING_SH.contains("/run/maestro-secrets/opencode"),
            "snippet must NOT source /run/maestro-secrets/opencode — \
             OpenCode reads opencode.json via the bundle's \
             opencode_config_dir mount (spec 2026-05-27)"
        );
        assert!(
            !BUNDLE_SOURCING_SH.contains("ANTHROPIC_API_KEY="),
            "OpenCode → ANTHROPIC_API_KEY mapping is intentionally dropped \
             (spec 2026-05-27 §2.1)"
        );

        // Claude session-state file uses `cp` (not source/export) because
        // it carries JSON, not shell variables. Assert the dedicated cp +
        // rm pair instead of the export pattern.
        assert!(
            BUNDLE_SOURCING_SH.contains("[ -f /run/maestro-secrets/claude_session.json ]"),
            "snippet must source-test claude_session.json"
        );
        // The snippet shallow-merges the session blob into the existing
        // $HOME/.claude.json via jq, with a `cp` fallback when jq is
        // unavailable OR the target file doesn't yet exist. Assert BOTH
        // paths are present so a regression to plain-cp gets caught.
        assert!(
            BUNDLE_SOURCING_SH.contains("jq -s '.[0] * .[1]'"),
            "snippet must merge via jq's `.[0] * .[1]` shallow-merge"
        );
        assert!(
            BUNDLE_SOURCING_SH
                .contains(r#"cp /run/maestro-secrets/claude_session.json "$HOME/.claude.json""#),
            "snippet must keep a cp fallback for the no-jq / no-existing-file case"
        );
        assert!(
            BUNDLE_SOURCING_SH.contains("rm -f /run/maestro-secrets/claude_session.json"),
            "snippet must rm -f /run/maestro-secrets/claude_session.json after merge"
        );

        // Observability breadcrumb. When MAESTRO_AUTH_BUNDLE=1 but the
        // mountpoint is empty, the snippet must emit a single grep-
        // friendly stderr line. Without this, the bundle's lifetime bugs
        // are invisible (everything silently no-ops).
        assert!(
            BUNDLE_SOURCING_SH.contains("[maestro-bundle]"),
            "snippet must carry the [maestro-bundle] stderr tag for the \
             empty-mountpoint case (observability)"
        );
        assert!(
            BUNDLE_SOURCING_SH.contains(">&2"),
            "the empty-mountpoint warning must go to stderr (not stdout)"
        );
        assert!(
            BUNDLE_SOURCING_SH.contains("WorkerSecretsBundle lifetime"),
            "warning must point at the WorkerSecretsBundle lifetime cause"
        );
    }

    /// Drift-detection. Read `docker/worker-entrypoint.sh` from
    /// disk and confirm the Rust [`BUNDLE_SOURCING_SH`] constant references
    /// the same `/run/maestro-secrets/<file>` ↔ env-var mappings the
    /// entrypoint hardcodes. If someone edits the shell script and adds a
    /// new provider, this test fails until [`BUNDLE_SOURCING_SH`] is
    /// updated in lockstep.
    #[test]
    fn bundle_sourcing_matches_worker_entrypoint_shell_script() {
        // CARGO_MANIFEST_DIR for maestro-core is crates/maestro-core; the
        // entrypoint lives at <repo>/docker/worker-entrypoint.sh.
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let script_path = manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("docker/worker-entrypoint.sh"))
            .expect("locate docker/worker-entrypoint.sh from manifest dir");
        let script = match std::fs::read_to_string(&script_path) {
            Ok(s) => s,
            Err(e) => {
                // Worktree / sparse-checkout safety: don't fail if the file
                // truly isn't present (CI uses the full repo, this guards
                // local edge cases).
                eprintln!("skip: cannot read {script_path:?}: {e}");
                return;
            }
        };
        // Each mapping the snippet must keep in sync with the script.
        // OpenCode intentionally omitted — its bearer lives in
        // opencode.json mounted via the bundle's opencode_config_dir
        // (spec lore/audits/2026-05-27-opencode-self-hosted-spec.md).
        for (file, env_var) in [
            ("/run/maestro-secrets/claude", "CLAUDE_CODE_OAUTH_TOKEN"),
            ("/run/maestro-secrets/cursor", "CURSOR_API_KEY"),
            ("/run/maestro-secrets/codex", "OPENAI_API_KEY"),
            ("/run/maestro-secrets/gh", "GH_TOKEN"),
        ] {
            assert!(
                script.contains(file),
                "drift: worker-entrypoint.sh no longer sources {file}; \
                 update BUNDLE_SOURCING_SH and this test in lockstep"
            );
            assert!(
                script.contains(&format!("export {env_var}")),
                "drift: worker-entrypoint.sh no longer exports {env_var}; \
                 update BUNDLE_SOURCING_SH and this test in lockstep"
            );
            // And the Rust snippet must mirror it.
            assert!(
                BUNDLE_SOURCING_SH.contains(file),
                "drift: BUNDLE_SOURCING_SH missing {file} (present in shell script)"
            );
            assert!(
                BUNDLE_SOURCING_SH.contains(&format!("export {env_var};")),
                "drift: BUNDLE_SOURCING_SH missing export {env_var} \
                 (present in shell script)"
            );
        }
        // OpenCode spec invariant: drift detector for the
        // "no /run/maestro-secrets/opencode env-var sourcing" rule.
        assert!(
            !script.contains("/run/maestro-secrets/opencode"),
            "drift: worker-entrypoint.sh re-introduced opencode mapping — \
             spec 2026-05-27 §2.1 deletes it (opencode reads opencode.json \
             via the bundle's opencode_config_dir mount, not env vars)"
        );
        assert!(
            !script.contains("ANTHROPIC_API_KEY="),
            "drift: worker-entrypoint.sh re-introduced ANTHROPIC_API_KEY \
             mapping — spec 2026-05-27 §2.1 deletes it (use the Claude \
             provider, not OpenCode, to talk to Anthropic)"
        );

        // The cli_state mapping doesn't use the standard
        // source + export pattern. It writes the session blob onto
        // $HOME/.claude.json via a `jq` shallow-merge (with a `cp`
        // fallback). Both the script and the Rust constant must:
        //   1. Reference the file path,
        //   2. Reference $HOME/.claude.json as the merge target,
        //   3. Carry the `jq -s '.[0] * .[1]'` invocation (so a regression
        //      to plain-cp gets caught).
        assert!(
            script.contains("/run/maestro-secrets/claude_session.json"),
            "drift: worker-entrypoint.sh missing claude_session.json handling"
        );
        assert!(
            script.contains("$HOME/.claude.json") || script.contains("HOME/.claude.json"),
            "drift: worker-entrypoint.sh must write the session blob onto $HOME/.claude.json"
        );
        assert!(
            script.contains("jq -s '.[0] * .[1]'"),
            "drift: worker-entrypoint.sh must merge via `jq -s '.[0] * .[1]'`; \
             a plain `cp` wipes accumulated state. Update both the script \
             and BUNDLE_SOURCING_SH in lockstep."
        );
        assert!(
            BUNDLE_SOURCING_SH.contains("/run/maestro-secrets/claude_session.json"),
            "drift: BUNDLE_SOURCING_SH missing claude_session.json handling"
        );

        // The empty-mountpoint observability breadcrumb must be present in
        // BOTH the script and the Rust constant. If it drifts out of one,
        // future lifetime bugs go silent again.
        assert!(
            script.contains("[maestro-bundle]"),
            "drift: worker-entrypoint.sh missing [maestro-bundle] empty-mountpoint warning"
        );
        assert!(
            BUNDLE_SOURCING_SH.contains("[maestro-bundle]"),
            "drift: BUNDLE_SOURCING_SH missing [maestro-bundle] empty-mountpoint warning"
        );
    }

    /// When the runner has NO secrets bundle attached, the
    /// `sh -c` payload built by `build_sh_payload` must NOT reference
    /// `/run/maestro-secrets/` — keeps the legacy path's argv clean and
    /// avoids any chance of confusing logs.
    #[test]
    fn build_sh_payload_without_bundle_does_not_source_run_maestro_secrets() {
        let cmd = build_sh_payload(false, "claude", &["--version"]);
        assert!(
            !cmd.contains("/run/maestro-secrets/"),
            "legacy payload must not reference /run/maestro-secrets/; got: {cmd}"
        );
        assert!(
            !cmd.contains("MAESTRO_AUTH_BUNDLE"),
            "legacy payload must not gate on MAESTRO_AUTH_BUNDLE; got: {cmd}"
        );
        // Sanity: existing legacy stanza is still there.
        assert!(cmd.contains("$HOME/.config/gh/gh-app-token"));
        assert!(cmd.starts_with("if [ ! -f \"$HOME/.claude.json\" ]"));
        assert!(cmd.contains("exec claude --version"));
    }

    /// When a bundle IS attached, `build_sh_payload`'s
    /// payload MUST contain the bundle-sourcing block BEFORE the
    /// `exec` so the agent CLI sees its token in env.
    #[test]
    fn build_sh_payload_with_bundle_sources_secrets_before_exec() {
        let cmd = build_sh_payload(true, "claude", &["--version"]);

        // Bundle-sourcing block must be present.
        assert!(
            cmd.contains("/run/maestro-secrets/claude"),
            "bundle-attached payload must source /run/maestro-secrets/claude; got: {cmd}"
        );
        assert!(
            cmd.contains("export CLAUDE_CODE_OAUTH_TOKEN"),
            "bundle-attached payload must export CLAUDE_CODE_OAUTH_TOKEN; got: {cmd}"
        );
        // And it must precede the `exec`, not run after.
        let bundle_pos = cmd
            .find("/run/maestro-secrets/claude")
            .expect("bundle source position");
        let exec_pos = cmd.find("exec claude").expect("exec position");
        assert!(
            bundle_pos < exec_pos,
            "bundle sourcing must precede exec; bundle@{bundle_pos} exec@{exec_pos} in: {cmd}"
        );
        // And all four provider mappings must be present (defence in depth
        // against accidentally narrowing the splice). OpenCode intentionally
        // absent per spec 2026-05-27 §2.1 — uses opencode.json instead.
        for file in [
            "/run/maestro-secrets/claude",
            "/run/maestro-secrets/cursor",
            "/run/maestro-secrets/codex",
            "/run/maestro-secrets/gh",
        ] {
            assert!(
                cmd.contains(file),
                "bundle-attached payload must reference {file}"
            );
        }
    }

    /// Lock-in test for `build_sh_payload`'s exact wire-format. Locks the
    /// composition order (`RESTORE; FIX_PERMS; GH_TOKEN exec <user>`),
    /// the `; ` and ` ` separators, the `exec ` keyword, and the
    /// shell-escaped argv joining for the no-bundle branch. Any drift in
    /// the format string or argument quoting fails this test.
    ///
    /// Intentionally uses the module-level snippet constants in the
    /// expected value so the runtime constants themselves remain the
    /// single source of truth (covered separately by
    /// `bundle_sourcing_snippet_covers_every_documented_mapping`), while
    /// this test pins the surrounding assembly byte-for-byte.
    #[test]
    fn lock_in_build_sh_payload_no_bundle_exact_output() {
        let actual = build_sh_payload(false, "echo", &["hello", "world"]);
        let expected = format!(
            "{RESTORE_SNIPPET}; {FIX_PERMS_SNIPPET}; {GH_TOKEN_SNIPPET} {GIT_CRED_HELPER_SNIPPET} exec echo hello world"
        );
        assert_eq!(
            actual, expected,
            "build_sh_payload(false, ...) wire-format drifted"
        );
    }

    /// Both payload variants must configure the git credential helper so the
    /// agent's `git push` authenticates — the gap that produced the
    /// "could not read Username for 'https://github.com'" loop.
    #[test]
    fn build_sh_payload_configures_git_credential_helper() {
        for has_bundle in [true, false] {
            let body = build_sh_payload(has_bundle, "opencode", &["run"]);
            assert!(
                body.contains("credential.https://github.com.helper"),
                "has_bundle={has_bundle}: payload must configure the git credential helper"
            );
            // Runs before the agent so git is ready when the agent pushes.
            let helper_at = body.find("credential.https://github.com.helper").unwrap();
            let exec_at = body.find("exec opencode").unwrap();
            assert!(helper_at < exec_at, "helper must be configured before exec");
        }
    }

    /// The credential helper must prefer `$GH_TOKEN` (the resolved per-workflow
    /// token — a user PAT when present) over the App-token file, so a provided
    /// PAT genuinely overrides the App token.
    #[test]
    fn git_cred_helper_prefers_gh_token_over_app_file() {
        assert!(
            GIT_CRED_HELPER_SNIPPET.contains(
                r#"${GH_TOKEN:-$(cat /home/maestro/.config/gh/gh-app-token 2>/dev/null)}"#
            ),
            "credential helper must be GH_TOKEN-first with the App-token file as fallback"
        );
        // The token-file path must match github_app::TOKEN_FILE_PATH.
        assert!(
            GIT_CRED_HELPER_SNIPPET.contains(crate::github_app::TOKEN_FILE_PATH),
            "helper token path must match github_app::TOKEN_FILE_PATH"
        );
    }
}

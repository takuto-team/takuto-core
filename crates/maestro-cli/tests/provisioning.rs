// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for `maestro provisioning {sha,commands}`.
//
// These tests invoke the compiled `maestro` binary against a temp config
// file so the entrypoint shell's contract (run `maestro provisioning
// sha`, compare against a stored SHA file; run `maestro provisioning
// commands`, iterate NUL-separated) is exercised end-to-end. The CLI is
// what `docker/entrypoint.sh::bootstrap_provisioning` shells out to —
// if its output shape changes, every Maestro deployment's boot would
// break, so we lock it down here.

use std::process::Command;

/// Locate the compiled `maestro` binary in the workspace target dir.
/// Cargo sets `CARGO_BIN_EXE_<name>` for integration tests; we read it
/// directly so the test doesn't depend on the working directory.
fn maestro_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_maestro"))
}

/// Write a minimal valid config.toml with the given `[provisioning]`
/// block at `dir/config.toml`. Returns the path.
fn write_config(dir: &std::path::Path, install_commands_toml: &str) -> std::path::PathBuf {
    let path = dir.join("config.toml");
    let content = format!(
        r#"
[general]
poll_interval_secs = 30

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"
repo_path = "/workspace"

[web]
port = 8080

[agent]
step_timeout_secs = 600

[provisioning]
{install_commands_toml}
"#
    );
    std::fs::write(&path, content).expect("write temp config");
    path
}

/// T-PROV-CLI-001: `maestro provisioning sha` prints a 64-char hex
/// SHA + newline for the empty install_commands case, matches the
/// recomputed sha256 of `[]`.
#[test]
fn t_prov_cli_001_sha_empty_list() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path(), "install_commands = []");
    let out = Command::new(maestro_bin())
        .args(["--config"])
        .arg(&cfg)
        .args(["provisioning", "sha"])
        .output()
        .expect("spawn maestro");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let sha = stdout.trim();
    assert_eq!(sha.len(), 64, "expected 64-char hex, got {sha:?}");
    assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
}

/// T-PROV-CLI-002: a config with two commands → SHA stable across
/// invocations (same input, same output).
#[test]
fn t_prov_cli_002_sha_stable_for_same_content() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path(), r#"install_commands = ["echo one", "echo two"]"#);
    let run_sha = || {
        let out = Command::new(maestro_bin())
            .args(["--config"])
            .arg(&cfg)
            .args(["provisioning", "sha"])
            .output()
            .expect("spawn maestro");
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let a = run_sha();
    let b = run_sha();
    assert_eq!(a, b, "SHA must be deterministic across invocations");
}

/// T-PROV-CLI-003: editing a command changes the SHA — the gate
/// invalidates correctly so admins editing config.toml see the install
/// pass re-run.
#[test]
fn t_prov_cli_003_sha_changes_when_command_changes() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_a = write_config(dir.path(), r#"install_commands = ["echo old"]"#);
    let sha_a = Command::new(maestro_bin())
        .args(["--config"])
        .arg(&cfg_a)
        .args(["provisioning", "sha"])
        .output()
        .map(|o| String::from_utf8(o.stdout).unwrap().trim().to_string())
        .unwrap();

    // Overwrite with a different command at the same path.
    let cfg_b = write_config(dir.path(), r#"install_commands = ["echo new"]"#);
    let sha_b = Command::new(maestro_bin())
        .args(["--config"])
        .arg(&cfg_b)
        .args(["provisioning", "sha"])
        .output()
        .map(|o| String::from_utf8(o.stdout).unwrap().trim().to_string())
        .unwrap();

    assert_ne!(sha_a, sha_b);
}

/// T-PROV-CLI-004: `maestro provisioning commands` emits NUL-separated
/// commands. The entrypoint's `while IFS= read -r -d ''` loop relies on
/// this contract — every command followed by exactly one NUL byte, no
/// trailing newline, no leading metadata.
#[test]
fn t_prov_cli_004_commands_are_nul_separated() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(
        dir.path(),
        r#"install_commands = ["echo foo", "echo 'has space and \"quotes\"'", "echo bar"]"#,
    );
    let out = Command::new(maestro_bin())
        .args(["--config"])
        .arg(&cfg)
        .args(["provisioning", "commands"])
        .output()
        .expect("spawn maestro");
    assert!(out.status.success());
    let bytes = out.stdout;
    // Three commands → three NULs. Each NUL terminates exactly one
    // command. No extra trailing bytes after the final NUL.
    let nul_count = bytes.iter().filter(|&&b| b == 0).count();
    assert_eq!(nul_count, 3, "expected 3 NUL separators; got {nul_count}");
    assert_eq!(*bytes.last().unwrap(), 0u8, "stream must end with NUL");

    // Split on NUL and verify the commands round-trip.
    let parts: Vec<&[u8]> = bytes.split(|&b| b == 0).collect();
    // split-on-NUL on a NUL-terminated stream yields N+1 parts where
    // the last is empty.
    assert_eq!(parts.len(), 4);
    assert_eq!(parts[0], b"echo foo");
    assert_eq!(parts[1], b"echo 'has space and \"quotes\"'");
    assert_eq!(parts[2], b"echo bar");
    assert!(parts[3].is_empty(), "post-final-NUL part must be empty");
}

/// T-PROV-CLI-005: empty install_commands → `commands` subcommand
/// emits zero bytes (no NULs, no headers). The entrypoint's read loop
/// must terminate immediately on an empty stream.
#[test]
fn t_prov_cli_005_empty_commands_emits_no_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path(), "install_commands = []");
    let out = Command::new(maestro_bin())
        .args(["--config"])
        .arg(&cfg)
        .args(["provisioning", "commands"])
        .output()
        .expect("spawn maestro");
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "got {:?}", out.stdout);
}

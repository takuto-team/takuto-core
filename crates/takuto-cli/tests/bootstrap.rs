// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Bootstrap safety-net: `load_config_or_default` must return `Config::default`
// when the config file is simply absent (the no-config first-run path), and
// must propagate errors when the file exists but is malformed.
//
// We exercise this via the CLI subcommands that call `load_config_or_default`
// — `provisioning sha`, `provisioning commands`, and `egress-hosts` — because
// the helper is private. The contract matters for every Docker deployment that
// starts without a pre-written config.toml.

use std::process::Command;

fn takuto_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_takuto"))
}

// ---------------------------------------------------------------------------
// load_config_or_default via `provisioning sha`
// ---------------------------------------------------------------------------

/// T-BOOTSTRAP-001: missing config.toml → `provisioning sha` exits 0 and
/// prints the SHA of an empty install_commands list (the default).
/// This is the first-run bootstrap path: the Docker entrypoint calls
/// `takuto provisioning sha` before any config.toml exists.
#[test]
fn missing_config_provisioning_sha_exits_ok_with_empty_sha() {
    let dir = tempfile::tempdir().unwrap();
    let nonexistent = dir.path().join("no_config.toml");

    let out = Command::new(takuto_bin())
        .args(["--config"])
        .arg(&nonexistent)
        .args(["provisioning", "sha"])
        .output()
        .expect("spawn takuto");

    assert!(
        out.status.success(),
        "missing config.toml must exit 0 (uses Config::default); stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sha = String::from_utf8(out.stdout).unwrap();
    let sha = sha.trim();
    assert_eq!(sha.len(), 64, "must print a 64-char hex SHA; got {sha:?}");
    assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
}

/// T-BOOTSTRAP-002: malformed config.toml → `provisioning sha` exits non-zero.
/// A broken file is NOT silently treated as defaults; the operator must fix it.
#[test]
fn malformed_config_provisioning_sha_exits_failure() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("config.toml");
    std::fs::write(&bad, "this is [ not } valid toml !!!").unwrap();

    let out = Command::new(takuto_bin())
        .args(["--config"])
        .arg(&bad)
        .args(["provisioning", "sha"])
        .output()
        .expect("spawn takuto");

    assert!(
        !out.status.success(),
        "malformed config.toml must exit non-zero; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

// ---------------------------------------------------------------------------
// load_config_or_default via `provisioning commands`
// ---------------------------------------------------------------------------

/// T-BOOTSTRAP-003: missing config.toml → `provisioning commands` exits 0
/// and emits zero bytes (default has no install_commands).
#[test]
fn missing_config_provisioning_commands_exits_ok_empty() {
    let dir = tempfile::tempdir().unwrap();
    let nonexistent = dir.path().join("no_config.toml");

    let out = Command::new(takuto_bin())
        .args(["--config"])
        .arg(&nonexistent)
        .args(["provisioning", "commands"])
        .output()
        .expect("spawn takuto");

    assert!(
        out.status.success(),
        "missing config.toml must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "default config has no install_commands; got {:?}",
        out.stdout
    );
}

/// T-BOOTSTRAP-004: malformed config.toml → `provisioning commands` exits non-zero.
#[test]
fn malformed_config_provisioning_commands_exits_failure() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("config.toml");
    std::fs::write(&bad, "[general]\npoll_interval_secs = \"not_a_number\"").unwrap();

    let out = Command::new(takuto_bin())
        .args(["--config"])
        .arg(&bad)
        .args(["provisioning", "commands"])
        .output()
        .expect("spawn takuto");

    assert!(
        !out.status.success(),
        "malformed config.toml must exit non-zero; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

// ---------------------------------------------------------------------------
// load_config_or_default via `egress-hosts`
// ---------------------------------------------------------------------------

/// T-BOOTSTRAP-005: missing config.toml → `egress-hosts` exits 0.
/// `docker/egress-rules.sh` calls `takuto egress-hosts` before any wizard
/// run; the command must not fail with no config.toml present.
#[test]
fn missing_config_egress_hosts_exits_ok() {
    let dir = tempfile::tempdir().unwrap();
    let nonexistent = dir.path().join("no_config.toml");

    let out = Command::new(takuto_bin())
        .args(["--config"])
        .arg(&nonexistent)
        .args(["egress-hosts"])
        .output()
        .expect("spawn takuto");

    assert!(
        out.status.success(),
        "missing config.toml must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Default config uses claude provider; at least one host must be emitted.
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        !stdout.trim().is_empty(),
        "expected egress hosts for default (claude) provider"
    );
}

/// T-BOOTSTRAP-006: malformed config.toml → `egress-hosts` exits non-zero.
#[test]
fn malformed_config_egress_hosts_exits_failure() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("config.toml");
    std::fs::write(&bad, "this is [ not } valid toml !!!").unwrap();

    let out = Command::new(takuto_bin())
        .args(["--config"])
        .arg(&bad)
        .args(["egress-hosts"])
        .output()
        .expect("spawn takuto");

    assert!(
        !out.status.success(),
        "malformed config.toml must exit non-zero; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

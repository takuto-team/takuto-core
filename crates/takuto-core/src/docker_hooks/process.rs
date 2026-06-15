// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Process-spawn primitives shared by every probe in `docker_hooks`.

// libc FFI: detach a spawned child into its own process group and group-wide SIGKILL.
#![allow(unsafe_code)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) fn preflight_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/takuto"))
}

#[cfg(unix)]
fn configure_auth_command_unix(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_auth_command_unix(_cmd: &mut Command) {}

#[cfg(unix)]
fn kill_process_group_best_effort(child: &mut std::process::Child) {
    let pid = child.id();
    if pid > 0 {
        unsafe {
            let _ = libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
fn kill_process_group_best_effort(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Run an auth probe with a wall-clock timeout so `docker compose up` cannot hang forever
/// (e.g. Cursor `agent status` waiting on network without a client-side deadline).
pub(super) fn auth_cmd_ok(program: &str, args: &[&str]) -> bool {
    let timeout = if args == ["status"] {
        Duration::from_secs(45)
    } else {
        Duration::from_secs(30)
    };

    let mut cmd = Command::new(program);
    cmd.args(args).stdout(Stdio::null()).stderr(Stdio::null());
    configure_auth_command_unix(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return false,
    };

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {}
            Err(_) => {
                kill_process_group_best_effort(&mut child);
                return false;
            }
        }
        if start.elapsed() >= timeout {
            kill_process_group_best_effort(&mut child);
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

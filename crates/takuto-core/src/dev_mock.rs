// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Dev-mode agent session mocking.
//!
//! When enabled, `ClaudeSession::run_prompt` and `CursorSession::run_prompt` short-circuit
//! into [`run_claude_mock`] / [`run_cursor_mock`] instead of spawning a real agent process.
//! This lets E2E tests exercise the full workflow pipeline (worktree creation, step driver,
//! dashboard streaming, "Improve with AI") **without burning a single Claude/Cursor token**.
//!
//! ## Activation precedence (high to low)
//! 1. [`set_test_override(Some(true|false))`] — per-process atomic, wins outright.
//! 2. `TAKUTO_DEV_MOCK_AGENT` env var (`1` / `true` / `TRUE` / `yes`).
//! 3. The installed `DevConfig::mock_agent` flag (set by `main.rs` from `config.toml`).
//! 4. `false` (off — production default).
//!
//! ## Test override caveat
//! The override is **process-global**. Tests that touch it must either live in their own
//! integration test file or use `#[serial_test::serial]` to avoid races. The [`MockGuard`]
//! RAII helper resets the override on drop so even panicking tests don't poison their
//! neighbours.

use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::DevConfig;
use crate::error::{Result, TakutoError};
use crate::process::OutputLine;

/// Default canned script used when no `mock_agent_script_path` is configured.
///
/// `{worktree}` is interpolated to the actual worktree path so the mock looks
/// realistic in dashboard logs.
pub const DEFAULT_MOCK_SCRIPT: &[&str] = &[
    "Reading worktree at {worktree}",
    "Found 3 files matching pattern",
    "Analyzing src/main.rs",
    "Editing src/main.rs (added 12, removed 4)",
    "Running cargo check",
    "  Finished `dev` profile [unoptimized + debuginfo] target(s)",
    "Done.",
];

/// Trailing summary appended to the final returned `output` string so downstream
/// "parse the final result" code sees a stable marker.
const MOCK_SUMMARY_LINE: &str = "\n\n[mock-agent] session complete (mocked, no AI invoked)";

/// Stable substring from the `IMPROVE_SYSTEM_PROMPT` in `routes/tickets.rs`.
/// When present in `system_prompt`, the mock returns a two-pane shape.
const IMPROVE_MARKER: &str = "technical writer who improves";

// --- Per-process activation signals ---

/// Test override: `-1` = no override (defer to env+config), `0` = force off, `1` = force on.
static TEST_OVERRIDE: AtomicI8 = AtomicI8::new(-1);

/// Installed `DevConfig::mock_agent` flag (mutable Arc so reloads are visible).
static CONFIG_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Installed `mock_agent_line_delay_ms`. Defaults to 75 ms when nothing is installed.
static CONFIG_LINE_DELAY_MS: AtomicU64 = AtomicU64::new(75);

/// Installed `mock_agent_total_ms`. Defaults to 5000 ms when nothing is installed.
static CONFIG_TOTAL_MS: AtomicU64 = AtomicU64::new(5000);

/// Installed `mock_agent_script_path` (read at the start of each mock session).
fn script_path_slot() -> &'static Mutex<Option<String>> {
    static SCRIPT_PATH: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SCRIPT_PATH.get_or_init(|| Mutex::new(None))
}

// --- Public API ---

/// Install (or update) the dev-mode configuration snapshot. Safe to call multiple times —
/// subsequent calls update the in-place values so config reloads take effect without restart.
///
/// Call this exactly once at startup after `Config::load`, and again from the
/// `ConfigWatcher` reload path so flipping `[dev] mock_agent` in `config.toml` is
/// picked up within the watcher tick.
pub fn install_dev_config(cfg: &DevConfig) {
    let flag = CONFIG_FLAG.get_or_init(|| Arc::new(AtomicBool::new(cfg.mock_agent)));
    flag.store(cfg.mock_agent, Ordering::SeqCst);
    CONFIG_LINE_DELAY_MS.store(cfg.mock_agent_line_delay_ms, Ordering::SeqCst);
    CONFIG_TOTAL_MS.store(cfg.mock_agent_total_ms, Ordering::SeqCst);
    // Update the script path; ignore failure to acquire the lock (only used during install).
    let path = cfg.mock_agent_script_path.clone();
    // Use try_lock first; if contended, spawn nothing — install is not on a hot path so block.
    let slot = script_path_slot();
    // We're synchronous; block_in_place would be wrong here. Use a blocking lock via tokio.
    // install_dev_config is allowed to be called from sync contexts during startup, so use
    // a parking-lot-free approach: spawn nothing, just acquire via blocking.
    // Tokio Mutex doesn't have a sync acquire; route through a small trick: try_lock in a loop
    // backed by std::thread::yield_now. install is rare and uncontended.
    loop {
        match slot.try_lock() {
            Ok(mut g) => {
                *g = path;
                break;
            }
            Err(_) => std::thread::yield_now(),
        }
    }
}

/// Returns `true` when the dev-mode mock should intercept the next agent call.
///
/// Resolution order: test override → env var → installed config flag → `false`.
pub fn is_enabled_from_runtime() -> bool {
    if let Some(v) = test_override() {
        return v;
    }
    if env_enabled() {
        return true;
    }
    if let Some(flag) = CONFIG_FLAG.get() {
        return flag.load(Ordering::SeqCst);
    }
    false
}

/// Set a per-process override for [`is_enabled_from_runtime`]. Pass `None` to
/// remove the override and defer to env + config.
///
/// **Process-global** — tests using this should be in their own integration file
/// or marked `#[serial_test::serial]`.
pub fn set_test_override(value: Option<bool>) {
    let v = match value {
        None => -1,
        Some(false) => 0,
        Some(true) => 1,
    };
    TEST_OVERRIDE.store(v, Ordering::SeqCst);
}

/// RAII helper that flips the test override on and clears it on drop.
///
/// ```ignore
/// let _g = dev_mock::MockGuard::on();
/// // ... exercise the agent code path; mock is active here ...
/// // _g drops here; override returns to None.
/// ```
pub struct MockGuard {
    previous: Option<bool>,
}

impl MockGuard {
    /// Force the mock **on** for the lifetime of this guard.
    pub fn on() -> Self {
        let previous = test_override();
        set_test_override(Some(true));
        Self { previous }
    }

    /// Force the mock **off** for the lifetime of this guard.
    pub fn off() -> Self {
        let previous = test_override();
        set_test_override(Some(false));
        Self { previous }
    }
}

impl Drop for MockGuard {
    fn drop(&mut self) {
        set_test_override(self.previous);
    }
}

// --- Mock implementations ---

/// Run a Claude-flavoured mock session.
///
/// Returns `(session_id, joined_output)` shaped exactly like the real
/// `run_claude_session`. Emits one line per script entry on `line_tx` when present.
pub async fn run_claude_mock(
    worktree: &Path,
    prompt: &str,
    cancel_token: CancellationToken,
    line_tx: Option<tokio::sync::mpsc::UnboundedSender<OutputLine>>,
    resume_session_id: Option<&str>,
    system_prompt: Option<&str>,
) -> Result<(String, String)> {
    run_mock_impl(
        "claude",
        worktree,
        prompt,
        cancel_token,
        line_tx,
        resume_session_id,
        system_prompt,
    )
    .await
}

/// Run a Cursor-flavoured mock session.
///
/// Returns `(session_id, joined_output)` shaped exactly like the real
/// `run_cursor_agent_session`.
pub async fn run_cursor_mock(
    worktree: &Path,
    prompt: &str,
    cancel_token: CancellationToken,
    line_tx: Option<tokio::sync::mpsc::UnboundedSender<OutputLine>>,
    resume_session_id: Option<&str>,
    system_prompt: Option<&str>,
) -> Result<(String, String)> {
    run_mock_impl(
        "cursor",
        worktree,
        prompt,
        cancel_token,
        line_tx,
        resume_session_id,
        system_prompt,
    )
    .await
}

async fn run_mock_impl(
    provider: &str,
    worktree: &Path,
    prompt: &str,
    cancel_token: CancellationToken,
    line_tx: Option<tokio::sync::mpsc::UnboundedSender<OutputLine>>,
    resume_session_id: Option<&str>,
    system_prompt: Option<&str>,
) -> Result<(String, String)> {
    let session_id = format!("mock-{}", uuid::Uuid::new_v4());
    info!(
        provider = provider,
        session_id = %session_id,
        worktree = %worktree.display(),
        prompt_len = prompt.len(),
        system_prompt_len = system_prompt.map(|s| s.len()).unwrap_or(0),
        resume = ?resume_session_id,
        "[mock-agent] starting scripted session"
    );

    // --- Improve-with-AI fast path -------------------------------------------------
    // When the system prompt carries the improve marker, return the two-pane shape
    // expected by `improve_ticket`'s `split_once("\n---\n")` parser. This works for
    // both streaming and non-streaming callers (description-edit uses line_tx = None).
    if let Some(sp) = system_prompt
        && sp.contains(IMPROVE_MARKER)
    {
        let summary =
            parse_summary_from_prompt(prompt).unwrap_or_else(|| "Mocked Title".to_string());
        let body = parse_body_from_prompt(prompt);
        let output = format!("{summary} [mocked]\n---\n{body} [improved by mock]");
        // Honor cancellation even on this fast path so tests can verify it.
        if cancel_token.is_cancelled() {
            return Err(TakutoError::Cancelled);
        }
        info!(
            provider = provider,
            session_id = %session_id,
            output_len = output.len(),
            "[mock-agent] improve-path session complete"
        );
        return Ok((session_id, output));
    }

    // --- Standard scripted path ----------------------------------------------------
    let total_cap = Duration::from_millis(CONFIG_TOTAL_MS.load(Ordering::SeqCst).max(1));
    let per_line = Duration::from_millis(CONFIG_LINE_DELAY_MS.load(Ordering::SeqCst).max(1));
    let started = std::time::Instant::now();

    let script = load_script(worktree).await;

    let mut emitted: Vec<String> = Vec::with_capacity(script.len() + 2);

    // Resume note: if the caller resumes a mock session, surface that in the stream.
    // If they resume something that doesn't look like a mock session, log a warning
    // but proceed — test code may legitimately resume a real session captured before
    // the mock was enabled.
    if let Some(sid) = resume_session_id {
        if sid.starts_with("mock-") {
            let line = format!("Resuming mock session {sid}…");
            emit_line(&line_tx, &line);
            emitted.push(line);
            if cancellable_sleep(&cancel_token, per_line).await.is_err() {
                return Err(TakutoError::Cancelled);
            }
        } else {
            warn!(
                provider = provider,
                resume = %sid,
                "[mock-agent] resume_session_id is not a mock-prefixed id; proceeding without resume"
            );
        }
    }

    for raw in script.iter() {
        // Hard time cap: stop emitting once we exceed mock_agent_total_ms.
        if started.elapsed() >= total_cap {
            break;
        }
        let line = raw.replace("{worktree}", &worktree.display().to_string());
        emit_line(&line_tx, &line);
        emitted.push(line);
        if cancellable_sleep(&cancel_token, per_line).await.is_err() {
            return Err(TakutoError::Cancelled);
        }
    }

    // Final synthetic result line.
    let result_line = format!("[mock-agent/{provider}] result ready");
    emit_line(&line_tx, &result_line);
    emitted.push(result_line);

    let mut output = emitted.join("\n");
    output.push_str(MOCK_SUMMARY_LINE);
    info!(
        provider = provider,
        session_id = %session_id,
        output_len = output.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "[mock-agent] scripted session complete"
    );
    Ok((session_id, output))
}

/// Sleep for `dur`, returning early with `Err(())` when the cancellation token fires.
async fn cancellable_sleep(
    token: &CancellationToken,
    dur: Duration,
) -> std::result::Result<(), ()> {
    tokio::select! {
        _ = tokio::time::sleep(dur) => Ok(()),
        _ = token.cancelled() => Err(()),
    }
}

fn emit_line(tx: &Option<tokio::sync::mpsc::UnboundedSender<OutputLine>>, line: &str) {
    if let Some(tx) = tx
        && let Err(e) = tx.send(OutputLine {
            content: line.to_string(),
            stream: "stdout".to_string(),
        })
    {
        // The receiver has dropped — that's fine; the mock can still complete.
        warn!(error = %e, "[mock-agent] line_tx send failed (receiver dropped)");
    }
}

async fn load_script(worktree: &Path) -> Vec<String> {
    let slot = script_path_slot();
    let path = {
        let g = slot.lock().await;
        g.clone()
    };
    let path = match path {
        Some(p) if !p.trim().is_empty() => p,
        _ => return DEFAULT_MOCK_SCRIPT.iter().map(|s| s.to_string()).collect(),
    };
    // Resolve relative paths against the worktree first; many callers pass a temp
    // dir, which is fine. Tests that point at an absolute path work without change.
    let abs = if Path::new(&path).is_absolute() {
        std::path::PathBuf::from(&path)
    } else {
        worktree.join(&path)
    };
    match tokio::fs::read_to_string(&abs).await {
        Ok(content) => {
            let lines: Vec<String> = content
                .lines()
                .map(|l| l.to_string())
                .filter(|l| !l.trim().is_empty())
                .collect();
            if lines.is_empty() {
                warn!(
                    path = %abs.display(),
                    "[mock-agent] script file is empty; falling back to DEFAULT_MOCK_SCRIPT"
                );
                DEFAULT_MOCK_SCRIPT.iter().map(|s| s.to_string()).collect()
            } else {
                lines
            }
        }
        Err(e) => {
            warn!(
                error = %e,
                path = %abs.display(),
                "[mock-agent] cannot read script file; falling back to DEFAULT_MOCK_SCRIPT"
            );
            DEFAULT_MOCK_SCRIPT.iter().map(|s| s.to_string()).collect()
        }
    }
}

// --- Private helpers ---

fn env_enabled() -> bool {
    std::env::var("TAKUTO_DEV_MOCK_AGENT")
        .ok()
        .as_deref()
        .map(|s| matches!(s, "1" | "true" | "TRUE" | "yes"))
        .unwrap_or(false)
}

pub(crate) fn test_override() -> Option<bool> {
    match TEST_OVERRIDE.load(Ordering::SeqCst) {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// Best-effort summary extraction from the improve prompt.
///
/// The improve-ticket route formats its prompt with a section like
/// `# Current Title\n<summary>\n` or `Summary: <title>`. We accept either.
fn parse_summary_from_prompt(prompt: &str) -> Option<String> {
    // Look for an obvious "summary"/"title" line.
    for line in prompt.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Summary:") {
            let v = rest.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
        if let Some(rest) = t.strip_prefix("Title:") {
            let v = rest.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    // Fall back: first non-empty non-header line.
    for line in prompt.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        // Use only the first 80 chars to avoid swallowing the whole description.
        let truncated: String = t.chars().take(80).collect();
        return Some(truncated);
    }
    None
}

fn parse_body_from_prompt(prompt: &str) -> String {
    // Use the last reasonably-sized chunk of the prompt as a stand-in "body excerpt".
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return "(empty)".to_string();
    }
    let body: String = trimmed.chars().take(400).collect();
    body
}

// --- Tests ---

#[cfg(test)]
// Test-only `std::env` mutation (unsafe in the 2024 edition); serialised via the test lock.
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use std::sync::OnceLock as StdOnceLock;
    use std::sync::atomic::Ordering;
    use std::time::Instant;
    use tokio::sync::{Mutex as TokioMutex, MutexGuard as TokioMutexGuard};

    /// Tests in this module touch process-global state (env var, atomics, OnceLock).
    /// Serialize them so they don't race each other.
    ///
    /// We use `tokio::sync::Mutex` instead of `std::sync::Mutex` so the async
    /// tests can hold the guard across `.await` without tripping
    /// `clippy::await_holding_lock` and — more importantly — without risk of
    /// deadlocking a single-threaded executor. Sync `#[test]` functions are
    /// not inside a tokio runtime, so they may use `blocking_lock()`; async
    /// `#[tokio::test]` functions acquire via `.lock().await`.
    fn lock_singleton() -> &'static TokioMutex<()> {
        static LOCK: StdOnceLock<TokioMutex<()>> = StdOnceLock::new();
        LOCK.get_or_init(|| TokioMutex::new(()))
    }

    /// Sync entry point — call from plain `#[test]` functions.
    fn test_lock() -> TokioMutexGuard<'static, ()> {
        lock_singleton().blocking_lock()
    }

    /// Async entry point — call from `#[tokio::test]` functions.
    async fn test_lock_async() -> TokioMutexGuard<'static, ()> {
        lock_singleton().lock().await
    }

    /// Ensure the env var is unset across tests that don't explicitly set it.
    fn clear_env() {
        // Safety: tests are serialized via test_lock(); no other thread reads env here.
        unsafe {
            std::env::remove_var("TAKUTO_DEV_MOCK_AGENT");
        }
    }

    fn reset_globals() {
        TEST_OVERRIDE.store(-1, Ordering::SeqCst);
        if let Some(flag) = CONFIG_FLAG.get() {
            flag.store(false, Ordering::SeqCst);
        }
        clear_env();
    }

    #[test]
    fn mock_off_by_default() {
        let _g = test_lock();
        reset_globals();
        assert!(!is_enabled_from_runtime());
    }

    #[test]
    fn env_var_enables() {
        let _g = test_lock();
        reset_globals();
        // Safety: tests are serialized via test_lock().
        unsafe {
            std::env::set_var("TAKUTO_DEV_MOCK_AGENT", "1");
        }
        assert!(is_enabled_from_runtime());
        clear_env();
        assert!(!is_enabled_from_runtime());
    }

    #[test]
    fn env_var_accepts_yes_and_true() {
        let _g = test_lock();
        reset_globals();
        for v in ["1", "true", "TRUE", "yes"] {
            unsafe {
                std::env::set_var("TAKUTO_DEV_MOCK_AGENT", v);
            }
            assert!(is_enabled_from_runtime(), "expected enabled for {v}");
        }
        unsafe {
            std::env::set_var("TAKUTO_DEV_MOCK_AGENT", "0");
        }
        assert!(!is_enabled_from_runtime());
        clear_env();
    }

    #[test]
    fn config_flag_enables_when_no_override() {
        let _g = test_lock();
        reset_globals();
        let cfg = DevConfig {
            mock_agent: true,
            ..Default::default()
        };
        install_dev_config(&cfg);
        assert!(is_enabled_from_runtime());
        // Now reinstall with off.
        install_dev_config(&DevConfig::default());
        assert!(!is_enabled_from_runtime());
    }

    #[test]
    fn test_override_wins_over_env_and_config() {
        let _g = test_lock();
        reset_globals();
        unsafe {
            std::env::set_var("TAKUTO_DEV_MOCK_AGENT", "1");
        }
        install_dev_config(&DevConfig {
            mock_agent: true,
            ..Default::default()
        });
        set_test_override(Some(false));
        assert!(!is_enabled_from_runtime());
        set_test_override(None);
        // Defers to env/config now.
        assert!(is_enabled_from_runtime());
        set_test_override(None);
        clear_env();
        install_dev_config(&DevConfig::default());
    }

    #[test]
    fn mock_guard_resets_on_drop() {
        let _g = test_lock();
        reset_globals();
        assert!(!is_enabled_from_runtime());
        {
            let _h = MockGuard::on();
            assert!(is_enabled_from_runtime());
        }
        assert!(!is_enabled_from_runtime());
    }

    #[test]
    fn mock_guard_restores_previous_override() {
        let _g = test_lock();
        reset_globals();
        set_test_override(Some(false));
        {
            let _h = MockGuard::on();
            assert!(is_enabled_from_runtime());
        }
        assert!(!is_enabled_from_runtime());
        set_test_override(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn claude_mock_emits_lines() {
        let _g = test_lock_async().await;
        reset_globals();
        // Use fast tickers so the test stays under ~1 s.
        install_dev_config(&DevConfig {
            mock_agent: false,
            mock_agent_script_path: None,
            mock_agent_line_delay_ms: 5,
            mock_agent_total_ms: 2_000,
        });

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<OutputLine>();
        let token = CancellationToken::new();
        let worktree = std::path::PathBuf::from("/tmp/mock-test");
        let start = Instant::now();
        let (sid, output) = run_claude_mock(&worktree, "do stuff", token, Some(tx), None, None)
            .await
            .expect("mock should succeed");
        let elapsed = start.elapsed();
        assert!(
            sid.starts_with("mock-"),
            "session id should be mock-prefixed: {sid}"
        );
        assert!(!output.is_empty(), "output should be non-empty: {output:?}");
        assert!(
            output.contains("[mock-agent] session complete"),
            "output should carry the stable marker: {output:?}"
        );
        // We should have at least the script + the final result line.
        let mut count = 0;
        while let Ok(_l) = rx.try_recv() {
            count += 1;
        }
        assert!(count >= 2, "expected ≥2 emitted lines, got {count}");
        // Wall time bound: total cap + 200 ms slack.
        assert!(
            elapsed <= Duration::from_millis(2_000 + 200),
            "mock ran too long: {elapsed:?}"
        );
        reset_globals();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn claude_mock_honors_cancel() {
        let _g = test_lock_async().await;
        reset_globals();
        install_dev_config(&DevConfig {
            mock_agent: false,
            mock_agent_script_path: None,
            mock_agent_line_delay_ms: 50,
            mock_agent_total_ms: 5_000,
        });

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<OutputLine>();
        let token = CancellationToken::new();
        let token2 = token.clone();
        let worktree = std::path::PathBuf::from("/tmp/mock-cancel-test");

        // Cancel after a brief delay so the mock is mid-stream.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            token2.cancel();
        });

        let res = run_claude_mock(&worktree, "do stuff", token, Some(tx), None, None).await;
        assert!(
            matches!(res, Err(TakutoError::Cancelled)),
            "expected Cancelled, got {res:?}"
        );
        reset_globals();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn improve_path_returns_two_pane() {
        let _g = test_lock_async().await;
        reset_globals();
        let token = CancellationToken::new();
        let worktree = std::path::PathBuf::from("/tmp/mock-improve");
        let prompt = "Title: Fix login\nThe login button does not work";
        let sp = "You are a technical writer who improves software ticket descriptions.";
        let (sid, output) = run_claude_mock(&worktree, prompt, token, None, None, Some(sp))
            .await
            .expect("mock improve should succeed");
        assert!(sid.starts_with("mock-"));
        assert!(
            output.contains("\n---\n"),
            "improve output should contain the two-pane separator: {output:?}"
        );
        let parts: Vec<&str> = output.splitn(2, "\n---\n").collect();
        assert_eq!(parts.len(), 2);
        assert!(!parts[0].trim().is_empty(), "title pane non-empty");
        assert!(!parts[1].trim().is_empty(), "body pane non-empty");
        reset_globals();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cursor_mock_works_same_shape() {
        let _g = test_lock_async().await;
        reset_globals();
        install_dev_config(&DevConfig {
            mock_agent: false,
            mock_agent_script_path: None,
            mock_agent_line_delay_ms: 5,
            mock_agent_total_ms: 1_000,
        });

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<OutputLine>();
        let token = CancellationToken::new();
        let worktree = std::path::PathBuf::from("/tmp/cursor-mock");
        let (sid, output) = run_cursor_mock(&worktree, "do work", token, Some(tx), None, None)
            .await
            .expect("cursor mock should succeed");
        assert!(sid.starts_with("mock-"));
        assert!(output.contains("[mock-agent/cursor] result ready"));
        // Drain the channel to confirm we emitted lines.
        let mut count = 0;
        while let Ok(_l) = rx.try_recv() {
            count += 1;
        }
        assert!(count > 0);
        reset_globals();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resume_mock_session_prepends_line() {
        let _g = test_lock_async().await;
        reset_globals();
        install_dev_config(&DevConfig {
            mock_agent: false,
            mock_agent_script_path: None,
            mock_agent_line_delay_ms: 5,
            mock_agent_total_ms: 1_000,
        });

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<OutputLine>();
        let token = CancellationToken::new();
        let worktree = std::path::PathBuf::from("/tmp/mock-resume");
        let resume_id = "mock-abcdef";
        let (_sid, _out) = run_claude_mock(&worktree, "p", token, Some(tx), Some(resume_id), None)
            .await
            .expect("resume mock should succeed");
        let mut first_line = String::new();
        while let Ok(l) = rx.try_recv() {
            if first_line.is_empty() {
                first_line = l.content;
                break;
            }
        }
        assert!(
            first_line.contains("Resuming mock session"),
            "expected resume notice as first line, got {first_line:?}"
        );
        reset_globals();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn no_line_tx_still_returns_output() {
        let _g = test_lock_async().await;
        reset_globals();
        install_dev_config(&DevConfig {
            mock_agent: false,
            mock_agent_script_path: None,
            mock_agent_line_delay_ms: 1,
            mock_agent_total_ms: 500,
        });
        let token = CancellationToken::new();
        let worktree = std::path::PathBuf::from("/tmp/mock-no-tx");
        let (_sid, output) = run_claude_mock(&worktree, "p", token, None, None, None)
            .await
            .expect("mock should succeed");
        assert!(!output.is_empty());
        assert!(output.contains("[mock-agent] session complete"));
        reset_globals();
    }
}

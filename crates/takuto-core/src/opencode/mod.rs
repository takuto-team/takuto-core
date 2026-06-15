// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! OpenCode AI agent adapter.
//!
//! Wraps the SST `opencode` CLI in non-interactive JSONL streaming mode
//! (`opencode run --format json …`) and mirrors the shape of
//! [`crate::claude::ClaudeSession`] and [`crate::cursor::CursorSession`].
//!
//! The driver/container/humanizer integration is intentionally NOT performed in
//! this module — a follow-up agent wires it up.
//!
//! # Known limitation — provider authentication
//!
//! `opencode` does not auto-read `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` from
//! the environment. Real provider auth requires either:
//!   * `~/.local/share/opencode/auth.json`, or
//!   * a project-level `opencode.json` config that names a provider.
//!
//! Takuto's `BUNDLE_SOURCING_SH` already exports those env vars from
//! `/run/takuto-secrets/opencode`, but until the integration agent emits a
//! matching `auth.json` (or `opencode.json`), the CLI will likely report
//! "no provider configured". That config-file emission is out of scope for
//! this adapter — this module is only responsible for the CLI invocation.

pub mod session;
pub use session::OpenCodeSession;

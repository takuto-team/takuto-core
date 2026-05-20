// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Codex AI agent adapter.
//!
//! Wraps the OpenAI `codex` CLI in the non-interactive JSONL streaming mode
//! (`codex exec --json …`) and mirrors the shape of [`crate::claude::ClaudeSession`]
//! and [`crate::cursor::CursorSession`].
//!
//! The driver/container/humanizer integration is intentionally NOT performed in
//! this module — a follow-up agent wires it up.

pub mod session;
pub use session::CodexSession;

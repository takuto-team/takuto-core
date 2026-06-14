// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Shared output location for the ts-rs-generated dashboard wire DTOs.
//!
//! The actual `export_all_to(...)` calls live in `#[cfg(test)]` modules next
//! to each DTO (so they reach the private route modules without widening
//! visibility). They all resolve their target through [`generated_dir`] so
//! every file lands in the single committed `ui/src/api/generated/`
//! directory. CI regenerates with `cargo test` and `git diff --exit-code`s
//! that directory; drift between a Rust DTO and the committed TypeScript
//! fails the build. The `ui/src/api/types.ts` barrel re-exports these so
//! frontend imports stay at `@/api/types`.

/// Absolute path to the committed generated-types directory, resolved from the
/// crate manifest so it is independent of the working directory.
#[cfg(test)]
pub(crate) fn generated_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/api/generated")
}

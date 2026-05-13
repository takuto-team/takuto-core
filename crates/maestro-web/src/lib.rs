// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

pub mod auth;
pub mod routes;
pub mod server;
pub mod session_registry;
pub mod state;

// Always-compiled so external integration tests under `tests/` (each its own
// crate) can use the shared helpers. The module contains only inert helpers
// (factories that build temp DBs / app state on demand), so leaving it in the
// production build is cheap and avoids each integration test re-implementing
// the same plumbing.
pub mod test_helpers;

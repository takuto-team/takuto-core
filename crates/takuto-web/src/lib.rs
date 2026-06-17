// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

pub mod auth;
pub mod container_spawner;
pub mod middleware;
pub mod routes;
pub mod server;
pub mod session_registry;
pub mod state;
pub mod ts_bindings;

// Gated out of release builds — see lore/code-quality-principles.md §6
// ("Test scaffolding does not ship in the production crate surface").
//
// - `cfg(test)`             — visible to in-crate unit tests under
//                             `#[cfg(test)] mod tests`.
// - `feature = "test-utils"` — activated by the self dev-dependency in
//                             `Cargo.toml` so external integration tests
//                             under `tests/*.rs` can `use
//                             takuto_web::test_helpers::…`.
//
// `cargo build` / `cargo build --release` do not enable dev-deps and do not
// set `cfg(test)`, so the module is compiled out of every production
// artifact.
#[cfg(any(test, feature = "test-utils"))]
pub mod test_helpers;

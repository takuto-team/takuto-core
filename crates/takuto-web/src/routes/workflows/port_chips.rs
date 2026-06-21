// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Pure derivation of the dashboard's dynamic-port chips from the live state of
//! a workspace container. Kept free of I/O so it can be unit-tested without a
//! Docker daemon — the list handler feeds it the raw `socat` forwards and the
//! set of currently-listening ports it reads from the container.

use std::collections::HashSet;

/// Keep only the `socat` forwards whose target app port is still listening.
///
/// A `socat` started with `fork` keeps its listener alive after the target app
/// exits, so the set of live forwards alone over-reports — a stopped dev server
/// would leave a stale chip on the card. Cross-checking each forward's target
/// against the ports actually listening in the container drops those stale
/// forwards so the chip clears.
///
/// `raw` is a list of `(spare_host_port, target_app_port)` pairs.
pub fn retain_live_forwards(raw: Vec<(u16, u16)>, listening: &HashSet<u16>) -> Vec<(u16, u16)> {
    raw.into_iter()
        .filter(|(_, target)| listening.contains(target))
        .collect()
}

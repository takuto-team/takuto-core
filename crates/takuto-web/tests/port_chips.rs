// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Dashboard port-chip derivation.
//
// The dashboard derives a card's dynamic-port chips from the LIVE state of the
// workspace container: the `socat` forwards actually running, cross-checked
// against the ports actually listening. A `fork` socat keeps its listener alive
// after the forwarded app exits, so reading the forwards alone would leave a
// stale chip on the card after (e.g.) the dev server is stopped. These tests
// pin the cross-check that clears those stale chips.

use std::collections::HashSet;

use takuto_web::routes::workflows::port_chips::retain_live_forwards;

fn listening(ports: &[u16]) -> HashSet<u16> {
    ports.iter().copied().collect()
}

#[test]
fn keeps_forward_whose_target_is_listening() {
    // socat 9110 -> 5173, and 5173 is up: the chip stays.
    let kept = retain_live_forwards(vec![(9110, 5173)], &listening(&[5173, 9100]));
    assert_eq!(kept, vec![(9110, 5173)]);
}

#[test]
fn drops_forward_whose_target_stopped() {
    // The dev server on 5173 was stopped; only the orphaned socat listener
    // (9110) and the IDE (9100) remain. The 5173 chip must clear.
    let kept = retain_live_forwards(vec![(9110, 5173)], &listening(&[9110, 9100]));
    assert!(kept.is_empty());
}

#[test]
fn keeps_only_the_live_targets_in_a_mixed_set() {
    // Two forwards; only one target is still listening.
    let raw = vec![(9110, 5173), (9111, 6006)];
    let kept = retain_live_forwards(raw, &listening(&[6006, 9100]));
    assert_eq!(kept, vec![(9111, 6006)]);
}

#[test]
fn empty_forwards_yields_empty() {
    assert!(retain_live_forwards(vec![], &listening(&[5173])).is_empty());
}

#[test]
fn nothing_listening_drops_everything() {
    let raw = vec![(9110, 5173), (9111, 6006)];
    assert!(retain_live_forwards(raw, &listening(&[])).is_empty());
}

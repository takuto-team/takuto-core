// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Docker label helpers: deterministic container naming and reading values
//! out of a `docker inspect --format '{{json .Config.Labels}}'` blob.

use super::super::sanitize_ticket_key;

/// Return the deterministic editor container name for a ticket.
pub(crate) fn editor_container_name(ticket_key: &str) -> String {
    format!("takuto-editor-{}", sanitize_ticket_key(ticket_key))
}

/// Parse the `takuto.connection_token` value from a Docker inspect JSON labels string.
/// Returns `None` if the label is absent, empty, or the JSON is malformed.
pub fn parse_connection_token_from_labels(json_str: &str) -> Option<String> {
    parse_label_value(json_str, "takuto.connection_token")
}

/// Extract a single label value from a `docker inspect` JSON labels map.
///
/// Returns `None` if the JSON is unparseable, the key is absent, or the
/// value is empty.
pub fn parse_label_value(json_str: &str, key: &str) -> Option<String> {
    let labels: std::collections::HashMap<String, String> = serde_json::from_str(json_str).ok()?;
    let val = labels.get(key)?;
    if val.is_empty() {
        None
    } else {
        Some(val.clone())
    }
}

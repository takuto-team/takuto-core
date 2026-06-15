// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Typed errors for the Jira subsystem.
//!
//! Sub-enum that captures every distinct failure mode produced inside
//! `crates/takuto-core/src/jira/` and the five `actions/{real,dry_run}.rs`
//! near-duplicate `acli` invocations. Lifted from
//! `TakutoError::Jira(String)` per the 2026-05-24 typed-errors-jira spec —
//! every variant cites the call site it replaces so the migration commits can
//! be traced back.
//!
//! Wired into the workspace error envelope via
//! `TakutoError::Jira(#[from] JiraError)` so existing `?` propagation across
//! `Result<T, TakutoError>` boundaries keeps working unchanged.
//!
//! # Foreign `#[from]` policy — none
//!
//! `serde_json::Error` is referenced from four variants
//! (`ParseTicketJson`, `ParseTicketListJson`, `ParseTicketDetailJson`,
//! `ParseLinkedItemJson`); only one `#[from]` per source type is legal under
//! `thiserror`, and each parse site carries distinct context (`key` vs.
//! structural-kind), so all four use `#[source]` + explicit `.map_err(...)`.
//! Mirrors the `GitHubAppError` policy for `jsonwebtoken::errors::Error` /
//! `chrono::format::ParseError`. No `std::io::Error` (acli failures
//! `?`-propagate through `TakutoError::Io` / `Command`). No HTTP-client type
//! (acli is shelled out).

/// Failures originating inside the Jira subsystem. Public for matching, but
/// callers should generally just `?`-propagate into a `TakutoError`.
#[derive(Debug, thiserror::Error)]
pub enum JiraError {
    /// `jira/client.rs:198` — `get_ticket_description_preview` invariant: the
    /// key's project prefix is not in `[jira] project_keys`.
    #[error("ticket {key} is not in configured [jira] project_keys")]
    TicketNotInConfiguredProjects { key: String },

    // ── acli subprocess failures (output.success() == false) ────────────────
    /// `jira/client.rs:178` — `list_todo_tickets_by_rank`.
    #[error("acli list To Do tickets failed: {stderr}")]
    ListTodoFailed { stderr: String },

    /// `jira/client.rs:216` — `get_ticket_description_preview`.
    #[error("acli load ticket {key} failed: {stderr}")]
    GetDescriptionPreviewFailed { key: String, stderr: String },

    /// `jira/client.rs:269` + `actions/real.rs:179` + `actions/dry_run.rs:83`.
    #[error("acli get ticket details for {key} failed: {stderr}")]
    GetDetailsFailed { key: String, stderr: String },

    /// `jira/client.rs:313` + `actions/real.rs:100`.
    #[error("acli assign ticket {key} failed: {stderr}")]
    AssignFailed { key: String, stderr: String },

    /// `jira/client.rs:336` + `actions/real.rs:153`.
    #[error("acli unassign ticket {key} failed: {stderr}")]
    UnassignFailed { key: String, stderr: String },

    /// `jira/client.rs:360` + `actions/real.rs:127`.
    #[error("acli transition ticket {key} to {status} failed: {stderr}")]
    TransitionFailed {
        key: String,
        status: String,
        stderr: String,
    },

    /// `jira/client.rs:384`.
    #[error("acli update description for {key} failed: {stderr}")]
    UpdateDescriptionFailed { key: String, stderr: String },

    /// `jira/client.rs:406`.
    #[error("acli get linked item {key} failed: {stderr}")]
    GetLinkedItemFailed { key: String, stderr: String },

    // ── serde_json parsing of acli `--json` output ──────────────────────────
    /// `jira/client.rs:223` — `get_ticket_description_preview`. Carries `key`
    /// because the parse site already has it bound.
    #[error("failed to parse acli ticket JSON for {key}")]
    ParseTicketJson {
        key: String,
        #[source]
        source: serde_json::Error,
    },

    /// `jira/client.rs:421` — `parse_ticket_list` (no key in scope).
    #[error("failed to parse acli ticket list JSON")]
    ParseTicketListJson {
        #[source]
        source: serde_json::Error,
    },

    /// `jira/client.rs:467` — `parse_ticket_detail` (no key bound at parse).
    #[error("failed to parse acli ticket detail JSON")]
    ParseTicketDetailJson {
        #[source]
        source: serde_json::Error,
    },

    /// `jira/client.rs:589` — `parse_linked_item` (no key bound at parse).
    #[error("failed to parse acli linked item JSON")]
    ParseLinkedItemJson {
        #[source]
        source: serde_json::Error,
    },
}

#[cfg(test)]
mod tests {
    //! Lock-in tests for the typed `JiraError` surface. Two assertions hold the
    //! migration in place:
    //!
    //!   1. The exact `Display` string for every one of the 13 variants —
    //!      mirrors `TakutoError::Jira(String)`'s original free-form messages
    //!      closely enough that consumer string-matching (`routes/jira.rs`
    //!      `msg.contains("not in configured")` → `StatusCode::FORBIDDEN`)
    //!      keeps working unchanged.
    //!   2. The `#[from] JiraError` chain into `TakutoError::Jira(..)` — every
    //!      `?`-propagation across `crates/takuto-core/src/jira/` and the
    //!      `actions/{real,dry_run}.rs` migration sites relies on this exact
    //!      path; if a refactor accidentally wraps via the deprecated
    //!      `JiraStr` shim these tests fail.
    use super::*;
    use crate::error::TakutoError;

    /// Produce a deterministic `serde_json::Error` for tests. The exact Display
    /// is whatever the upstream crate emits for the malformed input — none of
    /// the parse variants interpolate `{source}` into their `#[error(..)]`
    /// template, so the message itself is irrelevant to the lock-in
    /// assertions.
    fn sample_serde_json_error() -> serde_json::Error {
        serde_json::from_str::<serde_json::Value>("{ not json").unwrap_err()
    }

    #[test]
    fn lock_in_jira_error_display() {
        // 1. TicketNotInConfiguredProjects — interpolates {key}. Routes match
        //    "not in configured" → 403 FORBIDDEN; lock that substring in.
        assert_eq!(
            format!(
                "{}",
                JiraError::TicketNotInConfiguredProjects {
                    key: "OTHER-123".to_string()
                }
            ),
            "ticket OTHER-123 is not in configured [jira] project_keys"
        );

        // 2. ListTodoFailed — interpolates {stderr}.
        assert_eq!(
            format!(
                "{}",
                JiraError::ListTodoFailed {
                    stderr: "acli: auth required".to_string()
                }
            ),
            "acli list To Do tickets failed: acli: auth required"
        );

        // 3. GetDescriptionPreviewFailed — interpolates {key} + {stderr}.
        assert_eq!(
            format!(
                "{}",
                JiraError::GetDescriptionPreviewFailed {
                    key: "PROJ-1".to_string(),
                    stderr: "not found".to_string()
                }
            ),
            "acli load ticket PROJ-1 failed: not found"
        );

        // 4. GetDetailsFailed — interpolates {key} + {stderr}.
        assert_eq!(
            format!(
                "{}",
                JiraError::GetDetailsFailed {
                    key: "PROJ-2".to_string(),
                    stderr: "forbidden".to_string()
                }
            ),
            "acli get ticket details for PROJ-2 failed: forbidden"
        );

        // 5. AssignFailed — interpolates {key} + {stderr}.
        assert_eq!(
            format!(
                "{}",
                JiraError::AssignFailed {
                    key: "PROJ-3".to_string(),
                    stderr: "no permission".to_string()
                }
            ),
            "acli assign ticket PROJ-3 failed: no permission"
        );

        // 6. UnassignFailed — interpolates {key} + {stderr}.
        assert_eq!(
            format!(
                "{}",
                JiraError::UnassignFailed {
                    key: "PROJ-4".to_string(),
                    stderr: "not assigned".to_string()
                }
            ),
            "acli unassign ticket PROJ-4 failed: not assigned"
        );

        // 7. TransitionFailed — interpolates {key} + {status} + {stderr}.
        assert_eq!(
            format!(
                "{}",
                JiraError::TransitionFailed {
                    key: "PROJ-5".to_string(),
                    status: "In Progress".to_string(),
                    stderr: "invalid transition".to_string()
                }
            ),
            "acli transition ticket PROJ-5 to In Progress failed: invalid transition"
        );

        // 8. UpdateDescriptionFailed — interpolates {key} + {stderr}.
        assert_eq!(
            format!(
                "{}",
                JiraError::UpdateDescriptionFailed {
                    key: "PROJ-6".to_string(),
                    stderr: "edit conflict".to_string()
                }
            ),
            "acli update description for PROJ-6 failed: edit conflict"
        );

        // 9. GetLinkedItemFailed — interpolates {key} + {stderr}.
        assert_eq!(
            format!(
                "{}",
                JiraError::GetLinkedItemFailed {
                    key: "PROJ-7".to_string(),
                    stderr: "not found".to_string()
                }
            ),
            "acli get linked item PROJ-7 failed: not found"
        );

        // 10. ParseTicketJson — interpolates {key} (not source).
        assert_eq!(
            format!(
                "{}",
                JiraError::ParseTicketJson {
                    key: "PROJ-8".to_string(),
                    source: sample_serde_json_error(),
                }
            ),
            "failed to parse acli ticket JSON for PROJ-8"
        );

        // 11. ParseTicketListJson — static (no interpolation).
        assert_eq!(
            format!(
                "{}",
                JiraError::ParseTicketListJson {
                    source: sample_serde_json_error(),
                }
            ),
            "failed to parse acli ticket list JSON"
        );

        // 12. ParseTicketDetailJson — static (no interpolation).
        assert_eq!(
            format!(
                "{}",
                JiraError::ParseTicketDetailJson {
                    source: sample_serde_json_error(),
                }
            ),
            "failed to parse acli ticket detail JSON"
        );

        // 13. ParseLinkedItemJson — static (no interpolation).
        assert_eq!(
            format!(
                "{}",
                JiraError::ParseLinkedItemJson {
                    source: sample_serde_json_error(),
                }
            ),
            "failed to parse acli linked item JSON"
        );
    }

    #[test]
    fn lock_in_jira_error_into_takuto_error() {
        // Walk every variant through `TakutoError::from(..)` to guarantee the
        // `#[from] JiraError` chain — not the deprecated `JiraStr` shim — is
        // what `?`-propagation hits.

        let cases: Vec<JiraError> = vec![
            JiraError::TicketNotInConfiguredProjects {
                key: "OTHER-1".to_string(),
            },
            JiraError::ListTodoFailed {
                stderr: "stderr".to_string(),
            },
            JiraError::GetDescriptionPreviewFailed {
                key: "PROJ-1".to_string(),
                stderr: "stderr".to_string(),
            },
            JiraError::GetDetailsFailed {
                key: "PROJ-2".to_string(),
                stderr: "stderr".to_string(),
            },
            JiraError::AssignFailed {
                key: "PROJ-3".to_string(),
                stderr: "stderr".to_string(),
            },
            JiraError::UnassignFailed {
                key: "PROJ-4".to_string(),
                stderr: "stderr".to_string(),
            },
            JiraError::TransitionFailed {
                key: "PROJ-5".to_string(),
                status: "Done".to_string(),
                stderr: "stderr".to_string(),
            },
            JiraError::UpdateDescriptionFailed {
                key: "PROJ-6".to_string(),
                stderr: "stderr".to_string(),
            },
            JiraError::GetLinkedItemFailed {
                key: "PROJ-7".to_string(),
                stderr: "stderr".to_string(),
            },
            JiraError::ParseTicketJson {
                key: "PROJ-8".to_string(),
                source: sample_serde_json_error(),
            },
            JiraError::ParseTicketListJson {
                source: sample_serde_json_error(),
            },
            JiraError::ParseTicketDetailJson {
                source: sample_serde_json_error(),
            },
            JiraError::ParseLinkedItemJson {
                source: sample_serde_json_error(),
            },
        ];
        assert_eq!(cases.len(), 13, "must cover every JiraError variant");

        for err in cases {
            let wrapped: TakutoError = err.into();
            assert!(
                matches!(
                    wrapped,
                    TakutoError::Jira(
                        JiraError::TicketNotInConfiguredProjects { .. }
                            | JiraError::ListTodoFailed { .. }
                            | JiraError::GetDescriptionPreviewFailed { .. }
                            | JiraError::GetDetailsFailed { .. }
                            | JiraError::AssignFailed { .. }
                            | JiraError::UnassignFailed { .. }
                            | JiraError::TransitionFailed { .. }
                            | JiraError::UpdateDescriptionFailed { .. }
                            | JiraError::GetLinkedItemFailed { .. }
                            | JiraError::ParseTicketJson { .. }
                            | JiraError::ParseTicketListJson { .. }
                            | JiraError::ParseTicketDetailJson { .. }
                            | JiraError::ParseLinkedItemJson { .. }
                    )
                ),
                "expected TakutoError::Jira(JiraError::<variant>), got {wrapped:?}"
            );
        }
    }
}

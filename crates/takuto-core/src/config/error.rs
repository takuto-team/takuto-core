// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Typed errors for what `TakutoError::Config(String)` had become — a true
//! catch-all bag spanning config-file validation, workflow state-machine
//! guards, master-key bootstrap, AEAD seal/open, worker-secrets bundle
//! construction, and assorted operational misc.
//!
//! Replaces the historical `TakutoError::Config(String)` catch-all
//! (the `*Str(String)` deprecated shim was removed in the post-§8 #2
//! cleanup PR). Each variant captures structured operation context where
//! it's meaningful; a handful of variants accept a `detail: String` payload
//! for genuinely free-form text (validation messages from third-party crates,
//! AEAD operator errors, parser diagnostics) — documented deviation from the
//! "no String payload" architecture rule given the 111-site scope and the
//! catch-all nature of the original variant.
//!
//! See `lore/audits/2026-05-21-clean-code.md` §8 #2 and
//! `lore/audits/2026-05-24-typed-errors-spec.md` for the architecture rules
//! this module follows.

use std::path::PathBuf;

use thiserror::Error;

/// Failures originating from config-file validation, workflow state-machine
/// guards, master-key + AEAD primitives, worker-secrets bundle construction,
/// and assorted operational paths previously bagged into
/// `TakutoError::Config(String)`.
#[derive(Debug, Error)]
pub enum ConfigError {
    // ── Config file validation (`config/{load, patches, agent}.rs`) ──────
    /// Structured config validation failure. `section`+`field` pin the source;
    /// `detail` carries the third-party validator message (typically a CLI
    /// hint or a foreign error's Display) that the operator sees.
    #[error("[{section}] {field}: {detail}")]
    Validation {
        section: &'static str,
        field: &'static str,
        detail: String,
    },

    /// `toml::ser::to_string(<Config>)` failed during `config.to_toml_string()`
    /// (config_writer round-trip).
    #[error("Failed to serialize config: {source}")]
    SerializeToml {
        #[source]
        source: toml::ser::Error,
    },

    // ── Workflow state machine (`workflow/engine/*`, `workflow/snapshot.rs`) ──
    /// Workflow not present in the engine map for the supplied ticket key.
    #[error("Workflow not found: {ticket_key}")]
    WorkflowNotFound { ticket_key: String },

    /// State-machine guard: the workflow is not in a state that supports
    /// the requested operation.
    #[error("Cannot {op} workflow in state: {current_state}")]
    InvalidWorkflowState {
        op: &'static str,
        current_state: String,
        ticket_key: String,
    },

    /// Dynamic workflow definition (`*.toml` under `workflows/`) not found
    /// in the discovered set.
    #[error("Workflow definition '{def_name}' not found in {dir}")]
    DefinitionNotFound { def_name: String, dir: PathBuf },

    /// Discovered TOML had a parse / validation problem.
    #[error("Workflow definition '{def_name}' is invalid: {reason}")]
    DefinitionInvalid { def_name: String, reason: String },

    /// A def-run for `(def_name, ticket_key)` is already `Running`.
    #[error("Workflow definition '{def_name}' is already running for {ticket_key}")]
    DefinitionAlreadyRunning {
        def_name: String,
        ticket_key: String,
    },

    /// `depends_on` upstreams have not all reached `Completed`.
    #[error("Dependencies not met for workflow definition '{def_name}'")]
    DefinitionDependenciesNotMet { def_name: String },

    /// Retry on a def-run that has no run state, or whose state isn't `Error`.
    #[error(
        "Cannot retry workflow definition '{def_name}': current state is '{current_state}', expected 'error'"
    )]
    DefinitionRetryWrongState {
        def_name: String,
        current_state: String,
    },

    /// Retry-from-zero invoked before the def has ever run.
    #[error("Workflow definition '{def_name}' has no run state for {ticket_key}")]
    DefinitionNoRunState {
        def_name: String,
        ticket_key: String,
    },

    /// Docker daemon unreachable on a path that requires container isolation.
    #[error(
        "Docker daemon is not available. DinD is required for workflow isolation. Set [docker] mode = \"dind\" or run with DOCKER_HOST pointing at a reachable daemon."
    )]
    DockerUnavailable,

    /// Snapshot read/write/parse failure (workflow/snapshot.rs).
    #[error("Workflow snapshot {op}: {detail}")]
    Snapshot { op: &'static str, detail: String },

    // ── Master key bootstrap (`auth/master_key.rs`) ──────────────────────
    /// `TAKUTO_SECRET_KEY` env var is not valid hex.
    #[error("TAKUTO_SECRET_KEY is not valid hex: {source}")]
    MasterKeyHex {
        #[source]
        source: hex::FromHexError,
    },

    /// `TAKUTO_SECRET_KEY` decoded to the wrong number of bytes.
    #[error("TAKUTO_SECRET_KEY decoded to wrong length")]
    MasterKeyLength,

    /// On-disk master keyfile I/O failure (`op` = "read"|"write"|"fsync"|"size"|"perm").
    #[error("Master keyfile {op} failed for {path}: {detail}")]
    MasterKeyFile {
        op: &'static str,
        path: PathBuf,
        detail: String,
    },

    /// Master key unavailable (env var unset + no keyfile + degraded mode).
    #[error("master_key_unavailable: cannot unseal worker secrets")]
    MasterKeyUnavailable,

    /// CSPRNG (`getrandom::fill`) failure while drawing key/DEK/nonce bytes.
    #[error("CSPRNG failure during {op}: {source}")]
    Csprng {
        op: &'static str,
        #[source]
        source: getrandom::Error,
    },

    // ── AEAD envelope encryption (`auth/seal.rs`) ─────────────────────────
    /// AEAD encrypt failed (XChaCha20-Poly1305 over plaintext or DEK).
    #[error("AEAD encrypt({op}) failed: {detail}")]
    AeadEncrypt { op: &'static str, detail: String },

    /// AEAD decrypt failed (wrong key, tampered ciphertext, malformed nonce).
    #[error("AEAD decrypt({op}) failed: {detail}")]
    AeadDecrypt { op: &'static str, detail: String },

    /// Sealed-blob byte layout failed a length / version sanity check.
    #[error("Sealed blob malformed: {detail}")]
    SealMalformed { detail: String },

    // ── Worker secrets bundle (`auth/bundle.rs`) ──────────────────────────
    /// `tempfile::tempdir_in` failed creating the bundle's tmpfs root.
    #[error("failed to create bundle tempdir: {source}")]
    BundleTempdir {
        #[source]
        source: std::io::Error,
    },

    /// `OpenOptions::new()…open(path)` / `file.write_all(...)` failed for a
    /// per-bundle secret file. `op` = "create" | "write".
    #[error("failed to {op} secret file {path}: {detail}")]
    BundleSecretFile {
        op: &'static str,
        path: PathBuf,
        detail: String,
    },

    /// DB lookup during bundle construction (provider/github credential rows,
    /// cli_state row, etc.). `op` names the lookup.
    #[error("Bundle DB lookup ({op}) failed: {detail}")]
    BundleDbLookup { op: &'static str, detail: String },

    /// `AiAgentProvider::parse(&auth_pin.provider)` rejected the persisted
    /// provider identifier.
    #[error("auth_pin.provider invalid: {detail}")]
    BundleProviderInvalid { detail: String },

    /// Claude CLI state row malformed during bundle build (claude_session
    /// blob parse).
    #[error("Claude CLI state {op}: {detail}")]
    BundleClaudeState { op: &'static str, detail: String },

    // ── Last-resort catch-all ─────────────────────────────────────────────
    /// Operational failure that does not fit any structured variant above.
    /// `op` is a pinned `&'static str` operation label; `detail` carries the
    /// operator-visible context. Sites that land here are candidates for a
    /// future split into structured variants when the patterns stabilise.
    #[error("{op}: {detail}")]
    Operational { op: &'static str, detail: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::ser::Error as _;

    fn io_err() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied")
    }

    fn getrandom_err() -> getrandom::Error {
        getrandom::Error::UNSUPPORTED
    }

    #[test]
    fn lock_in_config_error_display_samples() {
        // Spot-check a representative sample (not all 25 variants — this is
        // the largest sub-enum and the prior phases' exhaustive Display
        // tests have shrinking marginal value once the architecture is set).
        // The `From` test below pins all 25 variants for envelope coverage.
        let cases: Vec<(ConfigError, String)> = vec![
            (
                ConfigError::Validation {
                    section: "agent",
                    field: "providers.cursor.cli",
                    detail: "value must not be empty".to_string(),
                },
                "[agent] providers.cursor.cli: value must not be empty".to_string(),
            ),
            (
                ConfigError::WorkflowNotFound {
                    ticket_key: "PROJ-1".to_string(),
                },
                "Workflow not found: PROJ-1".to_string(),
            ),
            (
                ConfigError::InvalidWorkflowState {
                    op: "pause",
                    current_state: "Done".to_string(),
                    ticket_key: "PROJ-1".to_string(),
                },
                "Cannot pause workflow in state: Done".to_string(),
            ),
            (
                ConfigError::DefinitionNotFound {
                    def_name: "review".to_string(),
                    dir: PathBuf::from("/workflows"),
                },
                "Workflow definition 'review' not found in /workflows".to_string(),
            ),
            (ConfigError::MasterKeyLength, "TAKUTO_SECRET_KEY decoded to wrong length".to_string()),
            (
                ConfigError::MasterKeyUnavailable,
                "master_key_unavailable: cannot unseal worker secrets".to_string(),
            ),
            (
                ConfigError::Csprng {
                    op: "generate DEK",
                    source: getrandom_err(),
                },
                format!("CSPRNG failure during generate DEK: {}", getrandom_err()),
            ),
            (
                ConfigError::DockerUnavailable,
                "Docker daemon is not available. DinD is required for workflow isolation. Set [docker] mode = \"dind\" or run with DOCKER_HOST pointing at a reachable daemon.".to_string(),
            ),
            (
                ConfigError::BundleTempdir { source: io_err() },
                format!("failed to create bundle tempdir: {}", io_err()),
            ),
            (
                ConfigError::Operational {
                    op: "github poller",
                    detail: "boom".to_string(),
                },
                "github poller: boom".to_string(),
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(format!("{err}"), expected, "Display mismatch for {err:?}");
        }
    }

    #[test]
    fn lock_in_config_error_into_takuto_error_all_variants() {
        use crate::error::TakutoError;
        let cases: Vec<ConfigError> = vec![
            ConfigError::Validation {
                section: "x",
                field: "y",
                detail: "".to_string(),
            },
            ConfigError::SerializeToml {
                source: toml::ser::Error::custom("x"),
            },
            ConfigError::WorkflowNotFound {
                ticket_key: "x".to_string(),
            },
            ConfigError::InvalidWorkflowState {
                op: "pause",
                current_state: "Done".to_string(),
                ticket_key: "x".to_string(),
            },
            ConfigError::DefinitionNotFound {
                def_name: "x".to_string(),
                dir: PathBuf::from("/"),
            },
            ConfigError::DefinitionInvalid {
                def_name: "x".to_string(),
                reason: "".to_string(),
            },
            ConfigError::DefinitionAlreadyRunning {
                def_name: "x".to_string(),
                ticket_key: "y".to_string(),
            },
            ConfigError::DefinitionDependenciesNotMet {
                def_name: "x".to_string(),
            },
            ConfigError::DefinitionRetryWrongState {
                def_name: "x".to_string(),
                current_state: "Running".to_string(),
            },
            ConfigError::DefinitionNoRunState {
                def_name: "x".to_string(),
                ticket_key: "y".to_string(),
            },
            ConfigError::DockerUnavailable,
            ConfigError::Snapshot {
                op: "read",
                detail: "".to_string(),
            },
            ConfigError::MasterKeyHex {
                source: hex::decode("zz").unwrap_err(),
            },
            ConfigError::MasterKeyLength,
            ConfigError::MasterKeyFile {
                op: "read",
                path: PathBuf::from("/"),
                detail: "".to_string(),
            },
            ConfigError::MasterKeyUnavailable,
            ConfigError::Csprng {
                op: "x",
                source: getrandom_err(),
            },
            ConfigError::AeadEncrypt {
                op: "x",
                detail: "".to_string(),
            },
            ConfigError::AeadDecrypt {
                op: "x",
                detail: "".to_string(),
            },
            ConfigError::SealMalformed {
                detail: "".to_string(),
            },
            ConfigError::BundleTempdir { source: io_err() },
            ConfigError::BundleSecretFile {
                op: "create",
                path: PathBuf::from("/"),
                detail: "".to_string(),
            },
            ConfigError::BundleDbLookup {
                op: "x",
                detail: "".to_string(),
            },
            ConfigError::BundleProviderInvalid {
                detail: "".to_string(),
            },
            ConfigError::BundleClaudeState {
                op: "x",
                detail: "".to_string(),
            },
            ConfigError::Operational {
                op: "x",
                detail: "".to_string(),
            },
        ];
        // Drift detection: 26 variants total — bump on add/remove.
        assert_eq!(cases.len(), 26);
        for err in cases {
            let outer: TakutoError = err.into();
            assert!(
                matches!(outer, TakutoError::Config(_)),
                "expected TakutoError::Config, got {outer:?}"
            );
        }
    }
}

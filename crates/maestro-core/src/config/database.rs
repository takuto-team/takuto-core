// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Plan-11 step 5 — `[database]` config block.
//!
//! Source: `tmp/plan-11-pluggable-database-backends.md` §5.
//!
//! When `connection` is empty (or the section is omitted entirely), Maestro
//! keeps the legacy "SQLite at {data_dir}/maestro.db" behaviour. Setting it
//! to a `postgres://`/`postgresql://`/`mysql://`/`sqlite://` URL switches the
//! deployment to that backend.
//!
//! `PUT /api/config` does NOT allow patching this section — backend changes
//! are restart-only, same policy as `web.host`/`web.port`.

use serde::{Deserialize, Serialize};

/// `[database]` section of `config.toml`. Drives [`Database::connect`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// Connection URL. Empty / omitted → SQLite at `{data_dir}/maestro.db`.
    ///
    /// Supported schemes:
    ///   - `sqlite://path/to/file.db`
    ///   - `postgres://user:pw@host:5432/db`
    ///   - `postgresql://user:pw@host:5432/db` (alias)
    ///   - `mysql://user:pw@host:3306/db` (covers MariaDB)
    pub connection: String,

    /// Optional pool tuning. `None` keeps sqlx defaults (10 connections,
    /// 30 s acquire timeout, 10 min idle timeout).
    pub max_connections: Option<u32>,
    pub acquire_timeout_secs: Option<u64>,
    pub idle_timeout_secs: Option<u64>,

    /// When true, the engine refuses to boot if the database is unreachable
    /// at startup. When false, it logs a warning and falls back to SQLite
    /// for the current process. Default: true.
    #[serde(default = "default_true")]
    pub fail_fast: bool,

    /// When true and a local SQLite file exists at `{data_dir}/maestro.db`
    /// AND the remote target has no `import_complete` marker, perform a
    /// one-shot data import on startup. Default: true. The importer is
    /// implemented in plan-11 §8 (separate cluster).
    #[serde(default = "default_true")]
    pub import_from_sqlite: bool,
}

fn default_true() -> bool {
    true
}

impl Default for DatabaseConfig {
    /// Mirror the serde defaults: empty connection (→ default SQLite),
    /// no pool tuning, `fail_fast` and `import_from_sqlite` both on.
    fn default() -> Self {
        Self {
            connection: String::new(),
            max_connections: None,
            acquire_timeout_secs: None,
            idle_timeout_secs: None,
            fail_fast: true,
            import_from_sqlite: true,
        }
    }
}

impl DatabaseConfig {
    /// Trim and return the connection URL, treating whitespace-only values
    /// as "empty" so an accidental `connection = "  "` doesn't slip past as
    /// a malformed URL.
    pub fn connection_url(&self) -> &str {
        self.connection.trim()
    }

    /// True when no operator-supplied URL was set — Database uses the
    /// default SQLite-at-data_dir behaviour.
    pub fn is_default_sqlite(&self) -> bool {
        self.connection_url().is_empty()
    }
}

/// Redact the password component of a connection URL for safe logging /
/// `GET /api/config` exposure.
///
/// - `postgres://user:pw@host/db` → `postgres://user:****@host/db`
/// - `postgres://user@host/db`    → unchanged (no password)
/// - `sqlite://path`              → unchanged (no userinfo)
/// - `not-a-url`                  → unchanged
pub fn redact_connection_password(url: &str) -> String {
    // The pattern we want: `<scheme>://<userinfo>@<rest>` where userinfo is
    // `user:password`. Standard URL parsing crates would do this in two
    // lines, but pulling `url` here for one helper isn't worth it. Manual
    // scan is short and explicit.
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let rest_start = scheme_end + 3;
    let Some(at_rel) = url[rest_start..].find('@') else {
        return url.to_string();
    };
    let at_abs = rest_start + at_rel;
    let userinfo = &url[rest_start..at_abs];
    let Some(colon_rel) = userinfo.find(':') else {
        // `user@host` — no password to redact.
        return url.to_string();
    };
    let user_end = rest_start + colon_rel;
    format!("{}:****{}", &url[..user_end], &url[at_abs..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty_and_recognised_as_default_sqlite() {
        let cfg = DatabaseConfig::default();
        assert!(cfg.is_default_sqlite());
        assert_eq!(cfg.connection_url(), "");
        assert!(cfg.fail_fast);
        assert!(cfg.import_from_sqlite);
    }

    #[test]
    fn whitespace_only_connection_treated_as_default() {
        let cfg = DatabaseConfig {
            connection: "  \t\n  ".to_string(),
            ..Default::default()
        };
        assert!(cfg.is_default_sqlite());
    }

    #[test]
    fn redact_postgres_password() {
        assert_eq!(
            redact_connection_password("postgres://maestro:s3cret@db.local:5432/maestro"),
            "postgres://maestro:****@db.local:5432/maestro"
        );
    }

    #[test]
    fn redact_keeps_url_without_password() {
        assert_eq!(
            redact_connection_password("postgres://maestro@db.local/maestro"),
            "postgres://maestro@db.local/maestro"
        );
    }

    #[test]
    fn redact_passes_through_sqlite_url() {
        assert_eq!(
            redact_connection_password("sqlite:///var/lib/maestro/maestro.db"),
            "sqlite:///var/lib/maestro/maestro.db"
        );
    }

    #[test]
    fn redact_passes_through_non_url_string() {
        assert_eq!(redact_connection_password(""), "");
        assert_eq!(redact_connection_password("not a url"), "not a url");
    }

    #[test]
    fn redact_only_replaces_password_portion() {
        // The `:` in the host:port section must survive — only the
        // userinfo `user:pw@` is touched.
        assert_eq!(
            redact_connection_password("mysql://u:p@h:3306/db"),
            "mysql://u:****@h:3306/db"
        );
    }
}

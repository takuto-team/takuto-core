// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! SQLite-backed persistence for multi-user authentication and access control.
//!
//! The database file lives at `{data_dir}/maestro.db`. Schema migrations run
//! automatically on first open via [`Database::open`].

pub mod credentials;
pub mod migration;
pub mod models;
pub mod schema;
pub mod users;

use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::error::{MaestroError, Result};

/// Thread-safe database handle. Wraps a `rusqlite::Connection` in an `Arc<Mutex<>>`
/// so it can be shared across async tasks via `spawn_blocking`.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl Database {
    /// Open (or create) the database at `{data_dir}/maestro.db` and run migrations.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir).map_err(|e| {
            MaestroError::Database(format!(
                "Failed to create data directory {}: {e}",
                data_dir.display()
            ))
        })?;

        let db_path = data_dir.join("maestro.db");
        let conn = rusqlite::Connection::open(&db_path)?;

        // Enable WAL mode for better concurrent read performance.
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        schema::run_migrations(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open an in-memory database for testing.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        schema::run_migrations(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Get a reference to the inner connection (for use with `spawn_blocking`).
    pub fn conn(&self) -> &Arc<Mutex<rusqlite::Connection>> {
        &self.conn
    }
}

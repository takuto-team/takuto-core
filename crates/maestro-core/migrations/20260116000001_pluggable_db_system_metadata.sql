-- system_metadata table.
--
-- A small key/value store on the target backend for one-shot operation
-- markers. The importer writes `import_complete` here on successful
-- SQLite→remote copy; future one-shots (e.g. v2 importer variants,
-- post-deploy clean-ups) can reuse the table by reserving their own
-- key prefix.
--
-- The migration runs on every backend so SQLite deployments also get
-- the table — they never use the `import_complete` row (no import
-- needed) but the schema stays in lockstep across backends, which
-- keeps the dialect-aware migration source's "one file = one DDL set"
-- contract intact.

CREATE TABLE IF NOT EXISTS system_metadata (
    key VARCHAR(64) PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at BIGINT NOT NULL
);

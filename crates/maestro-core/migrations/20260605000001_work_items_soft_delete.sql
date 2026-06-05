-- Soft-delete for work items + run history.
--
-- Deleting a work item (dashboard Delete, or Mark-as-Done removal) now marks
-- the row with a `deleted_at` timestamp instead of removing it, so past runs
-- are retained as history. Re-adding the same ticket creates a NEW row for a
-- brand-new run; the previous run stays in the table, flagged deleted.
--
-- The original UNIQUE (workspace_name, ticket_key) index is therefore dropped:
-- multiple rows for the same ticket must coexist (one live, plus any number of
-- soft-deleted historical runs). A plain (non-unique) index of the same name
-- is recreated so the by-ticket lookups keep their covering index. Live reads
-- already pick the most-recently-started row, so disambiguation is unchanged;
-- they additionally filter `deleted_at IS NULL`.

ALTER TABLE work_items ADD COLUMN deleted_at BIGINT;

DROP INDEX IF EXISTS idx_work_items_workspace_key;
CREATE INDEX IF NOT EXISTS idx_work_items_workspace_key
    ON work_items(workspace_name, ticket_key);

CREATE INDEX IF NOT EXISTS idx_work_items_deleted_at
    ON work_items(deleted_at);

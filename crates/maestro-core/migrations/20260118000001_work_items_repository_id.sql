-- Carry `repository_id` on `work_items` so the
-- `require_workflow_access` policy check can run against the DB row
-- directly without consulting the in-memory `Workflow`. The column
-- is nullable to match `Workflow::repository_id: Option<String>` —
-- legacy workflows have no repo association and that must stay
-- a permitted state.
--
-- No FK to `repositories(id)` because that would force a
-- back-fill story for orphan rows (repository deleted while the
-- work item still exists). The reconciler in `db/repositories` is
-- already responsible for cleaning up dangling refs.

ALTER TABLE work_items ADD COLUMN repository_id VARCHAR(64);

CREATE INDEX idx_work_items_repository
    ON work_items(repository_id);

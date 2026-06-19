-- Per-(user, workspace) report toggle. When enabled, each workflow flow run
-- for this workspace contributes its own section to the work item's report
-- (`lore/reports/<key>_report.md`); re-running a flow replaces only that
-- flow's section. Replaces the global `[general] generate_report` flag as the
-- source of truth for report generation (the global flag is no longer read by
-- the engine). Defaults off so existing workspaces are unchanged.
--
-- Boolean stored as INTEGER 0/1 (the project's portable convention; see
-- `users.suspended`). NOT NULL DEFAULT 0 back-fills existing rows.

ALTER TABLE user_worktree_commands ADD COLUMN generate_report INTEGER NOT NULL DEFAULT 0;

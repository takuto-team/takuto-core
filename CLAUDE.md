# Claude / AI agent instructions — Maestro

## Read `AGENTS.md` first (new session)

At the **start of every new session** that involves this codebase, **read `AGENTS.md` at the repository root before any other project documentation or exploratory reading** (`README.md`, `ARCHITECTURE.md`, crate sources, etc.). It is the **first** file you should load for Maestro context.

Exception: purely local edits where the user has pinned an exact file and no project context is needed (e.g. a trivial typo). Otherwise, **always start with `AGENTS.md`**.

## After that

Use **`AGENTS.md`** as the accurate map of architecture, workflows, Claude integration, and HTTP/WebSocket behavior. Use `README.md`, `ARCHITECTURE.md`, and `docs/workflow.md` for human setup, troubleshooting, and diagrams.

## Keep `AGENTS.md` current

Whenever you implement changes that affect anything documented in `AGENTS.md` — workflow behavior, Claude integration, config schema, REST/WebSocket contracts, crate layout, Jira polling, or external action boundaries — **update `AGENTS.md` in the same task** so it stays correct.

Skip updates only for changes that do not alter documented behavior (e.g. comments, trivial renames with no API impact).

# Claude / AI agent instructions — Maestro

## Read `AGENTS.md` first (new session)

At the **start of every new session** that involves this codebase, **read `AGENTS.md` at the repository root before any other project documentation or exploratory reading** (`README.md`, `ARCHITECTURE.md`, crate sources, etc.). It is the **first** file you should load for Maestro context.

Exception: purely local edits where the user has pinned an exact file and no project context is needed (e.g. a trivial typo). Otherwise, **always start with `AGENTS.md`**.

## After that

Use **`AGENTS.md`** as the accurate map of architecture, workflows, Claude integration, and HTTP/WebSocket behavior. Use `README.md`, `ARCHITECTURE.md`, and `docs/workflow.md` for human setup, troubleshooting, and diagrams.

Read **`CODING_STANDARDS.md`** before writing any code. It defines the SOLID, Rust, React/TypeScript, and security rules every contributor must follow. These rules are non-negotiable — no exceptions without an inline comment explaining why.

## Keep `AGENTS.md` current

Whenever you implement changes that affect anything documented in `AGENTS.md` — workflow behavior, Claude integration, config schema, REST/WebSocket contracts, crate layout, Jira polling, or external action boundaries — **update `AGENTS.md` in the same task** so it stays correct.

Skip updates only for changes that do not alter documented behavior (e.g. comments, trivial renames with no API impact).

## Comments

**Default to writing no comments.** Most code does not need a comment. A reader can already see what the code does; a well-named function or variable is worth more than a paragraph above it. Add a comment only when it carries information the reader cannot derive from the code itself: a hidden constraint, a subtle invariant, a workaround tied to a specific bug, or behavior that would genuinely surprise someone.

**Never reference internal planning artifacts in code, log messages, error strings, or user-facing text.** That includes:

- Plan documents (`Plan-NN`, `plan-NN`, `tmp/plan-XX-*.md`, etc.)
- Slice / step / phase / wave numbers (`slice 14`, `step 4`, `Phase 2b.3`)
- Issue or task identifiers (`Task #47`, `GH-45`, `AC-2`)
- Wording like "as part of plan-07 step 6 the engine will…" — the code is the artifact, not the plan it came from

Plans are scaffolding that exists only during a feature's incubation. The artifacts they leave behind in the source — comments, log lines, error messages, doc strings, UI copy — outlive the plans by years and become noise to everyone who reads the code afterwards. Describe what the code does and why, **not the planning history that produced it**.

If you need to record context about a change, the commit message and the PR description are the right places. Do not smear them across the source tree.

Apply the same rule to commit messages going forward when reasonable — the code itself should not require knowing what slice introduced it.

## Migrations are immutable

Once a migration file under `crates/maestro-core/migrations/` has been merged and applied to any environment, **never edit the file again — not even comments, whitespace, or trailing newlines**. sqlx stores a SHA256 checksum of each migration in `_sqlx_migrations` at apply time and refuses to boot when the on-disk content drifts (`migration X was previously applied but has been modified`).

This rule overrides every other "scrub", "rename", or "tidy" instruction. If you find unwanted content in an applied migration:

- Live with it — the file is part of the DB's contract, not the prose surface.
- Or write a follow-up migration that does what you wanted (rename a column, drop an index) the proper way.

The same prohibition applies to bulk find/replace tools, automated formatters, and agents: when sweeping comments, ticket references, or anything else across the tree, **exclude `crates/maestro-core/migrations/` from the sweep**.

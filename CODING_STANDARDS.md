# Coding Standards — Takuto

Rules for every AI agent and human contributor. Short, enforceable, no exceptions without a comment.

---

## 1 · SOLID

### Single Responsibility
- One file = one reason to change. Name it after what it **does**.
- **Rust:** when a file exceeds ~300 lines of non-test logic, split it into a `module/` directory with a thin `mod.rs` facade (see `workflow/engine/`).
- **React:** extract a sub-component when a component exceeds ~150 lines **or** mixes two unrelated concerns.

### Open / Closed
- Add new behaviour by **adding** code (new trait impl, new component variant, new TOML workflow step), not by modifying existing logic paths.
- **Rust:** use `match` exhaustiveness — never add a wildcard `_` arm just to silence a warning; handle every variant explicitly so future additions force a compiler error.

### Liskov Substitution
- Every `impl Trait` must honour the full trait contract — no panics, no silent no-ops, no partial implementations.
- **React:** component variants that share a prop type must be interchangeable at the call site.

### Interface Segregation
- **Rust:** split a trait the moment a caller only uses half of it. Prefer many small traits over one fat one.
- **React/TS:** pass only the props a child actually uses. No "pass everything down" objects.

### Dependency Inversion
- **Rust:** depend on `Arc<dyn Trait>`, not on concrete structs. All external side-effects live behind `ExternalActions` or an equivalent trait.
- **React:** components receive data and callbacks as props. No direct imports of global singletons inside a component.

---

## 2 · Rust

### Errors
- **No `.unwrap()` or `.expect()` in non-test code.** Use `?` or explicit `match`.
- Define errors with `thiserror`. Never expose `Box<dyn Error>` in a public API.
- Log at the handling site, not the origination site.

### Ownership & concurrency
- Prefer `&T` / `&mut T`; reach for `Arc<T>` only when shared ownership is genuinely required.
- **Never hold a `RwLock` or `Mutex` guard across an `.await`.** Lock, extract the value, drop the guard, then await.
- Shared mutable state: `Arc<RwLock<T>>`. Channels (`tokio::sync::mpsc`, `broadcast`) for cross-task communication.

### Async
- Never call blocking I/O inside `async fn`. Use `tokio::task::spawn_blocking` when you must.
- Keep `async fn` bodies short; factor pure synchronous logic into plain `fn`.

### Modules & visibility
- Follow the `engine/` pattern: large modules become directories; `mod.rs` is a thin facade that delegates to focused sub-modules.
- `pub(crate)` by default for internal items; `pub` only at true crate boundaries.
- Tests go in `#[cfg(test)] mod tests { … }` at the bottom of the same file they test.

### Quality bar
- `cargo build` must produce **zero warnings** before any commit.
- All public non-trivial items get a `///` doc comment.
- Prefer `match` over chains of `if let`; prefer `?` over nested `match Err`.
- No dead code, no commented-out code, no `todo!()` left in merged PRs.

---

## 3 · React / TypeScript

### Components
- One component per file; filename = component name (PascalCase).
- No inline logic in JSX — extract to a named variable or function above the `return`.
- No component that both fetches data **and** renders UI. Split into a data hook + pure rendering component.

### TypeScript
- **`strict: true`.** No `any`, no `as unknown as X` without an explanatory comment.
- All API shapes live in `src/api/types.ts`. Never inline anonymous object types for API data.
- Use `interface` for object shapes, `type` for unions, mapped types, and aliases.

### State & side-effects
- Colocate state as close to its consumer as possible.
- `useEffect` only for genuine side-effects (subscriptions, DOM imperatives). Never for derived values — derive inline or with `useMemo`.
- Extract reusable stateful logic into a `use*` custom hook.

### Props
- Destructure at the top of the component. Never `props.xxx` in the body.
- Required props first, optionals last (marked `?`).
- Never pass the parent's entire state object to a child.

### Quality bar
- `npm run build` must produce **zero TypeScript errors** before any commit.
- No `console.log` left in merged code.
- No `@ts-ignore` or `@ts-expect-error` without a comment explaining why it's safe.

---

## 4 · Security

### Input validation
- Validate and sanitise **every** input at system boundaries (API endpoints, CLI args, config file).
- Reject unexpected fields: use `#[serde(deny_unknown_fields)]` on request bodies where schema is strict.
- Check content-type and size before processing uploads or large payloads.

### Secrets & credentials
- **Zero hardcoded secrets, tokens, or passwords** anywhere in source. Read from environment variables only.
- Never log secret values, tokens, or PII — even at `DEBUG` level.
- Verify no secrets appear in diffs before committing (`git diff --staged`).

### Command execution
- **Never interpolate user input into a shell string.** Pass arguments as an array (`Command::arg()` in Rust; avoid `sh -c "… {input} …"`).
- Allowlist permitted commands; reject anything outside the allowlist.

### Authentication & authorisation
- Authenticate every endpoint that touches state or data.
- Authorise server-side on every request — never trust a client-supplied identity claim.
- Prefer short-lived tokens; rotate secrets on exposure.

### Output & injection
- Escape all user-controlled values rendered in HTML. Never use `dangerouslySetInnerHTML`.
- Parameterise all database queries — no string-concatenated SQL.

### Dependencies
- Run `cargo audit` and `npm audit` before each release; block on high-severity findings.
- Commit both `Cargo.lock` and `package-lock.json`; review dependency changes in PRs.

### Logging
- Log what the system **did**, not what the user **sent**. Strip credentials and PII.
- Use structured logging (`tracing` macros in Rust). No `println!` in production paths.

---

## 5 · General rules (all agents)

- **Read before you write.** Always read the file you are about to edit; never patch from memory.
- **Minimum viable change.** Solve exactly the stated problem. No unsolicited refactoring, no bonus features, no speculative abstractions.
- **One logical change per commit.** Commits must build and pass tests independently.
- **No regression.** `cargo test --workspace` and `npm run build` must be green after every change.
- **Update docs.** If a change affects `AGENTS.md`-documented behaviour, update `AGENTS.md` in the same task.
- **Ask before destructive actions.** Deleting files, force-pushing, or dropping data requires explicit user confirmation.

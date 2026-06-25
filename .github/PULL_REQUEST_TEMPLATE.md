> **Note:** Takuto is **not currently accepting external code contributions.**
> Accepting external contributions requires CLA infrastructure that is not
> yet in place. Pull requests from outside the maintainer team will be closed.
> Please open a [GitHub Issue](https://github.com/takuto-team/takuto-core/issues)
> or [Discussion](https://github.com/takuto-team/takuto-core/discussions) instead,
> and report security vulnerabilities privately per [`SECURITY.md`](../SECURITY.md).
> See [`CONTRIBUTING.md`](../CONTRIBUTING.md) for the full policy.

---

## What

<!-- 1–3 sentences. What does this change? -->

## Why

<!-- 1–3 sentences. What problem does it solve? Link to issue. -->

## Test plan

<!-- Checkboxes. Be specific. -->
- [ ] Unit tests added/updated
- [ ] Manual test path documented
- [ ] `AGENTS.md` updated if behaviour/contracts changed

## Checklist (maintainer PRs)

- [ ] Commits signed off (DCO — `git commit -s`)
- [ ] License headers on new files (FSL)
- [ ] `cargo fmt && cargo clippy && cargo test` pass locally
- [ ] `cd ui && npm run lint && npm test && npm run build` pass locally
- [ ] No secrets in commit (gitleaks)

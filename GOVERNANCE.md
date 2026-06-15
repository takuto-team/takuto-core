# Governance

## Current model

Takuto is currently maintained by **@morphet81**.

- **Response SLA:** best-effort within 1 week on issues and PRs.
- **Decisions:** the maintainer has final say on architecture and roadmap. Major
  changes (new public APIs, schema migrations, breaking config changes) are
  discussed in GitHub Discussions before implementation.
- **Reviews:** all PRs require maintainer approval. CI must pass.
- **Releases:** versioned per `VERSION`. Container images published on `v*` tags.

## Adding maintainers

Currently solo. Revisited once the external-contribution channel opens —
see [`CONTRIBUTING.md`](./CONTRIBUTING.md).

## Roadmap

Tracked in GitHub Issues with the `roadmap` label. See README for the current
priorities.

## Reporting security issues

Security vulnerabilities follow a separate private process — see
[`SECURITY.md`](./SECURITY.md). Do not file security issues on the public
tracker.

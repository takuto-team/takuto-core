# Security policy

## Supported versions

Takuto is in early development. Security fixes are applied to the latest
released minor only.

| Version | Supported |
| ------- | --------- |
| Latest minor (see `VERSION`) | ✅ |
| Older minors | ❌ |

## Reporting a vulnerability

**Do not** open public GitHub issues for security vulnerabilities.

Two private reporting channels:

1. **GitHub Security Advisories** (preferred) — file a private advisory at
   <https://github.com/takuto-team/takuto-core/security/advisories/new>.
2. **Email** — `morphet.contact@gmail.com` until a project domain is registered.

For sensitive reports, request a PGP key over the same channels.

We aim to acknowledge reports within **3 business days** and to disclose a
fix or mitigation within **90 days** of receipt. Disclosure timing is
coordinated with the reporter.

## Scope

In scope:

- Authentication, session management, and authorisation in the Takuto web
  layer (`crates/takuto-web`).
- Container and egress isolation boundaries (`docker/egress-rules.sh`,
  workflow container lifecycle).
- Reverse-proxy token leakage (`/s/*` path-token registry).
- Prompt-injection escapes from Jira/GitHub content reaching agent
  containers (`{ticket_context}` framing).
- Stored XSS or HTML injection in the dashboard.

Out of scope:

- Issues that require physical access to the host or an already-compromised
  Takuto account.
- Self-inflicted issues from disabling `cors_origins`, setting
  `allow_all_https`, or running with `cookie_secure = false` over HTTPS.
- Vulnerabilities in third-party agent runtimes (Claude Code, Cursor Agent) —
  report those upstream.
- Denial of service caused by misconfiguring `max_concurrent_workflows`
  beyond the host's resource budget.

## Trust model

Takuto is **multi-user, single-tenant**. All users on one instance share
the same Jira, GitHub, and AI credentials configured at deployment.

Any user you grant an account to can execute code in worker containers
under the deployment's identity — including any tokens stored in
`takuto.env`. **Do not grant accounts to users you do not trust.**

Admin privileges are scoped to user management (create, edit, suspend,
delete users, and change shared polling / config settings). Admins cannot
read other users' workflows.

## Defence-in-depth defaults

- Branch protection on `main` is **required** — see README "Security and
  operations". Agents push branches and open PRs, never commit to `main`.
- Egress firewall restricts agent containers to a fixed allowlist plus
  `[network] extra_egress_hosts`.
- Session cookies are `HttpOnly`; `Secure` is auto-detected from
  `cors_origins` or `X-Forwarded-Proto`.
- After 5 failed login or recovery attempts in 10 min the account is
  temporarily locked; per-IP rate limit on `/api/auth/login` is 10/min.

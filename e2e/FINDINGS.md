# E2E findings — application issues

Running log of product issues surfaced while building/running the e2e suite.
**No application source is edited** to fix these — each is reproduced from the
harness and described here.

---

## Summary

**Suite status: green.** Two spec groups exercise the implement-workflow surface
end-to-end against a real stack (DinD, real agent CLIs, a mock LM Studio). Part B
is driven **entirely through the dashboard UI** (real Playwright clicks), as a
user would.

| Group | Spec | Result |
|-------|------|--------|
| Part A — agent-CLI reachability | `tests/agent-reachability.spec.ts` | **4/4 pass** — claude / cursor / opencode / codex each install, run `--version`, and reach an auth/model failure with no token. |
| Part B — opencode implement-workflow (UI-driven) | `tests/workflow-implement.spec.ts` | **6/6 pass** — full user journey via clicks. |

Part B per-test outcome (all via UI clicks):

| Test | What it verifies | Result |
|------|------------------|--------|
| Onboarding | wizard: add repo, select opencode + mock `base_url` + bearer key, ticketing = None | ✅ pass |
| Config | flow editor creates a one-step flow; Repository Settings sets `npm ci` + `npm run dev` | ✅ pass |
| Run flow (B1+B2) | create work item, click the flow → opencode → mock → **completed** badge; **`node_modules` persists** in the bootstrapped worktree | ✅ pass |
| B3/B6 | start `npm run dev` from the card → port forwards → **open it from the card** → the Vite app renders | ✅ pass |
| B4 | open the IDE (openvscode) from the card → reachable via `/s/` proxy | ✅ pass |
| B5 | open the terminal (ttyd) from the card → reachable via `/s/` proxy | ✅ pass |

**Application findings (not fixed — app source untouched):**

| ID | Severity | One-liner | Status |
|----|----------|-----------|--------|
| **F1** | fixed | Unpinned ("latest") Cursor install version regex was stale → install failed with a **blank** error. | **FIXED + e2e-verified** — unpinned now defers to Cursor's official installer; `run_shell` surfaces stdout so errors are never blank. Part A runs unpinned (4/4). Deployments must rebuild the image to pick it up. |
| **F2** | — | Worktree pre-create race (mechanism present in code). | **Does NOT reproduce via the real UI** — was an artifact of the prior API-first harness. Downgraded. |
| **F3** | — | Def-run not finalized → run-commands `409`. | **Not reproduced**; conditional/unconfirmed. |

*(A previously-listed "F4 — no UI to add a local repo without GitHub" was **withdrawn**: the app behaves correctly — when GitHub auth is missing it surfaces explicit "configure a GitHub App / add a PAT" guidance (`routes/repos.rs:85`, onboarding banner). Takuto is GitHub-oriented; the suite's purely-local fixture repo is outside that supported flow, so the harness associates it via the API as a test convenience — not an app defect.)*

---

## F1 — Unpinned ("latest") Cursor install regex is stale — FIXED

**Status: FIXED (this session) + e2e-verified unpinned.** The fix
(`crates/takuto-core/src/agent_install.rs`):
- **Unpinned** now defers to Cursor's official installer
  (`curl https://cursor.com/install | bash`) run with `HOME` pointed at the
  tools dir (`/opt/takuto-tools`), then symlinks `bin/{agent,cursor-agent}` to
  the resolved binary — no more brittle self-parsing of a non-contractual
  version string. **Pinned** still downloads the versioned tarball directly.
- `run_shell` now surfaces **stdout when stderr is empty**, so a failing piped
  installer can no longer reach the UI as a blank `Cursor Agent: ` error.
- The e2e harness no longer pins Cursor by default
  (`CURSOR_VERSION` empty unless `TAKUTO_E2E_CURSOR_VERSION` is set), so Part A
  (`agent-reachability`) installs all four agents via their default/latest path
  and asserts each is runnable — **4/4 pass unpinned**.
- A deployment hitting the old blank error must **rebuild the image** to include
  the new binary (a cached Rust layer would skip it).

The original defect, for the record:

**Severity (original):** real code defect; only fired when an **unpinned** Cursor
install actually ran the version scrape against a fresh tools volume. Deployments that **pin** `[agent.providers.cursor]
version`, ship Cursor baked in the image, or already have it in the persistent
`takuto-tools` volume never execute the broken branch. (This is why a working
deployment installs Cursor "with no error" — the scrape isn't being run.) The
e2e harness forces a clean install into a fresh tools volume, which is precisely
why it pins the version to work around this.

**Verified empirically (this session):** fetching the live `https://cursor.com/install`
and applying the code's regex
`[0-9]{4}\.[0-9]{2}\.[0-9]{2}-[0-9-]+-[0-9a-f]+` matches **nothing** — the script
now serves `2026.06.26-7079533` (`YYYY.MM.DD-<build>`, a single trailing
segment), while the regex requires two (`-<build>-<hash>`).

**Where:** `crates/takuto-core/src/agent_install.rs` → `Installer::cursor_install`.

**Root cause:** the unpinned ("latest") path resolves the version by scraping
`https://cursor.com/install` with:

```
grep -oE '[0-9]{4}\.[0-9]{2}\.[0-9]{2}-[0-9-]+-[0-9a-f]+'
```

That regex expects the **retired** `YYYY.MM.DD-<build>-<hash>` shape (two
dash-separated trailing segments). Cursor now ships `YYYY.MM.DD-<build>` — a
single trailing segment (observed: `2026.06.26-7079533`). The regex matches
nothing, so `version` is empty.

Because the resolution runs inside `version="$(curl … | grep … | head -n1)"`
under `set -euo pipefail`, the empty `grep` makes the pipeline exit non-zero and
**`set -e` aborts the script before** the explicit
`[ -n "$version" ] || { echo "could not resolve latest cursor version"; exit 1; }`
guard is ever reached. So `run_shell` returns failure with an **empty stderr**,
and the install error shown is just `Cursor Agent: ` (blank).

**Repro (inside a takuto container with network):**
```
curl -fsSL https://cursor.com/install \
  | grep -oE '[0-9]{4}\.[0-9]{2}\.[0-9]{2}-[0-9-]+-[0-9a-f]+' | head -n1
# → prints nothing (no match)

# The actual current version appears in the script's DOWNLOAD_URL line:
curl -fsSL https://cursor.com/install | grep -oE 'lab/[0-9.-]+/' | head -n1
# → lab/2026.06.26-7079533/
```

**Suggested fix (product side, not applied here):**
- Make the trailing hash segment optional and tolerate the new shape, e.g.
  `[0-9]{4}\.[0-9]{2}\.[0-9]{2}-[0-9A-Za-z.-]+`, or parse the version out of the
  script's `DOWNLOAD_URL=.../lab/<version>/...` line directly.
- When resolution yields nothing, **report the unparsed output** instead of a
  blank error — restructure so the guard runs before `set -e` aborts.

**Scope:** only affects the unpinned cold-install path. A deployment that pins
the Cursor version or pre-installs Cursor never hits it.

**Harness workaround (in place, harness-only — no app edit):** the
`installAgents` stack seed pins `[agent.providers.cursor] version` (constant
`CURSOR_VERSION` in `e2e/src/docker/stack.ts`, override
`TAKUTO_E2E_CURSOR_VERSION`, default `2026.06.26-7079533`).

---

## F4 — WITHDRAWN (not a defect)

An earlier draft listed "no UI to add a reconciled local repository without
GitHub" as a finding. **Withdrawn after verifying the code:** the app behaves
correctly. When GitHub auth is missing it surfaces explicit guidance —
`crates/takuto-web/src/routes/repos.rs:85` returns *"GitHub authentication
unavailable — add a personal access token on the GitHub settings tab, or
configure a GitHub App"*, and the dashboard shows a *"GitHub authentication is
not configured … Set GitHub PAT →"* banner (`docker_hooks/status.rs:148`). The
"Available repositories" list is intentionally GitHub-sourced
(`GET /api/github/repos`); Takuto is GitHub-oriented and the supported path is to
configure a GitHub App/PAT, after which repos become addable in the UI.

The suite's fixture is a purely-**local** reconciled repo with **no GitHub** at
all — outside that supported flow — so the harness associates it once via the
API in `beforeAll` (`addExistingRepository`) as a **test convenience**. This is
not evidence of a missing affordance; documented inline in
`e2e/src/pages/OnboardingSteps.ts` and the spec.

---

## F2 — Worktree pre-create race (does NOT reproduce via the UI)

**Status:** **downgraded.** The earlier API-first spec reported `node_modules`
being wiped after a def run, attributed to a race between the background
`prepare_worktree_for_ticket` pre-create and the run-time bootstrap. Re-tested
through the **real dashboard UI** (create the work item, then click the flow at
human pace), **`node_modules` persists** in the bootstrapped worktree — the B1
check now passes as a normal assertion (no `xfail`).

**Conclusion:** the wipe was an **artifact of the API-first harness**, which
started the flow programmatically and promptly via
`POST …/run-workflow/{def}` immediately after `start-manual`, producing a
create/bootstrap ordering the UI does not generate. Under the UI's normal
"add work item → (worktree pre-creates) → click flow" sequence, the init product
survives.

**Code mechanism (still present, latent):**
`add_to_dashboard` spawns `prepare_worktree_for_ticket`
(`lifecycle.rs:327`), which unconditionally calls `create_worktree` →
`clear_worktree_path_for_recreate` (`git worktree remove --force`, deleting
untracked files) without checking `worktree_bootstrapped` first
(`bootstrap.rs:97`, `actions/real.rs:258`). It is not exercised destructively by
the UI path, but a no-op-when-already-bootstrapped guard would still be a sound
hardening. Not pursued further since it is not reproducible via the product UI.

---

## F3 — Def-run not finalized → run-commands `409` (NOT reproduced)

**Status:** conditional / unconfirmed. With the suite's single, dependency-free
flow the run-command path works (B3/B6 pass), so the `409` was **not
reproduced**. May be flow-shape-specific (e.g. flows with `depends_on`). Flagged
for a future spec with a multi-def flow; not a confirmed defect here.

---

## Harness notes (test-infra, not application bugs)

| Note | Detail | Status |
|------|--------|--------|
| origin-remote gap | `seedFixtureRepo` produced a repo without an `origin` remote the worktree bootstrap expected. | fixed in harness (self-referential origin) |
| in-DinD tar cleanup | temp tar artifacts left inside the DinD container after image load. | cleanup added |
| single-session-per-user | the server allows one active session per user. | Part B runs the whole journey on **one persistent logged-in page** (`describe.serial`) — the natural fit. |
| dashboard info modal | a "No Ticketing System" modal (shown when `ticketing_system = none`) overlays the add-item controls. | `DashboardPage.goto` dismisses it ("Got it"). |
| seeded default flow | the per-user flows store seeds a default "Implement" flow for a fresh workspace, so the editor uses a **unique** flow name to avoid a name collision. | spec uses `E2E Implement`. |
| overflow measurer | `WorkflowDefButtons` renders an off-screen `inert` measurer copy of every flow button. | badge locators scoped to `:visible`. |
| build-cache / disk | disk-full incident mid-development (multi-GB build cache + leftover DinD volumes). | cleaned; manage disk between runs |
| Cursor version pin | F1 worked around by pinning `[agent.providers.cursor] version`. | `TAKUTO_E2E_CURSOR_VERSION` (default `2026.06.26-7079533`) |

---

## Coverage note

Part B is driven through the **dashboard UI with real Playwright clicks** end to
end: the onboarding wizard (repo confirm, provider/base_url/key, ticketing), the
Config settings (flow editor, init/run commands), work-item creation, running the
flow, starting the dev run-command and opening its forwarded port from the card,
and opening the IDE and terminal from the card. The dashboard page objects
(`e2e/src/pages/{DashboardPage,PasteDescriptionModal,WorkItemCard,OnboardingWizard,
OnboardingSteps,ConfigPage}.ts`) are all **in use**.

The **one** non-UI step is associating the fixture repo (API in `beforeAll`),
because the fixture is a purely-local repo with no GitHub — outside the UI's
GitHub-oriented add flow (see the F4 note; not a defect). The IDE/terminal
proxied documents are fetched through the same authenticated session after the
card produces their `/s/` URLs.

---

## How to run

**Part A — agent-CLI reachability** (needs the agent install / `installAgents`):
```
cd e2e && TAKUTO_E2E_BACKENDS=sqlite npx playwright test agent-reachability
```

**Part B — opencode implement-workflow** (DinD; **serial / `workers=1` mandatory**):
```
cd e2e && TAKUTO_E2E_WORKERS=1 TAKUTO_E2E_BACKENDS=sqlite npx playwright test workflow-implement
```

Env knobs: `TAKUTO_E2E_CURSOR_VERSION` (pin Cursor — see F1),
`TAKUTO_E2E_BUILD_TARGET`, `TAKUTO_E2E_BACKENDS`, `TAKUTO_E2E_WORKERS`.

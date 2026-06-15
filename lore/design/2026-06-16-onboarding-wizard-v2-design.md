# Onboarding wizard v2 — design and layout guide

Audience: the frontend developer implementing the four enhancements. This doc
specifies labels, helper text, placement, Tailwind classes, and existing
components to reuse. Do not invent new design patterns — everything here is
already present in the codebase.

---

## Design vocabulary (what already exists)

| Pattern | Where | Key classes |
|---|---|---|
| Step card | `Onboarding.tsx` outer `div` | `bg-gray-900 border border-gray-800 rounded-xl p-6` |
| Sub-section header | `GitHubStep.tsx` PAT block | `text-sm font-semibold text-gray-300 mb-1` |
| Section description | same | `text-xs text-gray-500 mb-3` |
| Inert info card | `GitHubStep.tsx` GitHub App card | `bg-gray-950/60 border border-gray-800 rounded-lg p-4 text-sm text-gray-300` |
| Label | `TicketingStep.tsx` | `block text-xs text-gray-400 mb-1` |
| Text input | `TicketingStep.tsx` | `w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200` |
| Mono input | same (Jira token/site) | append `font-mono` |
| Input + validation error border | (follow TicketingStep pattern) | swap `border-gray-700` → `border-red-500` |
| Inline validation message | (not yet in wizard; add inline) | `text-xs text-red-400 mt-1` |
| Helper text | `TicketingStep.tsx` hint `p` | `text-xs text-gray-500 mt-1` |
| Horizontal divider + new section | `TicketingTab.tsx` polling gate | `border-t border-gray-800 pt-6` wrapper `div` |

---

## 1 · Step 3 — "Git & GitHub" (`GitHubStep.tsx`)

### Label change

In `Stepper.tsx`, index 3 entry:

- `title`: `"GitHub integration"` → **`"Git & GitHub"`**
- `body`: `"Connect a GitHub App for shared access, or bring your own personal token."` → **`"Set the git branch and remote Takuto works from, then connect a GitHub App or personal token."`**

### Layout

```
┌─ GitHubStep (flex flex-col gap-4) ───────────────────────────────────────┐
│                                                                            │
│  ┌─ Git sub-section (div, flex flex-col gap-3) ──────────────────────┐   │
│  │  <h3>  Git settings  </h3>                                         │   │
│  │  <p>   Helper line   </p>                                          │   │
│  │                                                                    │   │
│  │  [label] Base branch                                               │   │
│  │  [input  value="main"  placeholder="main"]                        │   │
│  │  [helper text]                                                     │   │
│  │                                                                    │   │
│  │  [label] Remote                                                    │   │
│  │  [input  value="origin"  placeholder="origin"]                    │   │
│  │  [helper text]                                                     │   │
│  └────────────────────────────────────────────────────────────────────┘   │
│                                                                            │
│  ┌─ GitHub App card (existing, bg-gray-950/60 border …) ─────────────┐   │
│  │  GitHub App: configured / not configured                           │   │
│  │  [description + link]                                              │   │
│  └────────────────────────────────────────────────────────────────────┘   │
│                                                                            │
│  <h3> Your personal access token (optional) </h3>                         │
│  <GitHubCredentialsSection />                                              │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

### Specification

**Git sub-section wrapper** — `<div className="flex flex-col gap-3">`

**Sub-section heading** — `<h3 className="text-sm font-semibold text-gray-300 mb-1">Git settings</h3>`

**Sub-section description** — `<p className="text-xs text-gray-500 mb-3">The branch Takuto checks out for each work item, and the git remote it pushes to.</p>`

**Base branch field**

| Attribute | Value |
|---|---|
| Label | `Base branch` |
| Input id | `onb-git-base-branch` |
| Type | `text` |
| Default / pre-populated | value from `GET /api/config` → `git.base_branch`; fall back to `"main"` |
| Placeholder | `main` |
| Helper text | `The branch work-item branches are cut from. Usually "main" or "master".` |
| Validation | Required — block Continue and show `"Base branch is required."` in `text-xs text-red-400 mt-1` when blank |
| Classes | standard text input (no `font-mono`) |

**Remote field**

| Attribute | Value |
|---|---|
| Label | `Remote` |
| Input id | `onb-git-remote` |
| Type | `text` |
| Default / pre-populated | value from `GET /api/config` → `git.remote`; fall back to `"origin"` |
| Placeholder | `origin` |
| Helper text | `The git remote Takuto fetches from and pushes branches to.` |
| Validation | Required — same block/message pattern as base branch |
| Classes | standard text input |

**Save behaviour** — when Continue is clicked, call `PUT /api/config` (or the git sub-route; confirm with the backend team) with `{ git: { base_branch, remote } }` before advancing. Skip does not call the API.

**Existing content below** — the GitHub App status card and the PAT section stay in place, in the same order as today. No visual change to them.

---

## 2 · Ticketing step — item polling (`TicketingStep.tsx` + `TicketingTab.tsx` pattern)

### Layout

```
┌─ Wizard step card ──────────────────────────────────────────────────────┐
│                                                                          │
│  ┌─ TicketingStep (existing) ──────────────────────────────────────┐   │
│  │  [system selector]                                              │   │
│  │  (Jira card when system === "jira")                             │   │
│  └─────────────────────────────────────────────────────────────────┘   │
│                                                                          │
│  ── shown only when isAdmin && system !== "none" ──────────────────     │
│  <div className="border-t border-gray-800 pt-6">                        │
│    <ItemPollingSettingsSection />                                        │
│  </div>                                                                  │
│                                                                          │
│  [← Back]                            [Skip for now]  [Continue →]       │
└──────────────────────────────────────────────────────────────────────────┘
```

### Specification

**Gate** — the exact same condition used in `TicketingTab.tsx`:

```ts
const showPolling = !loading && !!isAdmin && ticketing.system !== "none";
```

The check must react to the **live `ticketing.system` selection**, not to the last-saved value, so selecting "None" hides the section immediately (AC-P6).

**Wrapper** — mirror `TicketingTab.tsx` exactly:

```tsx
{showPolling && (
  <div className="border-t border-gray-800 pt-6">
    <ItemPollingSettingsSection />
  </div>
)}
```

**Component** — `ItemPollingSettingsSection` is a self-contained section with its own `<h2>Item polling</h2>` heading, load state, and internal Save button. Drop it in unchanged; no props needed.

**Interaction model** — the polling section saves independently via its own Save button (`PUT /api/config/polling`). The wizard step's Continue / Skip buttons save only the ticketing system selection (unchanged from today). Unsaved polling edits are abandoned on Skip or Continue (AC-P5).

**Wizard-specific copy difference** — add a brief contextual note directly above the `ItemPollingSettingsSection` divider (inside the `pt-6` block, before the section itself) to set expectations:

```tsx
<p className="text-xs text-gray-500 mb-4">
  These settings control how Takuto picks up new work items automatically.
  Use the Save button below to apply changes — they are independent of the
  Continue button above.
</p>
```

This note is absent from the Config tab (where the context is obvious). It is only needed in the wizard.

---

## 3 · Workflows step — step timeout (`FlowsTab`, step 4)

### Layout

```
┌─ Wizard step card ───────────────────────────────────────────────────────┐
│                                                                           │
│  ┌─ Timeout sub-section (div, flex flex-col gap-3) ──────────────────┐  │
│  │  <h3>  Step timeout  </h3>                                         │  │
│  │  [label] Timeout (seconds)                                         │  │
│  │  [number input  default=1800]                                      │  │
│  │  [helper text]                                                     │  │
│  └────────────────────────────────────────────────────────────────────┘  │
│                                                                           │
│  <div className="border-t border-gray-800 pt-4">                         │
│    <FlowsTab />                                                           │
│  </div>                                                                   │
│                                                                           │
│  [← Back]                          [Skip for now]  [Finish setup →]      │
└───────────────────────────────────────────────────────────────────────────┘
```

### Specification

Place the timeout block **above** `FlowsTab`. Separate the two with a divider so the flows list feels clearly distinct.

**Timeout sub-section wrapper** — `<div className="flex flex-col gap-3">`

**Sub-section heading** — `<h3 className="text-sm font-semibold text-gray-300 mb-1">Step timeout</h3>`

**Timeout field**

| Attribute | Value |
|---|---|
| Label | `Timeout (seconds)` |
| Input id | `onb-step-timeout` |
| Type | `number` |
| `min` attr | `1` |
| Default / pre-populated | value from `GET /api/config` → `agent.step_timeout_secs`; fall back to `1800` |
| Placeholder | `1800` |
| Helper text | `Maximum seconds an agent step may run before it is cancelled. Default 1800 (30 min).` |
| Validation | Must be a positive integer — block Finish and show `"Step timeout must be a positive number."` in `text-xs text-red-400 mt-1` when blank or ≤ 0 |
| Width | `max-w-xs` — the field should not stretch to full column width |
| Classes | standard text input + `max-w-xs` |

**Save behaviour** — on Finish, call `PUT /api/config/agent` with `{ step_timeout_secs: <parsed int> }` before the `POST /api/onboarding/complete` call. Skip does not call the API.

**FlowsTab divider**

```tsx
<div className="border-t border-gray-800 pt-4">
  <FlowsTab />
</div>
```

`pt-4` (not `pt-6`) because `FlowsTab` has its own internal spacing. Adjust to `pt-6` if it looks tight after visual review.

---

## 4 · First-run experience — copy and affordances

### When the wizard auto-launches (no config.toml)

The wizard auto-launches when `GET /api/onboarding/status` returns a first-run state. No extra UI chrome is needed for this path — the existing stepper + step card is sufficient.

**Step 1 (Ticketing) body text** — update from:

> `"Pick where Takuto should read tasks from. You can change this later."`

to:

> `"Welcome — this is Takuto's first-time setup. Pick where it should read tasks from. You can change any of these settings from the Configuration page later."`

This applies only when the wizard was auto-launched on first boot. If a flag distinguishing auto-launch from user-triggered (`/onboarding`) is available in the page state, show the extended copy only on auto-launch. If no such flag exists at implementation time, the extended copy is safe to show always (it reads naturally in both contexts).

**No extra banner or modal** — avoid adding a dedicated "first run" banner. The step body copy is sufficient context for an operator reading the page.

### Finish screen — database and port note

On the final step (step 4) the Finish button label is already `"Finish setup"`. Add a note above the nav controls — between the `FlowsTab` and the `[← Back] / [Skip for now] / [Finish setup]` row:

```
┌─ note block ──────────────────────────────────────────────────────────┐
│  bg-gray-950/60 border border-gray-800 rounded-lg p-3 text-xs        │
│  text-gray-400                                                        │
│                                                                       │
│  ℹ  Database and dashboard port are not configured here.              │
│     Takuto writes a config.toml with sensible defaults (SQLite,       │
│     port 8080). Edit that file directly to change them.               │
└───────────────────────────────────────────────────────────────────────┘
```

**Component placement** — inside the step card, between the step body and the `<div className="flex justify-between …">` nav row. Render it unconditionally when `step === 4` (it is relevant whether or not config.toml existed before).

**Copy** (exact string):

> **Database and dashboard port are not configured in this wizard.**
> Takuto writes a `config.toml` with the defaults (SQLite, port 8080). Edit that file directly to change them.

Use `<strong>` for the first sentence and inline `<code className="font-mono">` for `config.toml`.

**"Re-run from Settings" affordance** — at wizard completion (after `POST /api/onboarding/complete` redirects to the dashboard), no dedicated affordance is needed in the wizard itself. The existing `"Skip setup →"` link in the wizard header already lets the operator bail at any time. A link to Settings from the dashboard is out of scope for this ticket.

---

## Tailwind class quick-reference

```
Input (text/number):
  w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200

Input — validation error:
  w-full bg-gray-950 border border-red-500 rounded-lg px-3 py-2 text-sm text-gray-200

Input — max-width (timeout only):
  max-w-xs  (prepend to or wrap the input div)

Label:
  block text-xs text-gray-400 mb-1

Helper text:
  text-xs text-gray-500 mt-1

Validation message:
  text-xs text-red-400 mt-1

Sub-section heading (h3):
  text-sm font-semibold text-gray-300 mb-1

Sub-section description:
  text-xs text-gray-500 mb-3

Info / note card:
  bg-gray-950/60 border border-gray-800 rounded-lg p-3 text-xs text-gray-400

Divider + new section:
  border-t border-gray-800 pt-6   (or pt-4 before FlowsTab)
```

---

## Component reuse checklist

| New UI element | Reuse / source |
|---|---|
| Git inputs | Inline in `GitHubStep.tsx` — same pattern as `TicketingStep` inputs |
| Polling section | Drop in `<ItemPollingSettingsSection />` unchanged |
| Timeout input | Inline in the step-4 block in `Onboarding.tsx` |
| Finish note | Inline JSX in `Onboarding.tsx` (only rendered when `step === 4`) |
| Stepper label | Edit `ONBOARDING_STEPS[2]` in `Stepper.tsx` |

No new components need to be created.

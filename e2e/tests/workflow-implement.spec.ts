import { expect, type BrowserContext, type Page } from '@playwright/test';
import { test } from '../src/fixtures/admin.fixture.js';
import type { TakutoStack, DindInfo } from '../src/fixtures/stack.fixture.js';
import { OnboardingApi } from '../src/api/client.js';
import { WorkflowApi } from '../src/api/workflows.js';
import { OnboardingSteps } from '../src/pages/OnboardingSteps.js';
import { ConfigPage } from '../src/pages/ConfigPage.js';
import { DashboardPage } from '../src/pages/DashboardPage.js';
import { WorkItemCard } from '../src/pages/WorkItemCard.js';

/**
 * Part B — the opencode implement-workflow driven entirely through the
 * dashboard UI (real Playwright clicks), as a user would: onboarding wizard,
 * Config settings, work-item creation, running the flow, and the interactive
 * surface (run-command → forwarded port opened from the card, IDE, terminal).
 *
 * One persistent, logged-in `page` is shared across the ordered tests
 * (`describe.serial`) so the whole journey runs in a single browser session —
 * which also satisfies the server's single-session-per-user rule. Worker-scoped
 * `stack` + `dind` are captured in `beforeAll`; the tests take no test-scoped
 * fixtures (requesting `page`/`admin` would open a second session).
 */

/** Marker the fixture app renders in its served HTML (kept in sync with
 *  `fixtures/react-app`). Inlined — the fixture is a separate TS program. */
const APP_MARKER = 'takuto-e2e-react-app';

/** Mirrors the stack seed (`src/docker/stack.ts`). */
const MOCK_MODEL = 'mock-model';
// Unique name so the flow editor does not collide with the default "Implement"
// flow the per-user store seeds for a fresh workspace.
const FLOW_NAME = 'E2E Implement';
const STEP_NAME = 'Reply';
const DEV_COMMAND = { name: 'dev', command: 'npm run dev' };
const INIT_COMMAND = 'npm ci';

const WORKFLOW_TIMEOUT_MS = 360_000;
const SURFACE_TIMEOUT_MS = 180_000;

test.use({ dind: true });

let context: BrowserContext;
let page: Page;
let stackRef: TakutoStack;
let dind: DindInfo;
let baseURL = '';

/** Resolved facts about the single work item, captured as the journey runs. */
let ticketKey = '';
let worktreePath = '';
let mockHit = false;
let nodeModulesExit = -1;

function requireDind(stack: TakutoStack): DindInfo {
  if (!stack.dind) {
    throw new Error('Part B requires a DinD stack (test.use({ dind: true }))');
  }
  return stack.dind;
}

/** Capture the new tab opened by an `target="_blank"` dashboard link. */
async function openPopup(trigger: () => Promise<void>): Promise<Page> {
  const [popup] = await Promise.all([page.waitForEvent('popup'), trigger()]);
  await popup.waitForLoadState('domcontentloaded');
  return popup;
}

test.describe.serial('opencode implement-workflow via the dashboard UI (Part B)', () => {
  test.beforeAll(async ({ browser, stack, adminCreds }) => {
    stackRef = stack;
    dind = requireDind(stack);
    baseURL = stack.baseURL;

    context = await browser.newContext({ baseURL, ignoreHTTPSErrors: true });
    page = await context.newPage();

    // Register + log in the first admin via the API so the session cookie lands
    // in the page's context (auth setup only — every later step is UI-driven).
    const onboard = new OnboardingApi(page.request, baseURL);
    await onboard.bootstrapAdmin(adminCreds);

    // Associate the reconciled fixture repo with the admin via the API. This is
    // the one unavoidable non-UI step: the dashboard's "Available repositories"
    // list is sourced from GitHub-accessible repos (added by clone URL), so a
    // locally-reconciled repo with no GitHub backing cannot be added through the
    // UI (see FINDINGS F4). Everything after this is click-driven.
    const api = new WorkflowApi(page.request, baseURL);
    const mine = await api.listRepositories();
    if (!mine.some((r) => r.name === dind.workspaceName)) {
      const available = await api.listAvailableRepositories();
      const found = available.find((r) => r.name === dind.workspaceName);
      if (!found) {
        throw new Error(`fixture repo "${dind.workspaceName}" not registered`);
      }
      await api.addExistingRepository(found.id);
    }
  });

  test.afterAll(async () => {
    await context?.close();
  });

  test('repo listing surfaces GitHub-auth guidance when GitHub is unconfigured', async () => {
    // The stack has no GitHub App / PAT. The repo-listing endpoint that backs the
    // dashboard "Available repositories" UI must return actionable guidance (tell
    // the user to add a PAT / configure a GitHub App) rather than a silent empty
    // list — confirming a local repo with no GitHub is a guided state, not a gap.
    const res = await page.request.get(`${baseURL}/api/github/repos`);
    expect(res.status(), 'no GitHub auth → error, not an empty list').toBe(502);
    expect((await res.text()).toLowerCase()).toMatch(/personal access token|github app/);
  });

  test('onboarding: add the repo and configure opencode via the wizard', async () => {
    test.setTimeout(120_000);
    const steps = new OnboardingSteps(page);
    await steps.goto();
    await steps.runFullWizard({
      repository: dind.workspaceName,
      baseUrl: dind.mockBaseUrl,
      model: MOCK_MODEL,
      bearer: 'lm-studio',
    });
    // Finishing onboarding navigates away from /onboarding to the dashboard.
    await expect(page).toHaveURL((u) => !u.pathname.startsWith('/onboarding'));
  });

  test('config: create the flow + worktree commands via the settings UI', async () => {
    test.setTimeout(120_000);
    const config = new ConfigPage(page);
    await config.goto();
    await config.createSingleStepFlow({
      flowName: FLOW_NAME,
      stepName: STEP_NAME,
      prompt: 'Reply with a single short confirmation sentence.',
    });
    await config.setWorktreeCommands({
      initCommand: INIT_COMMAND,
      runName: DEV_COMMAND.name,
      runCommand: DEV_COMMAND.command,
    });
  });

  test('dashboard: create a work item and run the implement flow (opencode → mock)', async () => {
    test.setTimeout(WORKFLOW_TIMEOUT_MS + 120_000);
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.addWorkItem({
      name: 'impl-e2e-ui',
      description: 'Drive a single opencode step against the mock LM Studio via the UI.',
    });

    // Resolve the created work item (the only one on this fresh stack) by its
    // workspace, for the card locator and the backend-side checks below.
    const api = new WorkflowApi(page.request, baseURL);
    await expect
      .poll(
        async () => (await api.listWorkflows()).some((w) => w.workspace_name === dind.workspaceName),
        { timeout: 30_000, message: 'created work item never appeared' },
      )
      .toBe(true);
    const found = (await api.listWorkflows()).find((w) => w.workspace_name === dind.workspaceName);
    if (!found) {
      throw new Error('work item not found after creation');
    }
    ticketKey = found.ticket_key;

    const card = new WorkItemCard(page, ticketKey);
    await card.waitFor();

    // Run the flow by clicking its button; wait for the success (completed)
    // badge — NOT the error badge.
    await card.runFlow(FLOW_NAME);
    await expect(card.completedFlowBadge(FLOW_NAME)).toBeVisible({ timeout: WORKFLOW_TIMEOUT_MS });
    await expect(card.erroredFlowBadge(FLOW_NAME)).toHaveCount(0);

    // The mock MUST have been hit — proves the real opencode binary ran and
    // talked to the OpenAI-compatible endpoint.
    const logs = await dind.exec(['logs', dind.mockName]);
    mockHit = /MOCK_HIT \d+ POST .*chat\/completions/.test(logs.stdout);
    expect(mockHit, 'mock LM Studio was hit by opencode').toBeTruthy();

    // F2 re-assessment: under real UI pacing, did the init `npm ci` product
    // survive into the bootstrapped worktree? (The prior API-first path may have
    // raced the worktree pre-create.) Check before any run-command re-bootstraps.
    const summary = await api.getWorkflow(ticketKey);
    worktreePath = summary.worktree_path ?? '';
    expect(worktreePath, 'worktree path resolved').toBeTruthy();
    const nm = await stackRef.exec(['test', '-d', `${worktreePath}/node_modules`]);
    nodeModulesExit = nm.exitCode;
    expect(
      nodeModulesExit,
      `node_modules should persist in the bootstrapped worktree (worktree=${worktreePath})`,
    ).toBe(0);
  });

  test('B3/B6: start the dev run-command and open its forwarded port from the card', async () => {
    test.setTimeout(SURFACE_TIMEOUT_MS + 120_000);
    expect(ticketKey, 'depends on the work item having been created').toBeTruthy();
    const card = new WorkItemCard(page, ticketKey);
    await card.waitFor();

    // First interaction on a finished item may rebuild the worktree/container,
    // which disables the Run button until ready — wait it out, then start.
    const runBtn = card.runCommandButton(DEV_COMMAND.name);
    await expect(runBtn).toBeEnabled({ timeout: SURFACE_TIMEOUT_MS });
    await card.runCommand(DEV_COMMAND.name);

    // Stop button appears once running.
    await expect(card.stopCommandButton(DEV_COMMAND.name)).toBeVisible({ timeout: 60_000 });

    // The "Open" link appears once the listening port is detected + forwarded.
    const openLink = card.runCommandOpenLink();
    await expect(openLink).toBeVisible({ timeout: SURFACE_TIMEOUT_MS });
    const href = await openLink.getAttribute('href');
    expect(href, 'forwarded port link is a /s/ proxy url').toMatch(/^\/s\//);

    // Open the forwarded port FROM the dashboard (new tab) and assert the Vite
    // app is served through the proxy.
    const popup = await openPopup(() => openLink.click());
    await expect
      .poll(async () => (await popup.content()).includes(APP_MARKER), {
        timeout: SURFACE_TIMEOUT_MS,
        message: 'proxied dev server never served the fixture app',
      })
      .toBe(true);
    await popup.close();

    // Clean stop.
    await card.stopCommand(DEV_COMMAND.name);
    await expect(card.runCommandButton(DEV_COMMAND.name)).toBeVisible({ timeout: 60_000 });
  });

  test('B4: open the IDE from the card', async () => {
    test.setTimeout(SURFACE_TIMEOUT_MS + 120_000);
    const card = new WorkItemCard(page, ticketKey);
    await card.waitFor();

    await card.clickEditor();
    await expect
      .poll(() => card.isEditorRunning(), {
        timeout: SURFACE_TIMEOUT_MS,
        message: 'editor never reported running',
      })
      .toBe(true);

    const href = await card.editorProxiedUrl();
    expect(href, 'editor opens through the /s/ proxy').toMatch(/^\/s\//);
    // Fetch the proxied editor doc through the same authenticated session.
    const api = new WorkflowApi(page.request, baseURL);
    await expect
      .poll(async () => (await api.fetchProxied(href)).status, {
        timeout: SURFACE_TIMEOUT_MS,
        message: 'openvscode never answered through the proxy',
      })
      .toBe(200);
  });

  test('B5: open the terminal from the card', async () => {
    test.setTimeout(SURFACE_TIMEOUT_MS + 120_000);
    const card = new WorkItemCard(page, ticketKey);
    await card.waitFor();

    await card.clickTerminal();
    await expect
      .poll(() => card.isTerminalRunning(), {
        timeout: SURFACE_TIMEOUT_MS,
        message: 'terminal never reported running',
      })
      .toBe(true);

    const href = await card.terminalProxiedUrl();
    expect(href, 'terminal opens through the /s/ proxy').toMatch(/^\/s\//);
    const api = new WorkflowApi(page.request, baseURL);
    await expect
      .poll(async () => (await api.fetchProxied(href)).status, {
        timeout: SURFACE_TIMEOUT_MS,
        message: 'ttyd never answered through the proxy',
      })
      .toBe(200);
  });

  test('B7: all agent CLIs are reachable from inside the workflow container during a run', async () => {
    test.setTimeout(SURFACE_TIMEOUT_MS + 120_000);
    const card = new WorkItemCard(page, ticketKey);
    await card.waitFor();

    // Bring up the per-item workspace container if it isn't already. This is the
    // container that hosts the worktree and the agent steps during a workflow
    // run; it mounts `/opt/takuto-tools` through DinD and prepends its `bin/` to
    // the login-shell PATH. Opening the IDE ensures `takuto-ws-<ticket>` is live.
    if (!(await card.isEditorRunning())) {
      await card.clickEditor();
      await expect
        .poll(() => card.isEditorRunning(), {
          timeout: SURFACE_TIMEOUT_MS,
          message: 'workspace container never came up',
        })
        .toBe(true);
    }

    // Locate the live workflow container inside the DinD daemon.
    const ps = await dind.exec(['ps', '--format', '{{.Names}}']);
    const workflowContainer = ps.stdout
      .split('\n')
      .map((s) => s.trim())
      .find((n) => n.startsWith('takuto-ws-'));
    expect(
      workflowContainer,
      `a workflow/workspace container should be running (ps: ${ps.stdout})`,
    ).toBeTruthy();

    // Every agent CLI must resolve and run FROM INSIDE that container — i.e. the
    // tools volume + PATH reach the workflow runtime, not just the takuto server.
    // `bash -lc` uses the login-shell PATH the worker entrypoint sets; `2>&1`
    // captures CLIs that print their version to stderr.
    for (const bin of ['claude', 'cursor-agent', 'codex', 'opencode']) {
      const r = await dind.exec(['exec', workflowContainer!, 'bash', '-lc', `${bin} --version 2>&1`]);
      expect(r.exitCode, `${bin} --version inside the workflow container (out: ${r.stdout})`).toBe(
        0,
      );
      expect(r.stdout, `${bin} --version produced no version token (out: ${r.stdout})`).toMatch(
        /\d+\.\d+/,
      );
    }
  });

  test('B8: egress allowlist is enforced in the workflow container, not the control plane', async () => {
    test.setTimeout(SURFACE_TIMEOUT_MS + 120_000);
    const card = new WorkItemCard(page, ticketKey);
    await card.waitFor();
    // Ensure the per-item workspace container (IDE/terminal/run-command) is up.
    if (!(await card.isEditorRunning())) {
      await card.clickEditor();
      await expect
        .poll(() => card.isEditorRunning(), {
          timeout: SURFACE_TIMEOUT_MS,
          message: 'workspace container never came up',
        })
        .toBe(true);
    }
    const ps = await dind.exec(['ps', '--format', '{{.Names}}']);
    const ws = ps.stdout
      .split('\n')
      .map((s) => s.trim())
      .find((n) => n.startsWith('takuto-ws-'));
    expect(ws, `workspace container should be running (ps: ${ps.stdout})`).toBeTruthy();

    // (1) Egress is applied inside the workspace container: default-DROP OUTPUT
    // policy + a jump to the TAKUTO_EGRESS chain.
    const rules = await dind.exec(['exec', '-u', '0', ws!, 'iptables', '-S', 'OUTPUT']);
    expect(rules.stdout, `OUTPUT should default-DROP (got: ${rules.stdout})`).toMatch(
      /-P OUTPUT DROP/,
    );
    expect(rules.stdout, `OUTPUT should jump to the TAKUTO_EGRESS chain`).toMatch(
      /-j TAKUTO_EGRESS_[AB]/,
    );

    // (2) A non-allowlisted host is BLOCKED (the firewall is live).
    const blocked = await dind.exec([
      'exec',
      ws!,
      'bash',
      '-lc',
      'curl -sS --max-time 6 -o /dev/null https://example.com 2>/dev/null; echo EXIT=$?',
    ]);
    expect(
      blocked.stdout,
      `example.com must be blocked inside the workflow container (got: ${blocked.stdout})`,
    ).not.toMatch(/EXIT=0/);

    // (3) An ALLOWED non-GitHub host is reachable — proves ALL hosts are
    // resolved into the allowlist, not just GitHub.
    const allowed = await dind.exec([
      'exec',
      ws!,
      'bash',
      '-lc',
      'curl -sS --max-time 15 -o /dev/null https://registry.npmjs.org 2>/dev/null; echo EXIT=$?',
    ]);
    expect(
      allowed.stdout,
      `registry.npmjs.org (allowlisted) must be reachable (got: ${allowed.stdout})`,
    ).toMatch(/EXIT=0/);

    // (4) The runtime refresh loop is running (pidfile + live process).
    const refresh = await dind.exec([
      'exec',
      '-u',
      '0',
      ws!,
      'sh',
      '-lc',
      'test -f /run/takuto-egress-refresh.pid && kill -0 "$(cat /run/takuto-egress-refresh.pid)" 2>/dev/null; echo EXIT=$?',
    ]);
    expect(refresh.stdout, `egress refresh loop should be running (got: ${refresh.stdout})`).toMatch(
      /EXIT=0/,
    );

    // (5) The control plane is NOT firewalled: the takuto server still has
    // unrestricted egress (the workflow container's DROP must not leak to the
    // shared/control-plane netns).
    const cp = await stackRef.exec([
      'bash',
      '-lc',
      'curl -sS --max-time 8 -o /dev/null https://example.com 2>/dev/null; echo EXIT=$?',
    ]);
    expect(cp.stdout, `control-plane egress must remain intact (got: ${cp.stdout})`).toMatch(
      /EXIT=0/,
    );
  });
});

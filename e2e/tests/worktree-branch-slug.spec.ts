import { test, expect } from '../src/fixtures/admin.fixture.js';
import { WorkflowApi } from '../src/api/workflows.js';

/**
 * #2 regression: a work item keyed with spaces — a locally-created ticket keyed
 * by its title, e.g. "very simple test" — must not produce a branch with
 * spaces. Before the fix the branch was `feat/very simple test`, an invalid git
 * ref whose spaces word-split the `git worktree add` shell command
 * (`usage: git worktree add …`), so worktree creation failed. Now the key is
 * slugified to a valid ref and `git worktree add` runs as argv.
 *
 * Uses the DinD stack because the worktree is created against the seeded
 * fixture repo, whose `origin` points at itself so the base-branch fetch
 * resolves for a purely local repo.
 */
test.use({ dind: true });

test.describe('worktree branch slugification (#2)', () => {
  test('a spaced ticket key yields a slugified branch and a created worktree', async ({
    page,
    admin,
    stack,
    backend,
  }) => {
    test.skip(backend !== 'sqlite', 'worktree/branch derivation is backend-independent');
    test.setTimeout(120_000);

    // The `admin` fixture bootstrapped + logged in the first admin (its session
    // cookie lands in `page.request`); assert it ran before using the API.
    expect(admin.creds.username, 'admin session must be bootstrapped').toBeTruthy();

    const dind = stack.dind;
    if (!dind) throw new Error('this spec requires a DinD stack (test.use({ dind: true }))');

    // `admin` bootstrapped + logged in the first admin. Associate the reconciled
    // fixture repo so the manual workflow has a repo to branch from (mirrors the
    // one unavoidable non-UI step in workflow-implement.spec).
    const api = new WorkflowApi(page.request, stack.baseURL);
    const mine = await api.listRepositories();
    if (!mine.some((r) => r.name === dind.workspaceName)) {
      const available = await api.listAvailableRepositories();
      const found = available.find((r) => r.name === dind.workspaceName);
      if (!found) throw new Error(`fixture repo "${dind.workspaceName}" not registered`);
      await api.addExistingRepository(found.id);
    }

    const spacedKey = 'very simple test';
    const started = await api.startManualWorkflow({
      ticket_key: spacedKey,
      ticket_summary: spacedKey,
      ticket_description: 'Verify a spaced key slugifies into a valid branch.',
    });
    expect(started.ticket_key).toBe(spacedKey);

    // The worktree is pre-created during bootstrap, before any agent step. Poll
    // until the branch lands: the fix makes it a valid, space-free ref; the old
    // bug left it unset (worktree add failed with a git usage error).
    await expect
      .poll(
        async () =>
          (await api.listWorkflows()).find((w) => w.ticket_key === spacedKey)?.branch_name ?? '',
        { timeout: 60_000, message: 'branch_name never populated — worktree add likely failed' },
      )
      .toBe('feat/very-simple-test');

    const w = (await api.listWorkflows()).find((x) => x.ticket_key === spacedKey);
    expect(w, 'the work item must be present').toBeTruthy();
    expect(w!.branch_name, 'branch must contain no whitespace').not.toMatch(/\s/);
    expect(w!.worktree_path, 'worktree_path must be set (worktree was created)').toBeTruthy();
  });
});

import { test, expect } from '../src/fixtures/admin.fixture.js';
import { ApiError } from '../src/api/client.js';

/**
 * #5(b) regression: a pasted AI-provider API key is stripped of all whitespace
 * before it is sealed. A trailing newline, or an OAuth token that wrapped
 * across two lines on screen and was pasted as two lines, otherwise sealed a
 * malformed bearer that surfaced later as `401 Invalid bearer token` from the
 * agent CLI. Control-plane only, so it runs once on sqlite.
 */
test.describe('credential paste — whitespace is stripped (#5b)', () => {
  test('accepts a whitespace-mangled token and stores it connected', async ({ admin, backend }) => {
    test.skip(backend !== 'sqlite', 'credential handling is backend-independent');

    // Trailing newline + a mid-token line wrap (the two-line paste failure).
    await admin.api.setProviderCredential('claude', {
      api_key: '  sk-ant-oat01-AAAABBBBCCCC\nDDDDEEEEFFFF\n',
    });

    const creds = await admin.api.getUserCredentials('claude');
    expect(
      creds.provider?.api_key,
      'api_key slot must be set after a whitespace-mangled paste',
    ).toBeTruthy();
  });

  test('rejects an all-whitespace token as empty', async ({ admin, backend }) => {
    test.skip(backend !== 'sqlite', 'credential handling is backend-independent');

    // After stripping, the key is empty → the shape validator rejects it 400
    // instead of sealing a blank credential.
    const err = await admin.api
      .setProviderCredential('claude', { api_key: '   \n\t ' })
      .then(() => null)
      .catch((e: unknown) => e);
    expect(err, 'all-whitespace key must be rejected').toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(400);
  });
});

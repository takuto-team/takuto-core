import { randomBytes } from 'node:crypto';
import type { AdminCredentials } from './types.js';

/**
 * Mint a fresh first-admin credential set. The password is comfortably over the
 * server's 12-char minimum (`register.rs:72`). Randomized so a stack reused by
 * a worker never collides with a real value, and so two workers never share one.
 */
export function newAdminCredentials(): AdminCredentials {
  const token = randomBytes(6).toString('hex');
  return {
    username: `e2e-admin-${token}`,
    password: `e2e-pw-${token}-${token}`,
  };
}

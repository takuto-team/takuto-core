import { removeByLabel } from './src/docker/cli.js';
import { LABEL_RUN, RUN_ID } from './src/docker/naming.js';

/**
 * Final sweep: remove every container, network, and volume labelled with this
 * run's id, so a clean (or failed) run leaves zero residual Docker resources.
 */
export default async function globalTeardown(): Promise<void> {
  console.log(`[e2e:teardown] sweeping resources for run ${RUN_ID}`);
  await removeByLabel(`${LABEL_RUN}=${RUN_ID}`);
}

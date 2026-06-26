import { pull } from './src/docker/cli.js';
import { ensureImage } from './src/docker/image.js';
import { RUN_ID } from './src/docker/naming.js';

const DB_IMAGES = ['postgres:16', 'mariadb:11'];

/**
 * Build the Takuto image from the working tree once per run (cached by source
 * hash) and pre-pull the database images so per-worker stack bring-up is fast
 * and not racing on a shared `docker pull`.
 */
export default async function globalSetup(): Promise<void> {
  const log = (msg: string): void => {
    console.log(`[e2e:setup] ${msg}`);
  };
  log(`run id ${RUN_ID}`);

  const tag = await ensureImage(log);
  log(`takuto image: ${tag}`);

  await Promise.all(
    DB_IMAGES.map(async (image) => {
      log(`pulling ${image}…`);
      await pull(image);
    }),
  );
  log('database images ready');
}

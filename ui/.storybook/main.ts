import type { StorybookConfig } from '@storybook/react-vite';
import type { InlineConfig } from 'vite';

const config: StorybookConfig = {
  stories: [
    "../src/**/*.mdx",
    "../src/**/*.stories.@(js|jsx|mjs|ts|tsx)",
  ],
  addons: [
    "@chromatic-com/storybook",
    "@storybook/addon-vitest",
    "@storybook/addon-a11y",
    "@storybook/addon-docs",
    "@storybook/addon-onboarding",
  ],
  framework: "@storybook/react-vite",
  async viteFinal(config: InlineConfig) {
    // Remove the PWA plugin — it breaks the Storybook build (large manager chunks exceed
    // the workbox precache size limit). VitePWA returns an array of plugins, so we need
    // to flatten and filter at all nesting levels.
    if (config.plugins) {
      const isPwa = (p: unknown): boolean => {
        if (!p || typeof p !== "object") return false;
        if (Array.isArray(p)) return false;
        const plugin = p as { name?: string };
        return !!plugin.name?.startsWith("vite-plugin-pwa");
      };
      config.plugins = (config.plugins as Array<unknown>).flatMap((p) =>
        Array.isArray(p) ? p : [p]
      ).filter((p) => !isPwa(p));
    }
    return config;
  },
};
export default config;
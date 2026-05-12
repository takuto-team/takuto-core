/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { VitePWA } from "vite-plugin-pwa";
import { readFileSync } from "fs";
import { resolve } from "path";
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { storybookTest } from '@storybook/addon-vitest/vitest-plugin';
import { playwright } from '@vitest/browser-playwright';
const dirname = typeof __dirname !== 'undefined' ? __dirname : path.dirname(fileURLToPath(import.meta.url));

// More info at: https://storybook.js.org/docs/next/writing-tests/integrations/vitest-addon
const version = readFileSync(resolve(__dirname, "../VERSION"), "utf-8").trim();

// When running inside a Maestro container (run command), MAESTRO_PROXY_BASE is
// set to the proxy path (e.g. /s/{token}/).  Use it as Vite's `base` so all
// generated asset URLs go through the reverse proxy, and disable the dev proxy
// rules (they target the host's dashboard, not the container's).
const proxyBase = process.env.MAESTRO_PROXY_BASE;

export default defineConfig({
  base: proxyBase || "/",
  define: {
    __APP_VERSION__: JSON.stringify(version)
  },
  plugins: [react(), tailwindcss(), VitePWA({
    registerType: "autoUpdate",
    workbox: {
      // Don't let the service worker intercept navigations to
      // /s/<path-token>/...  — those must reach the Axum reverse-proxy
      // handler, not be served as cached SPA shell (GH-45).
      navigateFallbackDenylist: [/^\/s\//],
    },
    manifest: {
      name: "Maestro Dashboard",
      short_name: "Maestro",
      description: "Automated workflow orchestration dashboard",
      theme_color: "#030712",
      background_color: "#030712",
      display: "standalone",
      icons: []
    }
  })],
  server: {
    // Dev proxy rules forward API/WS/session requests to the Maestro backend
    // when running locally.  Inside a container (MAESTRO_PROXY_BASE is set),
    // these are not needed — the reverse proxy handles routing.
    proxy: proxyBase ? undefined : {
      "/api": "http://localhost:8080",
      "/ws": {
        target: "ws://localhost:8080",
        ws: true
      },
      "/s/": "http://localhost:8080"
    }
  },
  test: {
    projects: [
      {
        extends: true,
        plugins: [
        // The plugin will run tests for the stories defined in your Storybook config
        // See options at: https://storybook.js.org/docs/next/writing-tests/integrations/vitest-addon#storybooktest
        storybookTest({
          configDir: path.join(dirname, '.storybook')
        })],
        test: {
          name: 'storybook',
          browser: {
            enabled: true,
            headless: true,
            provider: playwright({}),
            instances: [{
              browser: 'chromium'
            }]
          }
        }
      },
      {
        test: {
          name: 'unit',
          environment: 'jsdom',
          include: ['src/**/*.test.{ts,tsx}'],
        }
      }
    ]
  }
});
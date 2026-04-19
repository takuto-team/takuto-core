import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { VitePWA } from "vite-plugin-pwa";

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    VitePWA({
      registerType: "autoUpdate",
      manifest: {
        name: "Maestro Dashboard",
        short_name: "Maestro",
        description: "Automated workflow orchestration dashboard",
        theme_color: "#030712",
        background_color: "#030712",
        display: "standalone",
        icons: [],
      },
    }),
  ],
  server: {
    proxy: {
      "/api": "http://localhost:8080",
      "/ws": {
        target: "ws://localhost:8080",
        ws: true,
      },
    },
  },
});

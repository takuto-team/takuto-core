import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// host 0.0.0.0 + a pinned, strict port so the dev server binds beyond loopback
// and is reachable through the container port-forward / `/s/` proxy.
export default defineConfig({
  plugins: [react()],
  server: {
    host: "0.0.0.0",
    port: 5173,
    strictPort: true,
  },
});

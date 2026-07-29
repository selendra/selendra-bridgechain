import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// One env var read at config time (dev-server proxy target). Declared locally to
// avoid pulling in @types/node just for this.
declare const process: { env: Record<string, string | undefined> };

// Backend (graphql-api) origin the dev server proxies to. Defaults to the
// standard :8088; scripts/run.sh sets VITE_PROXY_TARGET so a custom GQL_PORT
// still works same-origin (no CORS).
const proxyTarget = process.env.VITE_PROXY_TARGET ?? "http://127.0.0.1:8088";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    host: true,
    // Forward GraphQL/health calls to the backend so the app talks to it
    // same-origin in dev. Backend is launched by scripts/run.sh.
    proxy: {
      "/graphql": proxyTarget,
      "/health": proxyTarget,
    },
  },
});

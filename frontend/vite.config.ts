import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    host: true,
    // Forward GraphQL/health calls to the backend (graphql-api) so the app can
    // talk to it same-origin in dev. Backend is launched by scripts/run-dev.sh.
    proxy: {
      "/graphql": "http://127.0.0.1:8088",
      "/health": "http://127.0.0.1:8088",
    },
  },
});

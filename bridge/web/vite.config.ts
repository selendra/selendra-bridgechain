import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// During `vite dev` the browser talks to the dashboard's own origin and we proxy
// `/graphql` and `/health` to the running graphql-api (default 127.0.0.1:8088),
// sidestepping CORS. Override the target with GRAPHQL_API_URL when the API lives
// elsewhere. In a production build, set VITE_GRAPHQL_URL so the app calls the API
// directly (the proxy only exists in dev).
const API = process.env.GRAPHQL_API_URL ?? "http://127.0.0.1:8088";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      "/graphql": { target: API, changeOrigin: true },
      "/health": { target: API, changeOrigin: true },
    },
  },
});

import { defineConfig, devices } from "@playwright/test";

/**
 * Two projects, because the suite tests two different things:
 *
 *  - **unit** runs in Node and imports the pure modules directly (`data/format`,
 *    `wallet/eth` encoders). These are the functions that turn user input into
 *    calldata — worth testing at the byte level, where a wrong offset is visible.
 *  - **e2e** drives the real app in Chromium against a mocked backend and a
 *    mocked EIP-1193 wallet, so every flow (bridge, swap, explorer, finalize
 *    recovery, network guards) runs end to end without a chain.
 *
 * No live services: the backend is intercepted with `page.route` and the wallet
 * is injected with `addInitScript`. That keeps the suite deterministic and
 * runnable in CI.
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 2 : undefined,
  reporter: process.env.CI ? [["github"], ["list"]] : [["list"]],
  use: {
    baseURL: "http://127.0.0.1:5174",
    trace: "on-first-retry",
  },
  projects: [
    {
      name: "unit",
      testMatch: /unit\/.*\.spec\.ts/,
    },
    {
      name: "e2e",
      testMatch: /app\/.*\.spec\.ts/,
      use: { ...devices["Desktop Chrome"] },
    },
    {
      // Drives the REAL stack on real testnets — no mocks. Skips itself when
      // the backend isn't reachable, so it is safe to leave in the default run.
      // Not part of `test:unit`/`test:e2e`; run with `npm run test:live`.
      //
      // SERIAL on purpose. Every worker shares one hosted-RPC budget, and a
      // parallel run makes the suite rate-limit itself — producing failures that
      // look like product bugs but are pure self-contention.
      name: "live",
      testMatch: /live\/.*\.spec\.ts/,
      fullyParallel: false,
      workers: 1,
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  // The dev server is enough: these tests exercise app behaviour, and the
  // production build is separately gated by `tsc -b && vite build` in CI.
  webServer: {
    // Bare `vite`: Playwright puts `node_modules/.bin` on PATH, so this resolves
    // under both bun and npm installs without hardcoding either runner.
    command: "vite --port 5174 --strictPort",
    url: "http://127.0.0.1:5174",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});

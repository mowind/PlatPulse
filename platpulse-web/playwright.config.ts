import { availableParallelism } from 'node:os'

import { defineConfig } from '@playwright/test'

const ci = process.env.CI !== undefined && process.env.CI !== ''
// Attach mode: E2E_PORT names a Server the whole run shares (one started by
// hand with e2e/start-server.sh, or by the webServer below), as the evidence
// recipes under docs/visual-migration/ do. It runs in one worker.
const attachedPort = process.env.E2E_PORT === '' ? undefined : process.env.E2E_PORT

/**
 * Fixed viewport matrix required by the project (see AGENTS.md): the Home
 * and Admin shells must work on phone, tablet, and desktop from Phase 0
 * onward. The suite runs against a real dev-mode platpulse-server serving
 * the production WebUI build: every worker boots its own seeded Server
 * (e2e/fixtures.ts, e2e/worker-server.ts) on its own port and data
 * directory, and every test's baseURL points at it (issue #289).
 */
export default defineConfig({
  testDir: './e2e',
  // Builds the WebUI bundle and the Server binary once for every worker.
  globalSetup: './e2e/global-setup.ts',
  // Each worker runs a Server, its fixture refreshers and a browser. A GitHub
  // runner has 4 vCPUs, so CI runs two; locally up to four, never more than
  // half the cores. Override with --workers.
  workers: attachedPort !== undefined ? 1 : ci ? 2 : Math.max(1, Math.min(4, Math.floor(availableParallelism() / 2))),
  fullyParallel: true,
  reporter: 'list',
  // One retry on CI only, and only for evidence: a test that passes on its
  // retry is flaky, and failOnFlakyTests fails the run anyway, so the retry
  // never turns a failure green. It records a second trace for the issue that
  // every flaky test gets (timeout budget, race, or shared data; issue #283).
  // The retry runs in a new worker, so against a freshly seeded Server.
  // Local runs keep zero retries.
  retries: ci ? 1 : 0,
  failOnFlakyTests: ci,
  projects: [
    {
      name: 'phone-360-touch',
      use: { viewport: { width: 360, height: 800 }, hasTouch: true },
    },
    {
      name: 'phone-390-touch',
      use: { viewport: { width: 390, height: 844 }, hasTouch: true },
    },
    {
      name: 'tablet-768-touch',
      use: { viewport: { width: 768, height: 1024 }, hasTouch: true },
    },
    {
      name: 'desktop-1280',
      use: { viewport: { width: 1280, height: 800 } },
    },
    {
      // Required by the Emerald visual migration acceptance matrix. Adding a
      // project changes the CI matrix, so AGENTS.md lists it too.
      name: 'desktop-1440',
      use: { viewport: { width: 1440, height: 900 } },
    },
  ],
  webServer:
    attachedPort === undefined
      ? undefined
      : {
          // Start the attached Server unless one already listens on E2E_PORT.
          command: 'bash e2e/start-server.sh',
          url: `http://127.0.0.1:${attachedPort}`,
          reuseExistingServer: !ci,
          timeout: 300_000,
        },
})

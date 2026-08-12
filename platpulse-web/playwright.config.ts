import { defineConfig } from '@playwright/test'

/**
 * Fixed viewport matrix required by the project (see AGENTS.md): the Home
 * and Admin shells must work on phone, tablet, and desktop from Phase 0
 * onward. The suite runs against a real dev-mode platpulse-server serving
 * the production WebUI build (e2e/start-server.sh).
 */
export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  reporter: 'list',
  use: {
    baseURL: 'http://127.0.0.1:4173',
  },
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
  ],
  webServer: {
    // Serve the production build through the real platpulse-server (init +
    // owner create + serve in dev mode) so the auth flow is verified
    // against the same artifact production hosts.
    command: 'bash e2e/start-server.sh',
    url: 'http://127.0.0.1:4173',
    reuseExistingServer: !process.env.CI,
    timeout: 300_000,
  },
})

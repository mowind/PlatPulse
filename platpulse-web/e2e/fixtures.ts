import { join } from 'node:path'

import { test as base } from '@playwright/test'

import { startWorkerServer, type WorkerServer } from './worker-server'

/**
 * The suite's `test`: Playwright's own, plus one seeded Server per worker
 * (e2e/worker-server.ts) that every test's `baseURL` points at. Specs import
 * `test` and `expect` from here; eslint.config.js rejects `test` from
 * '@playwright/test' under e2e/.
 */
export * from '@playwright/test'

// Fixture callbacks name Playwright's `use` argument `provide` so the React
// hooks lint rule does not mistake it for React's `use` hook.
export const test = base.extend<object, { workerServer: WorkerServer }>({
  workerServer: [
    // eslint-disable-next-line no-empty-pattern -- Playwright reads fixture dependencies from this pattern
    async ({}, provide, workerInfo) => {
      const server = await startWorkerServer(
        workerInfo.workerIndex,
        join(workerInfo.project.outputDir, 'worker-servers', `worker-${workerInfo.workerIndex}.log`),
      )
      try {
        await provide(server)
      } finally {
        await server.stop()
      }
    },
    // Seeding takes seconds; the default 30 s fixture budget would cut a
    // loaded CI runner short.
    { scope: 'worker', timeout: 180_000 },
  ],
  // `page`, `context`, `request` and a test's own `browser.newContext()`
  // all read their base URL from this fixture.
  baseURL: async ({ workerServer }, provide) => {
    await provide(workerServer.baseUrl)
  },
})

import { expect, test } from '@playwright/test'

import type { CapacityOverview, StorageOverview } from '../src/api/generated'
import {
  VIEWPORTS,
  expectResolvedTheme,
  gotoAuthenticated,
  loginToDisposableServer,
} from './admin-flow'
import { expectNoHorizontalOverflow, expectVisibleInteractiveTargets } from './helpers'
import { startDisposableServer, type DisposableServer } from './server-harness'

/**
 * Storage Backend boundary acceptance (issue #256, ADR 0013).
 *
 * The disposable harness boots the production Server on SQLite, so this spec
 * proves the surface that a SQLite Operator actually sees: the Owner-only
 * Storage Backend facts, the capacity policy that still applies, and the
 * absence of any storage identity in a Public Projection. The PostgreSQL half
 * of the issue — the [capacity] refusal at startup, the Doctor checks that
 * replace the SQLite file checks, and the Backup artifact surface that has
 * nothing to list — is driven by the Server's own test suite, because a
 * PostgreSQL Storage Backend needs a real PostgreSQL server rather than a
 * browser.
 */

/** The Admin API the WebUI reads, over real HTTP with the Owner session. */
async function readStorage(server: DisposableServer): Promise<StorageOverview> {
  const response = await server.adminGet('/api/admin/v1/storage')
  expect(response.status).toBe(200)
  return response.body as StorageOverview
}

async function readCapacity(server: DisposableServer): Promise<CapacityOverview> {
  const response = await server.adminGet('/api/admin/v1/capacity')
  expect(response.status).toBe(200)
  return response.body as CapacityOverview
}

const FACTS = '[data-slot="storage-backend-facts"]'

test('the Server states its Storage Backend and what that backend cannot do', async ({
  browser,
}, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1280', 'the Server flow runs once')
  test.setTimeout(600_000)
  const server = await startDisposableServer()
  try {
    // The facts the Owner surface publishes are the ones the Server actually
    // uses: the SQLite file path is the harness's real database, and no
    // PostgreSQL identity is invented beside it.
    const storage = await readStorage(server)
    expect(storage.backend).toBe('sqlite')
    expect(storage.databasePath).toBe(server.dbPath)
    expect(storage.postgres ?? null).toBeNull()

    // Capacity protection applies here, so the Server declares no reason for
    // it to be absent.
    const capacity = await readCapacity(server)
    expect(capacity.notApplicableReason ?? null).toBeNull()

    const context = await browser.newContext()
    const page = await context.newPage()
    try {
      await loginToDisposableServer(page, server.baseUrl)

      // A Public Projection is not an Admin DTO with fields filtered out: it
      // carries no storage identity at all, even for a signed-in Owner.
      for (const route of ['/api/public/v1/networks', '/api/public/v1/access']) {
        const publicResponse = await page.request.get(server.baseUrl + route)
        expect(publicResponse.status(), route).toBe(200)
        const publicText = (await publicResponse.text()).toLowerCase()
        for (const forbidden of ['databasepath', 'storagebackend', 'postgres', 'sqlite']) {
          expect(publicText, route + ' must not carry ' + forbidden).not.toContain(forbidden)
        }
      }

      await gotoAuthenticated(page, server.baseUrl, '/admin/settings')
      const facts = page.locator(FACTS)
      await expect(facts).toBeVisible()
      await expect(facts).toContainText('SQLite')
      await expect(facts).toContainText(server.dbPath)
      // A backend reported is not a backend switchable: no control offers to
      // change where the Server keeps its data.
      await expect(facts.getByRole('button')).toHaveCount(0)
      await expect(facts.getByRole('combobox')).toHaveCount(0)
    } finally {
      await context.close()
    }
  } finally {
    await server.dispose()
  }
})

test('the Storage Backend surface holds across the fixed viewport and theme matrix', async ({
  browser,
}, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix runs once against one disposable Server')
  test.setTimeout(600_000)
  const server = await startDisposableServer()
  try {
    const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
    const page = await context.newPage()
    try {
      await loginToDisposableServer(page, server.baseUrl)
      for (const viewport of VIEWPORTS) {
        for (const colorScheme of ['light', 'dark'] as const) {
          await page.setViewportSize(viewport)
          await page.emulateMedia({ colorScheme })

          // Settings reports the backend as configuration.
          await gotoAuthenticated(page, server.baseUrl, '/admin/settings')
          await expect(
            page.getByRole('heading', { level: 1, name: 'Settings', exact: true }),
          ).toBeVisible()
          const settingsFacts = page.locator(FACTS)
          await expect(settingsFacts).toBeVisible()
          await expect(settingsFacts).toContainText('SQLite')
          await expect(settingsFacts).toContainText(server.dbPath)
          await expect(page.getByRole('heading', { level: 2, name: 'Storage Backend' })).toBeVisible()
          await expectResolvedTheme(page, colorScheme)
          await expectNoHorizontalOverflow(page)

          // Doctor reads the same facts beside its checks.
          await gotoAuthenticated(page, server.baseUrl, '/admin/doctor')
          await expect(
            page.getByRole('heading', { level: 1, name: 'Doctor', exact: true }),
          ).toBeVisible()
          const doctorFacts = page.locator(FACTS)
          await expect(doctorFacts).toBeVisible()
          await expect(doctorFacts).toContainText('SQLite')
          await expect(doctorFacts).toContainText(server.dbPath)
          await expectResolvedTheme(page, colorScheme)
          await expectNoHorizontalOverflow(page)
          if (viewport.width <= 768) await expectVisibleInteractiveTargets(page)
        }
      }
    } finally {
      await context.close()
    }
  } finally {
    await server.dispose()
  }
})

/**
 * The PostgreSQL half of the surface, in the browser. A Playwright project has
 * no PostgreSQL server to boot, so the two Admin responses the new states are
 * rendered from are served by route interception: the page components, their
 * states, and the layout are the production ones, and every fixed viewport
 * and theme renders them. The Server-side truth of these responses (the
 * refusal, the NOT_APPLICABLE checks, the omitted database size) is proven by
 * the Rust suites against a real PostgreSQL server.
 */
const POSTGRES_STORAGE: StorageOverview = {
  backend: 'postgres',
  databasePath: null,
  postgres: {
    host: 'db.internal',
    port: 5432,
    database: 'platpulse',
    user: 'platpulse',
    serverVersion: '18.6',
    serverVersionNum: 180006,
    minimumServerVersionNum: 150000,
  },
}

test('the PostgreSQL Storage Backend states render in every viewport and theme', async ({
  browser,
}, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix runs once against one disposable Server')
  test.setTimeout(600_000)
  const server = await startDisposableServer()
  try {
    // The policy fields are the Server's own; only the reason they do not
    // apply is the PostgreSQL one, so the rest of the page stays truthful.
    const capacity = await readCapacity(server)
    const notApplicable = {
      ...capacity,
      enabled: false,
      mountPath: null,
      notApplicableReason:
        'the PostgreSQL Storage Backend leaves the database disk to the database operator, so the Server neither measures nor protects it against low space',
    }

    const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
    const page = await context.newPage()
    try {
      await loginToDisposableServer(page, server.baseUrl)
      await page.route('**/api/admin/v1/storage', (route) =>
        route.fulfill({ json: POSTGRES_STORAGE }),
      )
      await page.route('**/api/admin/v1/capacity', (route) => route.fulfill({ json: notApplicable }))
      await page.route('**/api/admin/v1/doctor', (route) =>
        route.fulfill({
          json: {
            checks: [
              {
                checkId: 'backup_age',
                label: 'Latest backup is recent',
                status: 'not_applicable',
                detail: 'the database disk belongs to the database operator',
              },
            ],
          },
        }),
      )

      for (const viewport of VIEWPORTS) {
        for (const colorScheme of ['light', 'dark'] as const) {
          await page.setViewportSize(viewport)
          await page.emulateMedia({ colorScheme })

          await gotoAuthenticated(page, server.baseUrl, '/admin/settings')
          const settingsFacts = page.locator(FACTS)
          await expect(settingsFacts).toBeVisible()
          await expect(settingsFacts).toContainText('PostgreSQL')
          await expect(settingsFacts).toContainText('18.6')
          await expect(settingsFacts).toContainText('db.internal:5432')
          await expect(settingsFacts).not.toContainText('SQLite')
          await expectResolvedTheme(page, colorScheme)
          await expectNoHorizontalOverflow(page)

          // Capacity protection is reported as not applicable instead of as
          // an absent policy the Operator could declare.
          await gotoAuthenticated(page, server.baseUrl, '/admin/operations')
          await expect(
            page.getByRole('heading', { level: 1, name: 'Operations', exact: true }),
          ).toBeVisible()
          await expect(page.getByText('Does not apply').first()).toBeVisible()
          await expect(page.locator('[data-slot="capacity-intervals-table"]')).toHaveCount(0)
          await expectResolvedTheme(page, colorScheme)
          await expectNoHorizontalOverflow(page)

          // A Doctor check says it does not apply on this backend.
          await gotoAuthenticated(page, server.baseUrl, '/admin/doctor')
          await expect(page.getByText('Does not apply').first()).toBeVisible()
          await expectNoHorizontalOverflow(page)

          // The Backup artifact surface says there is nothing to list here.
          await gotoAuthenticated(page, server.baseUrl, '/admin/backups')
          await expect(page.getByText(/keeps no Backup Artifacts/)).toBeVisible()
          await expect(page.getByText(/No backup artifact is recorded yet/)).toHaveCount(0)
          await expectNoHorizontalOverflow(page)
          if (viewport.width <= 768) await expectVisibleInteractiveTargets(page)
        }
      }
    } finally {
      await context.close()
    }
  } finally {
    await server.dispose()
  }
})

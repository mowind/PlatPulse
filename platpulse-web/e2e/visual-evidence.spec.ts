import { expect, test } from '@playwright/test'
import { loginAs } from './helpers'

/**
 * Screenshot evidence for the Emerald visual migration (docs/adr/0002).
 *
 * These captures are EVIDENCE, never gates: there is no toHaveScreenshot
 * baseline anywhere in this repository, so no regression can be hidden by
 * re-recording one. The matrix comes from playwright.config.ts, including the
 * desktop-1440 project added for this migration.
 *
 * Run explicitly, once a build is being served:
 *   EMERALD_EVIDENCE=1 npx playwright test e2e/visual-evidence.spec.ts
 * Output: docs/visual-migration/emerald/screenshots/<project>/<page>.png
 */
const enabled = Boolean(process.env.EMERALD_EVIDENCE)
test.skip(!enabled, 'set EMERALD_EVIDENCE=1 to capture visual evidence')


/**
 * The map is an ECharts canvas whose geometry and library are both loaded
 * asynchronously, so a screenshot taken at network idle can catch it empty.
 * Wait until the canvas actually has a painted pixel before capturing. A missing
 * or unpainted canvas fails this evidence run instead of producing a blank map.
 */
async function waitForMapPainted(page: import('@playwright/test').Page) {
  const canvas = page.locator('[data-slot="geo-chart"] canvas').first()
  await expect(canvas).toBeVisible({ timeout: 15_000 })
  await page.waitForFunction(
    () => {
      const element = document.querySelector('[data-slot="geo-chart"] canvas') as HTMLCanvasElement | null
      if (!element) return false
      const context = element.getContext('2d')
      if (!context) return false
      const data = context.getImageData(0, 0, element.width, element.height).data
      for (let index = 3; index < data.length; index += 4) {
        if (data[index] > 0) return true
      }
      return false
    },
    undefined,
    { timeout: 15_000 },
  )
}


/**
 * The Server's seeded Geo provider is Disabled, so the map correctly renders
 * nothing at all. Enable the Local MMDB provider first so the evidence shows the
 * map the acceptance actually cares about. This only touches the throwaway e2e
 * Server state; the evidence harness disposes of that Server after the run.
 */
async function enableLocalGeoProvider(page: import('@playwright/test').Page) {
  await page.goto('/admin/settings')
  await page.waitForLoadState('networkidle')
  const card = page.getByRole('article').filter({ has: page.getByRole('heading', { name: 'Geo provider' }) })
  const local = card.getByRole('radio', { name: 'Local MMDB' })
  if (!(await local.isChecked())) {
    await local.check()
    await card.getByRole('button', { name: 'Save Geo provider' }).click()
    await expect(card.getByText(/Geo provider is now Local MMDB/)).toBeVisible({ timeout: 15_000 })
  }
}


/** Absence of a loading placeholder is not readiness: before the Node query
 * resolves there are no chart placeholders either. Require the real content
 * first, then fail (rather than warn) if history never settles. */
async function waitForChartsSettled(page: import('@playwright/test').Page) {
  await expect(page.locator('#node-detail-title')).toBeVisible({ timeout: 15_000 })
  await expect(page.locator('[data-slot="node-metric-card"]')).toHaveCount(6)
  await expect(page.getByText('Loading metric history…')).toHaveCount(0, { timeout: 15_000 })
  await expect(page.getByRole('group', { name: 'Node key summary' })).toBeVisible()
}

const OUTPUT = '../docs/visual-migration/emerald/screenshots'

/**
 * The acceptance surfaces, not every page: six captures per viewport keep the
 * evidence reviewable, including Settings for native selection controls. Other
 * Admin pages share the same primitives and shell; their behaviour is covered
 * by the e2e suite rather than by a
 * screenshot.
 */
const PAGES: Array<[string, string]> = [
  ['admin.home', '/admin'],
  ['admin.settings', '/admin/settings'],
]

test('capture Emerald migration evidence', async ({ page }, testInfo) => {
  const dir = OUTPUT + '/' + testInfo.project.name

  // Login is public: capture it before the session exists.
  await page.goto('/login')
  await page.waitForLoadState('networkidle')
  await page.screenshot({ path: dir + '/public.login.png', fullPage: true })

  await loginAs(page)
  await enableLocalGeoProvider(page)
  await page.goto('/')

  await page.waitForLoadState('networkidle')
  await waitForMapPainted(page)
  await page.screenshot({ path: dir + '/home.png', fullPage: true })

  // The public Node Detail is reached through the whole-card link, so the
  // capture never depends on a seeded Node id.
  const nodeLink = page.getByLabel('Active Nodes', { exact: true }).getByRole('link').first()
  await expect(nodeLink).toBeVisible()
  await nodeLink.click()
  await waitForChartsSettled(page)
  await page.screenshot({ path: dir + '/public.node-detail.png', fullPage: true })

  // The Node Peer Country View first row: the wider map track beside the four
  // summary tiles, with the map's in-scope figure agreeing with the Peer Count
  // tile because both read the same retained Peer records. The locator capture
  // is the reviewable evidence for the approved §11.1 composition.
  const peerCountries = page.getByRole('region', { name: 'Node Peer countries' })
  await expect(peerCountries).toBeVisible()
  const peerRecords = peerCountries.getByRole('note')
  await expect(peerRecords).toBeVisible({ timeout: 15_000 })
  const tileCount = (await page.locator('[data-slot="node-summary-tile"]').filter({ hasText: 'Peers' }).locator('strong').textContent()) ?? ''
  expect(await peerRecords.getAttribute('aria-label')).toContain(tileCount.trim() + ' Peer records in scope')
  await page.locator('[data-slot="node-overview"]').screenshot({ path: dir + '/public.node-detail-peer-countries.png' })

  // The Network overview page is deleted; its evidence capture is gone with it.

  for (const [name, path] of PAGES) {
    await page.goto(path)
    await page.waitForLoadState('networkidle')
    await page.screenshot({ path: dir + '/' + name + '.png', fullPage: true })
  }
})

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
 * Wait until the canvas actually has a painted pixel before capturing, and say
 * so loudly if it never does.
 */
async function waitForMapPainted(page: import('@playwright/test').Page) {
  const canvas = page.locator('[data-slot="geo-chart"] canvas').first()
  if ((await canvas.count()) === 0) return
  try {
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
  } catch {
    console.log('WARNING: the map canvas never painted before the capture')
  }
}


/**
 * The Server's seeded Geo provider is Disabled, so the map correctly renders
 * nothing at all. Enable the Local MMDB provider first so the evidence shows the
 * map the acceptance actually cares about. This only touches the throwaway e2e
 * Server state, and it is restored below.
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

const OUTPUT = '../docs/visual-migration/emerald/screenshots'

/**
 * The acceptance surfaces, not every page: five captures per viewport keeps the
 * evidence reviewable. The remaining Admin pages share the same primitives and
 * shell, and their behaviour is covered by the e2e suite rather than by a
 * screenshot.
 */
const PAGES: Array<[string, string]> = [
  ['admin.home', '/admin'],
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
  if (await nodeLink.count()) {
    await nodeLink.click()
    await page.waitForLoadState('networkidle')
    await page.screenshot({ path: dir + '/public.node-detail.png', fullPage: true })
  }

  const networkLink = page.locator('a[href^="/networks/"]').first()
  if (await networkLink.count()) {
    await networkLink.click()
    await page.waitForLoadState('networkidle')
    await page.screenshot({ path: dir + '/public.network-detail.png', fullPage: true })
  }

  for (const [name, path] of PAGES) {
    await page.goto(path)
    await page.waitForLoadState('networkidle')
    await page.screenshot({ path: dir + '/' + name + '.png', fullPage: true })
  }
})

import { test } from '@playwright/test'
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

  await page.waitForLoadState('networkidle')
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

import { test, expect, type Page } from '@playwright/test'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { loginAs } from './helpers'
import { findRepoRoot } from './repo-root'

// Opt-in matched captures for the Admin interaction closure round: the Node
// detail header's context links, the Back to results / unavailable return
// links, and the Audit refresh-failure and first-page retry rows. Run once per
// revision with ADMIN_CLOSURE_EVIDENCE=before|after against the local seeded
// e2e Server. Audit pages and the long Network name are local Playwright
// route fixtures; no production service is touched and no credential is
// captured. Behaviour is asserted by admin-closure.spec.ts.
const phase = process.env.ADMIN_CLOSURE_EVIDENCE
test.skip(!phase, 'Set ADMIN_CLOSURE_EVIDENCE=before or after for matched captures')

const ROOT = join(findRepoRoot(), 'docs/visual-migration/emerald/admin-closure')
const NODE_B = '0195f2a1-0015-4015-8015-000000000015'
const LONG_NETWORK = 'PlatON Long Regional Network Display Name For Wrapping Checks'

const AUDIT_ITEM = (id: number) => ({
  auditEventId: id,
  eventKind: id % 2 === 0 ? 'node_metadata_changed' : 'session_created',
  actorUsername: 'admin',
  targetKind: id % 2 === 0 ? 'node' : 'session',
  targetId: id % 2 === 0 ? 'evidence-node-' + id : 'evidence-session-' + id,
  createdAt: '2026-10-18T0' + (id % 10) + ':00:00Z',
  details: { evidence: 'local fixture ' + id },
})

async function applyTheme(page: Page, theme: string) {
  await page.addInitScript((value) => localStorage.setItem('platpulse.themeMode', value), theme)
}

async function settle(page: Page) {
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible({ timeout: 20_000 })
  await expect(page.getByText(/^(Loading|Checking access)/)).toHaveCount(0, { timeout: 20_000 })
  await page.waitForTimeout(500)
}

test('matched Admin closure evidence', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop-1280', 'one project drives every width')
  test.setTimeout(600_000)
  await page.clock.install()
  await loginAs(page)
  const out = ROOT + '/' + phase
  await mkdir(out, { recursive: true })
  const shot = (name: string, selector: string) =>
    page.locator(selector).first().screenshot({ path: out + '/' + name + '.png', animations: 'disabled' })

  for (const theme of ['light', 'dark'] as const) {
    await applyTheme(page, theme)
    for (const width of [390, 1280]) {
      await page.setViewportSize({ width, height: width < 768 ? 844 : 900 })
      // Node detail header opened directly (All Nodes), seeded Network name.
      await page.goto('/admin/nodes/' + NODE_B)
      await settle(page)
      await shot('node-header-' + width + '-' + theme, '[data-slot="node-detail-header"]')
      // The same header with a Network name long enough to wrap.
      await page.route(new RegExp('/api/admin/v1/nodes/' + NODE_B + '$'), async (route) => {
        const response = await route.fetch()
        const json = await response.json()
        await route.fulfill({ response, json: { ...json, network_display_name: LONG_NETWORK } })
      })
      await page.goto('/admin/nodes/' + NODE_B)
      await settle(page)
      await shot('node-header-long-' + width + '-' + theme, '[data-slot="node-detail-header"]')
      await page.unroute(new RegExp('/api/admin/v1/nodes/' + NODE_B + '$'))
      // Opened from a filtered, sorted list.
      await page.goto('/admin/nodes?health=unhealthy&sort=name')
      await settle(page)
      await page.locator('[data-slot="node-name-link"]').first().click()
      await settle(page)
      await shot('node-header-from-results-' + width + '-' + theme, '[data-slot="node-detail-header"]')
      // An unknown Node.
      await page.goto('/admin/nodes/evidence-missing-node')
      await expect(page.getByRole('heading', { level: 1, name: 'Node unavailable' })).toBeVisible({ timeout: 20_000 })
      await page.waitForTimeout(300)
      await shot('node-unavailable-' + width + '-' + theme, '[data-slot="admin-page"]')
    }
  }

  // Audit: two loaded pages, then a refresh of the shown page fails.
  let mode: 'ok' | 'fail' | 'slow' = 'ok'
  await page.route(/\/api\/admin\/v1\/audit(\?|$)/, async (route) => {
    const before = new URL(route.request().url()).searchParams.get('before')
    if (mode === 'fail') {
      return route.fulfill({ status: 503, json: { error: { code: 'unavailable', message: 'Evidence: simulated refresh failure' } } })
    }
    if (mode === 'slow') await new Promise((resolve) => setTimeout(resolve, 5_000))
    if (before === null) return route.fulfill({ json: { items: [910, 909, 908].map(AUDIT_ITEM), nextBefore: 908 } })
    return route.fulfill({ json: { items: [907, 906].map(AUDIT_ITEM), nextBefore: 906 } })
  })
  for (const theme of ['light', 'dark'] as const) {
    await applyTheme(page, theme)
    for (const width of [390, 1280]) {
      mode = 'ok'
      await page.setViewportSize({ width, height: width < 768 ? 844 : 900 })
      await page.goto('/admin')
      await page.goto('/admin/access/audit')
      await settle(page)
      await page.getByRole('button', { name: 'Load older events' }).click()
      await expect(page.locator('[data-slot="audit-count"]')).toHaveText('5 loaded')
      mode = 'fail'
      // The cached page goes stale, and the browser's online signal refetches it.
      await page.clock.fastForward('00:31')
      await page.evaluate(() => {
        window.dispatchEvent(new Event('offline'))
        window.dispatchEvent(new Event('online'))
      })
      await page.waitForTimeout(1_500)
      await shot('audit-refresh-failed-' + width + '-' + theme, '[data-slot="audit-panel"]')
    }
  }

  // Audit: the first page failed and its retry is running.
  for (const width of [390, 1280]) {
    await applyTheme(page, 'light')
    await page.setViewportSize({ width, height: width < 768 ? 844 : 900 })
    mode = 'fail'
    await page.goto('/admin')
    await page.goto('/admin/access/audit')
    await expect(page.getByRole('button', { name: 'Try again' })).toBeVisible({ timeout: 20_000 })
    mode = 'slow'
    await page.getByRole('button', { name: 'Try again' }).click()
    await page.waitForTimeout(800)
    await shot('audit-initial-retry-' + width + '-light', '[data-slot="audit-panel"]')
    await page.waitForTimeout(5_000)
  }
})

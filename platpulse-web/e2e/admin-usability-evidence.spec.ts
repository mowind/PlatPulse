import { test, expect, type Page } from '@playwright/test'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { loginAs } from './helpers'
import { findRepoRoot } from './repo-root'

// Opt-in matched captures for the Admin usability round (marker tones, Nodes
// sort/empty/filter chips, Audit paging and deep-link feedback, Overview setup
// guide). Run once per revision with ADMIN_USABILITY_EVIDENCE=before|after
// against the local seeded e2e Server. Every non-seeded state is a local
// Playwright route fixture; no production service is touched and no
// credential is captured. Behaviour is asserted by admin-usability.spec.ts.
const phase = process.env.ADMIN_USABILITY_EVIDENCE
test.skip(!phase, 'Set ADMIN_USABILITY_EVIDENCE=before or after for matched captures')

const ROOT = join(findRepoRoot(), 'docs/visual-migration/emerald/admin-usability')
const NODE_B = '0195f2a1-0015-4015-8015-000000000015'
const AGENT = '0195f2a1-0011-4011-8011-000000000011'

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

test('matched Admin usability evidence', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop-1280', 'one project drives every width')
  test.setTimeout(600_000)
  await loginAs(page)
  const out = ROOT + '/' + phase
  await mkdir(out, { recursive: true })
  const shot = async (name: string, selector?: string) => {
    if (selector) await page.locator(selector).first().screenshot({ path: out + '/' + name + '.png', animations: 'disabled' })
    else await page.screenshot({ path: out + '/' + name + '.png', fullPage: true, animations: 'disabled' })
  }

  for (const theme of ['light', 'dark'] as const) {
    await applyTheme(page, theme)
    for (const width of [390, 1280]) {
      await page.setViewportSize({ width, height: width < 768 ? 844 : 900 })
      // Detail tab markers: an identity mismatch (Server error mapping) on
      // Node B; an unsaved draft on the Agent's Manage tab.
      await page.goto('/admin/nodes/' + NODE_B)
      await settle(page)
      await shot('node-detail-tabs-' + width + '-' + theme, '[data-slot="admin-node-detail"]')
      await page.goto('/admin/agents/' + AGENT + '?section=manage')
      await settle(page)
      await page.getByRole('textbox', { name: 'Display name' }).first().fill('Unsaved evidence draft')
      await page.waitForTimeout(200)
      await shot('agent-detail-tabs-' + width + '-' + theme, '[data-slot="admin-detail-sections"] [role="tablist"]')

      // Both secondary filters active while More filters is collapsed.
      await page.goto('/admin/nodes?visibility=private&lifecycle=retired')
      await settle(page)
      await shot('nodes-filter-chips-' + width + '-' + theme, '[data-slot="node-inventory"]')
      // Filters that match nothing in a non-empty inventory.
      await page.goto('/admin/nodes?visibility=public&lifecycle=retired&sort=name')
      await settle(page)
      await shot('nodes-filtered-empty-' + width + '-' + theme, '[data-slot="node-inventory"]')
    }
  }

  // An empty inventory (local fixture) at one width per theme.
  await page.route(/\/api\/admin\/v1\/nodes(\?|$)/, (route) => route.fulfill({ json: [] }))
  for (const theme of ['light', 'dark'] as const) {
    await applyTheme(page, theme)
    await page.setViewportSize({ width: 390, height: 844 })
    await page.goto('/admin/nodes')
    await settle(page)
    await shot('nodes-inventory-empty-390-' + theme, '[data-slot="node-inventory"]')
  }

  // First-use Overview (all three surfaces authoritatively empty).
  await page.route(/\/api\/admin\/v1\/agents(\?|$)/, (route) => route.fulfill({ json: [] }))
  await page.route(/\/api\/admin\/v1\/overview(\?|$)/, (route) =>
    route.fulfill({
      json: {
        generated_at: new Date().toISOString(),
        summary: {
          agents: { total: 0, online: 0, offline: 0, unknown: 0 },
          nodes: { total: 0, active: 0, healthy: 0, unhealthy: 0, unknown: 0, retired: 0 },
          networks: { total: 0, with_identity_mismatch: 0 },
        },
        attention: [],
      },
    }),
  )
  for (const theme of ['light', 'dark'] as const) {
    await applyTheme(page, theme)
    for (const width of [390, 1280]) {
      await page.setViewportSize({ width, height: width < 768 ? 844 : 900 })
      await page.goto('/admin')
      await settle(page)
      await expect(page.locator('[data-slot="setup-guide"]')).toBeVisible({ timeout: 20_000 })
      await shot('overview-setup-' + width + '-' + theme, '[data-slot="setup-guide"]')
    }
  }

  // Audit: a loaded first page, a failed older page with its retry, and a
  // deep link to an event outside the loaded results.
  let olderAttempts = 0
  await page.route(/\/api\/admin\/v1\/audit(\?|$)/, async (route) => {
    const url = new URL(route.request().url())
    const before = url.searchParams.get('before')
    if (before === null) {
      return route.fulfill({ json: { items: [910, 909, 908, 907].map(AUDIT_ITEM), nextBefore: 907 } })
    }
    olderAttempts += 1
    if (olderAttempts === 1) {
      return route.fulfill({ status: 503, json: { error: { code: 'unavailable', message: 'Evidence: simulated older-page failure' } } })
    }
    return route.fulfill({ json: { items: [906, 905].map(AUDIT_ITEM), nextBefore: null } })
  })
  for (const width of [390, 1280]) {
    olderAttempts = 0
    await applyTheme(page, 'light')
    await page.setViewportSize({ width, height: width < 768 ? 844 : 900 })
    // Leave the page first: a goto that only repeats the hash would not reload.
    await page.goto('/admin')
    await page.goto('/admin/access/audit#event-42')
    await settle(page)
    await shot('audit-deep-link-' + width + '-light', '[data-slot="audit-panel"]')
    await page.getByRole('button', { name: /Load older events/ }).click()
    await page.waitForTimeout(800)
    await shot('audit-older-failed-' + width + '-light', '[data-slot="audit-panel"]')
  }
})

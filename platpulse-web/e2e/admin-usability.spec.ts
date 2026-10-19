import { test, expect, type Locator, type Page } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'
import { focusByKeyboard } from './admin-flow'

// Admin usability round against the seeded local e2e Server: tab marker
// tones, Nodes sort in the URL, the two empty states, More filters chips with
// laid-out 44px clear targets, Audit paging feedback and deep links, and the
// Overview first-use guide. States the seed cannot produce are local
// Playwright route fixtures; nothing outside the local Server is touched.
const NODE_B = '0195f2a1-0015-4015-8015-000000000015'
const AGENT = '0195f2a1-0011-4011-8011-000000000011'
const THEMES = ['light', 'dark'] as const

type Box = { x: number; y: number; width: number; height: number }
const overlaps = (a: Box, b: Box) =>
  a.x < b.x + b.width - 0.5 && b.x < a.x + a.width - 0.5 && a.y < b.y + b.height - 0.5 && b.y < a.y + a.height - 0.5
const contains = (outer: Box, inner: Box) =>
  inner.x >= outer.x - 0.5 &&
  inner.y >= outer.y - 0.5 &&
  inner.x + inner.width <= outer.x + outer.width + 0.5 &&
  inner.y + inner.height <= outer.y + outer.height + 0.5

async function applyTheme(page: Page, theme: string) {
  await page.addInitScript((value) => localStorage.setItem('platpulse.themeMode', value), theme)
}

/** Points just inside every edge of a control hit the control itself. */
async function expectOwnHitArea(control: Locator) {
  await control.evaluate((element) => element.scrollIntoView({ block: 'center' }))
  const misses = await control.evaluate((element) => {
    const rect = element.getBoundingClientRect()
    const points = [
      [rect.left + rect.width / 2, rect.top + 2],
      [rect.left + rect.width / 2, rect.bottom - 2],
      [rect.left + 2, rect.top + rect.height / 2],
      [rect.right - 2, rect.top + rect.height / 2],
    ]
    return points
      .filter(([x, y]) => !element.contains(document.elementFromPoint(x, y)))
      .map(([x, y]) => Math.round(x) + ',' + Math.round(y) + ' -> ' + document.elementFromPoint(x, y)?.outerHTML.slice(0, 80))
  })
  expect(misses, 'points inside the control hit it').toEqual([])
}

test.beforeEach(async ({ page }) => {
  await loginAs(page)
})

for (const theme of THEMES) {
  test('detail tab markers keep error, neutral and draft cues apart (' + theme + ')', async ({ page }) => {
    await applyTheme(page, theme)
    await page.goto('/admin/nodes/' + NODE_B)
    const diagnostics = page.getByRole('tab', { name: /^Diagnostics, \d+ current finding/ })
    await expect(diagnostics).toBeVisible({ timeout: 20_000 })
    // Node B's identity mismatch keeps its existing error mapping.
    await expect(diagnostics.locator('[data-slot="detail-section-marker"]')).toHaveAttribute('data-tone', 'error')

    await page.goto('/admin/agents/' + AGENT + '?section=manage')
    const manage = page.getByRole('tab', { name: /^Manage/ })
    await expect(manage).toHaveAttribute('aria-selected', 'true', { timeout: 20_000 })
    await page.getByRole('textbox', { name: 'Display name' }).fill('Unsaved usability draft')
    const unsaved = manage.locator('[data-slot="detail-section-marker"]')
    await expect(unsaved).toHaveText('Unsaved')
    await expect(unsaved).toHaveAttribute('data-tone', 'neutral')
    await expect(page.getByRole('tab', { name: /^Manage, unsaved change or open confirmation/ })).toBeVisible()
    // A neutral marker does not borrow the warning or error colour.
    const colours = await unsaved.evaluate((element) => {
      const probe = (cls: string) => {
        const node = document.createElement('span')
        node.className = cls
        document.body.append(node)
        const value = getComputedStyle(node).backgroundColor
        node.remove()
        return value
      }
      return {
        own: getComputedStyle(element).backgroundColor,
        warning: probe('bg-warning/15'),
        error: probe('bg-destructive/10'),
      }
    })
    expect(colours.own).not.toBe(colours.warning)
    expect(colours.own).not.toBe(colours.error)

    // Switching away keeps the draft; the hidden controls leave the tab order;
    // switching back restores it.
    await page.getByRole('tab', { name: 'Overview' }).click()
    await expect(page.getByRole('textbox', { name: 'Display name' })).toHaveCount(0)
    await page.getByRole('tab', { name: /^Manage/ }).click()
    await expect(page.getByRole('textbox', { name: 'Display name' })).toHaveValue('Unsaved usability draft')
    await expectNoHorizontalOverflow(page)
  })
}

test('Nodes sort survives reload and back/forward, and Clear filters keeps it', async ({ page }) => {
  await page.goto('/admin/nodes?lifecycle=active')
  await expect(page.locator('[data-slot="node-table"]')).toBeVisible({ timeout: 20_000 })
  await page.getByLabel('Sort by').selectOption('name')
  await expect(page).toHaveURL(/sort=name/)
  await expect(page).toHaveURL(/lifecycle=active/)
  await page.reload()
  await expect(page.getByLabel('Sort by')).toHaveValue('name', { timeout: 20_000 })
  await expect(page.locator('[data-slot="node-table"] caption')).toContainText('Sorted by Node name.')
  const viewport = page.viewportSize()!
  if (viewport.width >= 768) {
    await expect(page.locator('[data-slot="node-table"] thead th[aria-sort="ascending"]')).toHaveText(/Node/)
  }
  await page.getByLabel('Sort by').selectOption('freshness')
  await expect(page).toHaveURL(/sort=freshness/)
  await page.goBack()
  await expect(page.getByLabel('Sort by')).toHaveValue('name')
  await page.goForward()
  await expect(page.getByLabel('Sort by')).toHaveValue('freshness')
  await page.getByRole('button', { name: 'Clear filters' }).first().click()
  await expect(page).not.toHaveURL(/lifecycle=/)
  await expect(page).toHaveURL(/sort=freshness/)
  await expect(page.getByLabel('Sort by')).toHaveValue('freshness')
})

test('Nodes tells an empty inventory apart from filters that match nothing', async ({ page }) => {
  await page.goto('/admin/nodes?visibility=public&lifecycle=retired&sort=name')
  await expect(page.getByText('No Nodes match these filters.')).toBeVisible({ timeout: 20_000 })
  const panel = page.locator('[data-slot="node-inventory"]')
  await panel.locator('[data-slot="node-filtered-empty"]').locator('..').getByRole('button', { name: 'Clear filters' }).click()
  await expect(page.locator('[data-slot="node-table"]')).toBeVisible()
  await expect(page).toHaveURL(/\?sort=name$/)

  await page.route(/\/api\/admin\/v1\/nodes(\?|$)/, (route) => route.fulfill({ json: [] }))
  await page.goto('/admin/nodes')
  await expect(page.getByText('No Nodes have been reported yet.')).toBeVisible({ timeout: 20_000 })
  await expect(panel.getByRole('link', { name: 'Open Agents' })).toHaveAttribute('href', '/admin/agents')
  await expect(page.getByText('No Nodes match these filters.')).toHaveCount(0)
  await expectNoHorizontalOverflow(page)
})

const LONG_VISIBILITY = 'an-unrecognised-visibility-value-from-an-old-bookmark-that-is-long-enough-to-wrap'

for (const theme of THEMES) {
  test('More filters chips reserve their own 44px clear targets when they wrap (' + theme + ')', async ({ page }) => {
    await applyTheme(page, theme)
    for (const query of ['?visibility=private&lifecycle=retired', '?visibility=' + LONG_VISIBILITY + '&lifecycle=retired']) {
      await page.goto('/admin/nodes' + query)
      const chips = page.locator('[data-slot="node-active-filter"]')
      await expect(chips).toHaveCount(2, { timeout: 20_000 })
      await expect(page.getByRole('button', { name: /^More filters\s*2\s*active$/ })).toHaveAttribute('aria-expanded', 'false')
      const clears = page.getByRole('button', { name: /^Clear (Visibility|Lifecycle):/ })
      const toolbarControls = page.locator('[data-slot="admin-toolbar"]').locator('button, select')
      const toolbarBoxes: Box[] = []
      for (let index = 0; index < (await toolbarControls.count()); index += 1) {
        toolbarBoxes.push((await toolbarControls.nth(index).boundingBox())!)
      }
      const clearBoxes: Box[] = []
      for (let index = 0; index < 2; index += 1) {
        const chip = (await chips.nth(index).boundingBox())!
        const clear = (await clears.nth(index).boundingBox())!
        expect(Math.round(clear.width)).toBeGreaterThanOrEqual(44)
        expect(Math.round(clear.height)).toBeGreaterThanOrEqual(44)
        // The touch area is part of the chip's own laid-out box.
        expect(contains(chip, clear), 'clear target stays inside its chip').toBe(true)
        for (const control of toolbarBoxes) expect(overlaps(clear, control)).toBe(false)
        clearBoxes.push(clear)
      }
      expect(overlaps(clearBoxes[0], clearBoxes[1])).toBe(false)
      expect(overlaps((await chips.nth(0).boundingBox())!, (await chips.nth(1).boundingBox())!)).toBe(false)
      // Hit testing scrolls each control into view, so it runs after every
      // box above was measured in one scroll position.
      for (let index = 0; index < 2; index += 1) await expectOwnHitArea(clears.nth(index))
      await expectNoHorizontalOverflow(page)
    }
    // A real click clears only its own filter.
    await page.getByRole('button', { name: 'Clear Lifecycle: Retired' }).click()
    await expect(page).not.toHaveURL(/lifecycle=/)
    await expect(page).toHaveURL(/visibility=/)
    await expect(page.locator('[data-slot="node-active-filter"]')).toHaveCount(1)
    // Opening and closing More filters never clears a condition.
    const more = page.getByRole('button', { name: /^More filters/ })
    await more.click()
    await expect(more).toHaveAttribute('aria-expanded', 'true')
    await more.click()
    await expect(page.locator('[data-slot="node-active-filter"]')).toHaveCount(1)
  })
}

test('Node table sort buttons stay inside their header row and keep a visible focus ring', async ({ page }) => {
  test.skip(page.viewportSize()!.width < 768, 'phones sort with the Sort by control; the header is visually hidden')
  await page.goto('/admin/nodes')
  const table = page.locator('[data-slot="node-table"]')
  await expect(table).toBeVisible({ timeout: 20_000 })
  for (const name of ['Node', 'Health', 'Freshness']) {
    const button = table.getByRole('button', { name: new RegExp('^' + name) })
    const box = (await button.boundingBox())!
    expect(Math.round(box.height)).toBeGreaterThanOrEqual(44)
    const geometry = await button.evaluate((element) => {
      const scroller = element.closest('.overflow-x-auto')!.getBoundingClientRect()
      const row = element.closest('tr')!.getBoundingClientRect()
      const next = element.closest('thead')!.nextElementSibling!.querySelector('tr')!.getBoundingClientRect()
      const rect = element.getBoundingClientRect()
      return { top: rect.top - scroller.top, bottom: next.top - rect.bottom, rowTop: rect.top - row.top, rowBottom: row.bottom - rect.bottom }
    })
    // The target is not cut by the table's scroller, stays inside its header
    // row and does not reach into the first body row.
    expect(geometry.top, 'sort button starts inside the scroller').toBeGreaterThanOrEqual(-0.5)
    expect(geometry.rowTop, 'sort button starts inside its header row').toBeGreaterThanOrEqual(-0.5)
    expect(geometry.rowBottom, 'sort button ends inside its header row').toBeGreaterThanOrEqual(-0.5)
    expect(geometry.bottom, 'sort button stays out of the first row').toBeGreaterThanOrEqual(-0.5)
    await expectOwnHitArea(button)
  }
  // Keyboard focus draws its ring inside the button, where the scroller
  // cannot clip it.
  const focused = await focusByKeyboard(page, table.getByRole('button', { name: /^Node/ }))
  const shadow = await focused.evaluate((element) => getComputedStyle(element).boxShadow)
  expect(shadow).toContain('inset')
  await table.getByRole('button', { name: /^Health/ }).click()
  await expect(page.getByLabel('Sort by')).toHaveValue('health')
  await table.getByRole('button', { name: /^Node/ }).click()
  await expect(page).toHaveURL(/sort=name/)
  await expect(page.getByLabel('Sort by')).toHaveValue('name')
})

const AUDIT_ITEM = (id: number) => ({
  auditEventId: id,
  eventKind: 'node_metadata_changed',
  actorUsername: 'admin',
  targetKind: 'node',
  targetId: 'usability-node-' + id,
  createdAt: '2026-10-18T08:00:00Z',
  details: { evidence: 'local fixture ' + id },
})

test('Audit keeps loaded events through a failed older page, retries its cursor and opens a linked event', async ({ page }) => {
  const befores: (string | null)[] = []
  let olderAttempts = 0
  await page.route(/\/api\/admin\/v1\/audit(\?|$)/, async (route) => {
    const before = new URL(route.request().url()).searchParams.get('before')
    befores.push(before)
    if (before === null) return route.fulfill({ json: { items: [910, 909, 908].map(AUDIT_ITEM), nextBefore: 908 } })
    olderAttempts += 1
    if (olderAttempts === 1) {
      return route.fulfill({ status: 503, json: { error: { code: 'unavailable', message: 'Simulated older-page failure' } } })
    }
    // The older page overlaps the loaded one by one event.
    return route.fulfill({ json: { items: [908, 907, 906].map(AUDIT_ITEM), nextBefore: null } })
  })
  await page.goto('/admin/access/audit#event-906')
  const notice = page.locator('[data-slot="audit-deep-link-notice"]')
  await expect(notice).toContainText('Event 906 is not in the 3 loaded events.', { timeout: 20_000 })
  await expect(notice).not.toContainText(/older than|deleted|does not exist/)
  const rows = page.locator('[data-slot="audit-table"] tbody tr[id^="event-"]')
  await rows.first().getByRole('button', { name: 'Show details' }).click()

  await page.getByRole('button', { name: 'Load older events' }).click()
  const failure = page.locator('[data-slot="audit-older-error"]')
  await expect(failure).toContainText('Simulated older-page failure')
  await expect(rows).toHaveCount(3)
  await expect(page.locator('[data-slot="audit-count"]')).toHaveText('3 loaded')
  await expect(notice).toContainText('Loading older events failed; try again below.')

  await page.getByRole('button', { name: 'Try loading older events again' }).click()
  await expect(rows).toHaveCount(5)
  expect(befores).toEqual([null, '908', '908'])
  await expect(page.locator('[data-slot="audit-count"]')).toHaveText('5 loaded')
  await expect(failure).toHaveCount(0)
  await expect(notice).toHaveCount(0)
  // Event 910 stays expanded; the linked 906 opens once it is loaded.
  await expect(page.getByRole('region', { name: 'Redacted details for Audit event 910' })).toBeVisible()
  await expect(page.getByRole('region', { name: 'Redacted details for Audit event 906' })).toBeVisible()
  await expect(page.locator('[data-slot="audit-older-end"]')).toHaveText('No older events for the current filters.')
  await expectNoHorizontalOverflow(page)
})

test('Overview first-use guide links only real Server pages and keeps Agent configuration local', async ({ page }) => {
  await page.route(/\/api\/admin\/v1\/(nodes|agents)(\?|$)/, (route) => route.fulfill({ json: [] }))
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
  await page.goto('/admin')
  const guide = page.locator('[data-slot="setup-guide"]')
  await expect(guide).toBeVisible({ timeout: 20_000 })
  await expect(guide.locator('a')).toHaveCount(2)
  await expect(guide.getByRole('link', { name: 'Register the expected Network identity' })).toHaveAttribute('href', '/admin/networks')
  await expect(guide.getByRole('link', { name: 'Add an Agent' })).toHaveAttribute('href', '/admin/agents/enroll')
  await expect(guide.locator('a[href="/admin/settings"]')).toHaveCount(0)
  await expect(guide.locator('[data-slot="setup-local-inventory"]')).toContainText("On the Agent's Host")
  await guide.getByRole('link', { name: 'Add an Agent' }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Enroll a new Agent' })).toBeVisible()
})

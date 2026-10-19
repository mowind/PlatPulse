import { test, expect, type Locator, type Page } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'
import { focusByKeyboard } from './admin-flow'

// Admin interaction closure round against the seeded local e2e Server: the
// navigation drawer across the sidebar breakpoint, Audit refresh failures with
// earlier events kept, the Node detail header's context-link targets, and the
// return from a Node detail to the Nodes list reading it was opened from.
// Audit pages and a long Network name are local Playwright route fixtures;
// nothing outside the local Server is touched and nothing is written.
const NODE_B = '0195f2a1-0015-4015-8015-000000000015'
const AGENT = '0195f2a1-0011-4011-8011-000000000011'
const LONG_NETWORK = 'PlatON Long Regional Network Display Name For Wrapping Checks'

type Box = { x: number; y: number; width: number; height: number }
const overlaps = (a: Box, b: Box) =>
  a.x < b.x + b.width - 0.5 && b.x < a.x + a.width - 0.5 && a.y < b.y + b.height - 0.5 && b.y < a.y + a.height - 0.5

test.beforeEach(async ({ page }) => {
  await loginAs(page)
})

/** Points just inside every edge of a control hit the control itself. */
async function expectOwnHitArea(control: Locator) {
  const misses = await control.evaluate((element) => {
    const rect = element.getBoundingClientRect()
    const points = [
      [rect.left + rect.width / 2, rect.top + 2],
      [rect.left + rect.width / 2, rect.bottom - 2],
      [rect.left + 2, rect.top + rect.height / 2],
      [rect.right - 2, rect.top + rect.height / 2],
      // Inside the rounded corners, which are not part of a hit area.
      [rect.left + 8, rect.bottom - 2],
      [rect.right - 8, rect.bottom - 2],
    ]
    return points
      .filter(([x, y]) => !element.contains(document.elementFromPoint(x, y)))
      .map(([x, y]) => Math.round(x) + ',' + Math.round(y) + ' -> ' + document.elementFromPoint(x, y)?.outerHTML.slice(0, 80))
  })
  expect(misses, 'points inside the control hit it').toEqual([])
}

const focusedInNav = (page: Page) =>
  page.evaluate(() => Boolean(document.activeElement?.closest('[data-slot="admin-nav"]')))

test.describe('drawer across the sidebar breakpoint', () => {
  test('an open drawer ends when the viewport becomes desktop, and stays closed back on a phone', async ({ page }, info) => {
    test.skip(!['phone-390-touch', 'desktop-1280'].includes(info.project.name), 'drives its own viewports')
    await page.setViewportSize({ width: 390, height: 844 })
    await page.goto('/admin/nodes')
    await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible({ timeout: 20_000 })
    // An earlier inline overflow value is what the drawer must give back.
    await page.evaluate(() => (document.body.style.overflow = 'visible'))
    const menu = page.locator('[data-slot="admin-nav-toggle"]')
    const nav = page.getByRole('navigation', { name: 'Admin' })
    await menu.click()
    await expect(menu).toHaveAttribute('aria-expanded', 'true')
    await expect.poll(() => page.evaluate(() => document.body.style.overflow)).toBe('hidden')
    await expect(nav.getByRole('link', { name: 'Overview', exact: true })).toBeFocused()

    // Cross into the sidebar layout without closing the drawer.
    await page.setViewportSize({ width: 1280, height: 800 })
    await expect(menu).toBeHidden()
    await expect(menu).toHaveAttribute('aria-expanded', 'false')
    await expect.poll(() => page.evaluate(() => document.body.style.overflow)).toBe('visible')
    await expect(page.locator('[data-slot="admin-nav-scrim"]')).toBeHidden()
    // Focus stayed on the link, now in the visible sidebar, not on the hidden Menu.
    await expect(nav.getByRole('link', { name: 'Overview', exact: true })).toBeFocused()
    // The page scrolls again.
    await page.evaluate(() => window.scrollTo(0, 400))
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(0)
    await page.evaluate(() => window.scrollTo(0, 0))
    // Tab leaves the last sidebar link for the page instead of wrapping.
    await nav.getByRole('link', { name: 'Audit', exact: true }).focus()
    await page.keyboard.press('Tab')
    expect(await focusedInNav(page)).toBe(false)
    await expect.poll(() => page.evaluate(() => Boolean(document.activeElement?.closest('[data-slot="admin-main"]')))).toBe(true)
    // Escape on the sidebar changes nothing.
    await nav.getByRole('link', { name: 'Nodes', exact: true }).focus()
    await page.keyboard.press('Escape')
    await expect(nav.getByRole('link', { name: 'Nodes', exact: true })).toBeFocused()

    // Back to a phone: the drawer does not reappear.
    await page.setViewportSize({ width: 390, height: 844 })
    await expect(menu).toBeVisible()
    await expect(menu).toHaveAttribute('aria-expanded', 'false')
    await expect(nav).not.toBeInViewport()
    await expect.poll(() => page.evaluate(() => document.body.style.overflow)).toBe('visible')

    // The ordinary phone contract is intact: open, Escape, focus back to Menu.
    await menu.click()
    await expect(nav.getByRole('link', { name: 'Overview', exact: true })).toBeFocused()
    await page.keyboard.press('Escape')
    await expect(menu).toBeFocused()
    await expect.poll(() => page.evaluate(() => document.body.style.overflow)).toBe('visible')
    await expectNoHorizontalOverflow(page)
  })

  test('script and layout agree on the breakpoint: 1023px is a drawer, 1024px the sidebar', async ({ page }, info) => {
    test.skip(info.project.name !== 'desktop-1280', 'drives its own viewports')
    await page.setViewportSize({ width: 1023, height: 800 })
    await page.goto('/admin')
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible({ timeout: 20_000 })
    const menu = page.locator('[data-slot="admin-nav-toggle"]')
    await expect(menu).toBeVisible()
    await menu.click()
    await expect.poll(() => page.evaluate(() => document.body.style.overflow)).toBe('hidden')
    await page.setViewportSize({ width: 1024, height: 800 })
    await expect(menu).toBeHidden()
    await expect(menu).toHaveAttribute('aria-expanded', 'false')
    await expect.poll(() => page.evaluate(() => document.body.style.overflow)).toBe('')
    await page.setViewportSize({ width: 1023, height: 800 })
    await expect(menu).toBeVisible()
    await expect(page.getByRole('navigation', { name: 'Admin' })).not.toBeInViewport()
  })
})

const AUDIT_ITEM = (id: number) => ({
  auditEventId: id,
  eventKind: 'node_metadata_changed',
  actorUsername: 'admin',
  targetKind: 'node',
  targetId: 'closure-node-' + id,
  createdAt: '2026-10-18T08:00:00Z',
  details: { evidence: 'local fixture ' + id },
})

/** Age the cached Audit page past its freshness and send the browser's online
 *  signal, which refetches the shown query — no Realtime event involved. */
async function refreshAudit(page: Page) {
  await page.clock.fastForward('00:31')
  await page.evaluate(() => {
    window.dispatchEvent(new Event('offline'))
    window.dispatchEvent(new Event('online'))
  })
}

test('Audit keeps earlier events when a refresh fails, after one page and after several, and retries the same query', async ({ page }) => {
  await page.clock.install()
  const requests: string[] = []
  let mode: 'ok' | 'fail' | 'slow' = 'ok'
  await page.route(/\/api\/admin\/v1\/audit(\?|$)/, async (route) => {
    const before = new URL(route.request().url()).searchParams.get('before')
    requests.push(String(before) + ':' + mode)
    if (mode === 'fail') {
      return route.fulfill({ status: 503, json: { error: { code: 'unavailable', message: 'Simulated refresh failure' } } })
    }
    if (mode === 'slow') await new Promise((resolve) => setTimeout(resolve, 1_500))
    if (before === null) return route.fulfill({ json: { items: [920, 919, 918].map(AUDIT_ITEM), nextBefore: 918 } })
    return route.fulfill({ json: { items: [918, 917, 916].map(AUDIT_ITEM), nextBefore: 916 } })
  })
  await page.goto('/admin/access/audit')
  const rows = page.locator('[data-slot="audit-table"] tbody tr[id^="event-"]')
  const count = page.locator('[data-slot="audit-count"]')
  const refreshRow = page.locator('[data-slot="audit-refresh-error"]')
  await expect(rows).toHaveCount(3, { timeout: 20_000 })
  await rows.first().getByRole('button', { name: 'Show details' }).click()

  // B: the first page's refresh fails.
  mode = 'fail'
  await refreshAudit(page)
  await expect(refreshRow).toContainText('Refreshing the Audit log failed: Simulated refresh failure. Showing the 3 events loaded before.')
  await expect(page.getByRole('alert')).toHaveCount(1)
  await expect(rows).toHaveCount(3)
  await expect(count).toHaveText('3 loaded')
  await expect(page.getByRole('region', { name: 'Redacted details for Audit event 920' })).toBeVisible()
  // A successful refresh clears it.
  mode = 'ok'
  await refreshRow.getByRole('button', { name: 'Try refreshing again' }).click()
  await expect(refreshRow).toHaveCount(0)

  // C: several pages are loaded and the shown (cursor 918) page's refresh fails.
  await page.getByRole('button', { name: 'Load older events' }).click()
  await expect(count).toHaveText('5 loaded')
  mode = 'fail'
  const before = requests.length
  await refreshAudit(page)
  await expect(refreshRow).toContainText('Showing the 5 events loaded before.')
  expect(requests.slice(before)).toEqual(['918:fail'])
  await expect(rows).toHaveCount(5)
  await expect(page.getByRole('alert')).toHaveCount(1)
  await expect(page.locator('[data-slot="audit-older-error"]')).toHaveCount(0)

  // The retry asks for the same cursor once, says so, and restores the view.
  mode = 'slow'
  const retry = refreshRow.getByRole('button', { name: 'Try refreshing again' })
  await retry.focus()
  await page.keyboard.press('Enter')
  const busy = refreshRow.getByRole('button', { name: 'Refreshing…' })
  await expect(busy).toHaveAttribute('aria-disabled', 'true')
  await expect(busy).toBeFocused()
  await busy.click({ force: true })
  await expect(refreshRow).toHaveCount(0, { timeout: 10_000 })
  expect(requests.slice(before)).toEqual(['918:fail', '918:slow'])
  await expect(rows).toHaveCount(5)
  await expect(count).toHaveText('5 loaded')
  await expect(page.getByRole('alert')).toHaveCount(0)
  await expect(page.getByRole('region', { name: 'Redacted details for Audit event 920' })).toBeVisible()
  await expect(page.locator('[data-slot="audit-table"]')).toBeFocused()
  await expect(page.getByRole('button', { name: 'Load older events' })).toBeVisible()
  await expectNoHorizontalOverflow(page)
})

test('Audit first-page retry keeps its button, focus and feedback while it runs', async ({ page }) => {
  let attempts = 0
  await page.route(/\/api\/admin\/v1\/audit(\?|$)/, async (route) => {
    attempts += 1
    if (attempts === 1) {
      return route.fulfill({ status: 503, json: { error: { code: 'unavailable', message: 'Simulated first-page failure' } } })
    }
    await new Promise((resolve) => setTimeout(resolve, 1_500))
    return route.fulfill({ json: { items: [930, 929].map(AUDIT_ITEM), nextBefore: null } })
  })
  await page.goto('/admin/access/audit')
  const retry = page.getByRole('button', { name: 'Try again' })
  await expect(retry).toBeVisible({ timeout: 20_000 })
  await retry.click()
  const busy = page.getByRole('button', { name: 'Trying again…' })
  await expect(busy).toBeFocused()
  await expect(page.locator('[data-slot="audit-initial-error"]')).toContainText('Loading the Audit log again…')
  await busy.click({ force: true })
  await expect(page.locator('[data-slot="audit-table"] tbody tr[id^="event-"]')).toHaveCount(2, { timeout: 10_000 })
  expect(attempts).toBe(2)
  await expect(page.locator('[data-slot="audit-initial-error"]')).toHaveCount(0)
})

for (const longName of [false, true]) {
  test('Node detail context links keep separate 44px targets' + (longName ? ' with a wrapping Network name' : ''), async ({ page }) => {
    if (longName) {
      await page.route(new RegExp('/api/admin/v1/nodes/' + NODE_B + '$'), async (route) => {
        const response = await route.fetch()
        const json = await response.json()
        await route.fulfill({ response, json: { ...json, network_display_name: LONG_NETWORK } })
      })
    }
    await page.goto('/admin/nodes/' + NODE_B)
    const header = page.locator('[data-slot="node-detail-header"]')
    await expect(header).toBeVisible({ timeout: 20_000 })
    const network = header.getByRole('link', { name: longName ? LONG_NETWORK : /PlatON/ }).first()
    const agent = header.locator('[data-slot="node-detail-context"] a').nth(1)
    const showFull = header.locator('[data-slot="admin-identifier"] button').first()
    await expect(showFull).toHaveAccessibleName(/Show full\s*Node ID/)
    const others = [header.getByRole('link', { name: 'All Nodes' }), header.getByRole('link', { name: 'Incidents for this Node' }), header.getByRole('link', { name: 'Investigate this Node' })]
    const boxes: Box[] = []
    for (const control of [network, agent, showFull, ...others]) {
      const box = (await control.boundingBox())!
      expect(Math.round(box.height)).toBeGreaterThanOrEqual(44)
      expect(Math.round(box.width)).toBeGreaterThanOrEqual(44)
      boxes.push(box)
    }
    for (let a = 0; a < boxes.length; a += 1) {
      for (let b = a + 1; b < boxes.length; b += 1) expect(overlaps(boxes[a], boxes[b]), 'targets ' + a + ' and ' + b).toBe(false)
    }
    for (const control of [network, agent, showFull]) await expectOwnHitArea(control)

    // The keyboard focus ring is drawn whole: no clipping ancestor up to the header.
    await focusByKeyboard(page, agent)
    const ring = await agent.evaluate((element) => {
      const style = getComputedStyle(element)
      let clipped = false
      for (let node = element.parentElement; node && node.dataset.slot !== 'node-detail-header'; node = node.parentElement) {
        const overflow = getComputedStyle(node)
        if (/hidden|clip|scroll|auto/.test(overflow.overflowX + overflow.overflowY)) clipped = true
      }
      return { style: style.outlineStyle, width: parseFloat(style.outlineWidth), clipped }
    })
    expect(ring.style).not.toBe('none')
    expect(ring.width).toBeGreaterThan(0)
    expect(ring.clipped).toBe(false)

    // A real click at the bottom edge of the Agent link opens the Agent, not Show full.
    const agentBox = (await agent.boundingBox())!
    await page.mouse.click(agentBox.x + 6, agentBox.y + agentBox.height - 3)
    await expect(page).toHaveURL(new RegExp('/admin/agents/' + AGENT))
    await page.goBack()
    await expect(showFull).toHaveAttribute('aria-expanded', 'false')
    const toggleBox = (await showFull.boundingBox())!
    await page.mouse.click(toggleBox.x + toggleBox.width / 2, toggleBox.y + 3)
    await expect(showFull).toHaveAttribute('aria-expanded', 'true')
    await expect(page).toHaveURL(new RegExp('/admin/nodes/' + NODE_B))
    await expectNoHorizontalOverflow(page)
  })
}

test('Back to results returns from a filtered list through section switches; Back still leaves the page', async ({ page }) => {
  const list = '/admin/nodes?health=unhealthy&sort=name'
  await page.goto(list)
  await expect(page.locator('[data-slot="node-table"]')).toBeVisible({ timeout: 20_000 })
  await page.locator('[data-slot="node-name-link"]').first().click()
  const back = page.getByRole('link', { name: 'Back to results' })
  await expect(back).toBeVisible({ timeout: 20_000 })
  await expect(page.getByRole('link', { name: 'All Nodes' })).toHaveCount(0)
  expect(new URL(page.url()).search).toBe('')
  await page.getByRole('tab', { name: /^Diagnostics/ }).click()
  await expect(page).toHaveURL(/\?section=diagnostics$/)
  await page.getByRole('tab', { name: /^History/ }).click()
  await expect(page).toHaveURL(/\?section=history$/)
  await expect(back).toHaveAttribute('href', list)
  // The browser's own Back leaves the detail for the list, as before.
  await page.goBack()
  await expect(page).toHaveURL(new RegExp(list.replace(/[?]/g, '\\?') + '$'))
  await page.goForward()
  await expect(back).toBeVisible({ timeout: 20_000 })
  await back.click()
  await expect(page).toHaveURL(new RegExp(list.replace(/[?]/g, '\\?') + '$'))
  await expect(page.getByLabel('Sort by')).toHaveValue('name')
  await expect(page.locator('[data-slot="admin-toolbar"] select').nth(1)).toHaveValue('unhealthy')

  // The row's Open Node diagnostics entry carries the same return.
  const firstRow = page.locator('[data-slot="node-table"] tbody tr').first()
  await firstRow.getByRole('button', { name: /^Details/ }).click()
  await page.getByRole('link', { name: 'Open Node diagnostics' }).first().click()
  await expect(page).toHaveURL(/\?section=diagnostics$/)
  await expect(page.getByRole('link', { name: 'Back to results' })).toHaveAttribute('href', list)
})

test('a detail opened directly and an unavailable Node fall back to the complete list', async ({ page }) => {
  await page.goto('/admin/nodes/' + NODE_B + '?section=manage')
  await expect(page.getByRole('link', { name: 'All Nodes' })).toHaveAttribute('href', '/admin/nodes', { timeout: 20_000 })
  await expect(page.getByRole('link', { name: 'Back to results' })).toHaveCount(0)

  await page.goto('/admin/nodes/closure-missing-node')
  await expect(page.getByRole('heading', { level: 1, name: 'Node unavailable' })).toBeVisible({ timeout: 20_000 })
  const back = page.getByRole('link', { name: 'All Nodes' })
  const box = (await back.boundingBox())!
  expect(Math.round(box.height)).toBeGreaterThanOrEqual(44)
  await back.click()
  await expect(page).toHaveURL(/\/admin\/nodes$/)
  await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible()
})

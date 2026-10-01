import { expect, test } from '@playwright/test'
import {
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  loginAs,
  setPageZoom,
} from './helpers'

// Node E is dedicated to this file's SSE-refetch mutation (see seed in
// start-server.sh): renamed and restored through the Admin API, it never
// shares mutation state with the Nodes page metadata test (Node C).
const NODE_E = '0195f2a1-0018-4018-8018-000000000018'

/** Change a Node's Server-owned display name through the real Admin API
 * from an authenticated page. The Overview surface itself carries no
 * mutations (issue #93); an audited metadata change publishes the Admin
 * `node` invalidation the refetch test needs. The call runs inside the
 * page so the browser sends a same-origin Origin header; the Server's
 * mutation guard requires it alongside the CSRF token (design §13.3). */
async function renameNode(
  page: Parameters<typeof loginAs>[0],
  nodeId: string,
  displayName: string,
): Promise<void> {
  const csrf = await page.evaluate(async () => {
    const response = await fetch('/api/public/v1/session')
    const body = (await response.json()) as { csrfToken: string }
    return body.csrfToken
  })
  const result = await page.evaluate(
    async ({ nodeId, displayName, csrf }) => {
      const response = await fetch(`/api/admin/v1/nodes/${nodeId}/metadata`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrf },
        // The Server DTO is camelCase (NodeMetadataRequest): displayName.
        body: JSON.stringify({ displayName }),
      })
      return { status: response.status, body: await response.json() }
    },
    { nodeId, displayName, csrf },
  )
  expect(result.status, JSON.stringify(result.body)).toBe(200)
}

async function openOverview(page: Parameters<typeof loginAs>[0]) {
  await loginAs(page)
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
}

// This file owns the shared Node E display-name mutation; serial mode keeps
// its tests from overlapping each other on the same Server state.
test.describe.configure({ mode: 'serial' })

test.describe('Owner Overview (PAGE-ADMIN-OVERVIEW)', () => {
  test('shows the Server-owned attention queue and Node Health Summary', async ({ page }) => {
    await openOverview(page)

    // Server-provided attention includes Node B's identity mismatch.
    const attentionPanel = page.getByRole('heading', { level: 2, name: 'Attention queue' }).locator('xpath=ancestor::article[1]')
    await expect(attentionPanel).toContainText('identity mismatches', { timeout: 15_000 })

    // Node Health Summary comes from the Server (health + reason), with the
    // fixed freshness vocabulary and last-good values.
    const nodeBRow = page.getByRole('row', { name: /Node B \(private\)/ })
    await expect(nodeBRow).toContainText('unhealthy')
    await expect(nodeBRow).toContainText('Stale')
    // Last-good head remains visible next to the Error state.
    await expect(nodeBRow).toContainText('12842018')

    // The Agent dimension stays independent of Node state.
    await expect(page.getByRole('heading', { level: 2, name: 'Agent inventory' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Attention queue' })).toBeVisible()
    // Legacy Geo and per-Node publication content is gone from Overview.
    await expect(page.getByRole('heading', { level: 2, name: 'Geo database' })).toHaveCount(0)
    await expect(page.getByRole('heading', { level: 2, name: 'Operations' })).toHaveCount(0)
    await expect(page.getByText(/Nodes are visible on Home/)).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })

  test('groups additional Attention items with an accessible disclosure at every fixed viewport', async ({ page }) => {
    await openOverview(page)
    const additional = page.getByRole('button', { name: /Show \d+ additional issues?/ })
    // Multiple subjects may each have grouped findings; verify the first
    // disclosure rather than assuming the fixture has only one group.
    await expect(additional.first()).toBeVisible()
    expect(await additional.count()).toBeGreaterThan(0)
    const firstAdditional = additional.first()
    const detailsId = await firstAdditional.getAttribute('aria-controls')
    expect(detailsId).toBeTruthy()
    await firstAdditional.click()
    await expect(page.locator(`[id="${detailsId}"]`)).toBeVisible()
    await expect(page.locator(`[aria-controls="${detailsId}"]`)).toHaveAttribute('aria-expanded', 'true')
    await expectVisibleInteractiveTargets(page)
    await expectNoHorizontalOverflow(page)
  })

  test('honors reduced motion while retaining the complete triage stack', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' })
    await openOverview(page)
    await expect(page.getByRole('heading', { level: 2, name: 'Attention queue' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Node Health Summary' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Agent inventory' })).toBeVisible()
    const transitions = await page.locator('[data-slot="admin-overview"] *').evaluateAll((elements) =>
      elements.map((element) => getComputedStyle(element).transitionDuration),
    )
    expect(transitions.every((duration) => duration === '0s')).toBe(true)
    await expectNoHorizontalOverflow(page)
  })

  test('node detail expansion survives an SSE-triggered REST refetch', async ({ page }, testInfo) => {
    // The mutation mutates shared Server state, so only one project runs it
    // and restores the seeded display name afterwards.
    test.skip(testInfo.project.name !== 'desktop-1280', 'metadata mutation runs once')
    await openOverview(page)

    // Expand Node B (private)'s component details.
    await page.getByRole('button', { name: 'Node B (private)' }).click()
    await expect(page.locator('[data-slot="detail-row"]')).toBeVisible()
    await expect(page.getByText('platon/1.5.1 · 0 namespaces')).toBeVisible()

    try {
      // Rename Node E through the real Admin API: the Server publishes an
      // Admin `node` invalidation, and the shell refetches the
      // authoritative REST resources without a reload.
      await renameNode(page, NODE_E, 'Node E (refetched)')
      await expect(
        page.getByRole('row', { name: /Node E \(refetched\)/ }),
        { timeout: 15_000 },
      ).toBeVisible()

      // Expansion and URL state survive the authorized refetch.
      await expect(page.locator('[data-slot="detail-row"]')).toBeVisible()
      await expect(page).toHaveURL(/\/admin$/)

      // Escape collapses the detail row; focus stays on the toggle.
      await page.getByRole('button', { name: 'Node B (private)' }).focus()
      await page.keyboard.press('Escape')
      await expect(page.locator('[data-slot="detail-row"]')).toHaveCount(0)
      await expect(page.getByRole('button', { name: 'Node B (private)' })).toBeFocused()
      await expectNoHorizontalOverflow(page)
    } finally {
      // Restore Node E's seeded display name so repeated runs and parallel
      // projects keep the seeded Server state.
      await renameNode(page, NODE_E, 'Node E (private)')
      await expect(
        page.getByRole('row', { name: /Node E \(private\)/ }),
        { timeout: 15_000 },
      ).toBeVisible()
    }
  })

  test('browser offline is reported as You are offline and recovers', async ({ page }) => {
    await openOverview(page)
    await expect(page.getByRole('status', { name: /^Current/ })).toBeVisible({ timeout: 15_000 })

    await page.context().setOffline(true)
    await expect(page.getByText('You are offline')).toBeVisible()

    await page.context().setOffline(false)
    await expect(page.getByRole('status', { name: /^Current/ })).toBeVisible({ timeout: 15_000 })
    await expectNoHorizontalOverflow(page)
  })

  test('overview remains usable at 200% zoom', async ({ page }) => {
    await openOverview(page)
    await expect(page.getByRole('heading', { level: 2, name: 'Attention queue' })).toBeVisible({ timeout: 15_000 })
    await expectVisibleInteractiveTargets(page)

    await setPageZoom(page, 2)
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Node Health Summary' })).toBeVisible()
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - window.innerWidth,
    )
    expect(overflow).toBeLessThanOrEqual(1)
  })

  test('mobile Admin drawer locks scroll, traps focus, and closes predictably', async ({ page }, testInfo) => {
    // The drawer is the tablet/mobile navigation; desktop has a sidebar.
    test.skip(['desktop-1280', 'desktop-1440'].includes(testInfo.project.name), 'desktop uses the sidebar')
    await openOverview(page)

    const menu = page.getByRole('button', { name: 'Menu' })
    await expect(menu).toBeVisible()
    await menu.click()
    const adminNav = page.getByRole('navigation', { name: 'Admin' })
    await expect(adminNav).toBeVisible()
    await expect.poll(() => page.evaluate(() => document.body.style.overflow)).toBe('hidden')
    // Opening moves focus inside the drawer.
    await expect(adminNav.getByRole('link', { name: 'Overview', exact: true })).toBeFocused()
    // Reverse Tab wraps from the first retained page group to the last.
    await page.keyboard.press('Shift+Tab')
    await expect(adminNav.getByRole('link', { name: 'Audit', exact: true })).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(adminNav.getByRole('link', { name: 'Overview', exact: true })).toBeFocused()
    // Tab stays inside the drawer and wraps at the last item (issue #92:
    // the MVP navigation holds exactly the seven retained page groups).
    const mvpNav = [
      'Overview',
      'Agents',
      'Nodes',
      'Networks',
      'Settings',
      'Sessions',
      'Audit',
    ]
    for (const item of [...mvpNav.slice(1), 'Overview']) {
      await page.keyboard.press('Tab')
      await expect(adminNav.getByRole('link', { name: item, exact: true })).toBeFocused()
    }
    for (const deferred of ['Validators', 'Alert Rules', 'Operations', 'Data', 'People']) {
      await expect(adminNav.getByRole('link', { name: deferred, exact: true })).toHaveCount(0)
    }
    // Escape closes the drawer, unlocks the body, and restores focus.
    await page.keyboard.press('Escape')
    await expect(menu).toBeFocused()
    await expect(adminNav).not.toBeInViewport()
    await expect.poll(() => page.evaluate(() => document.body.style.overflow)).toBe('')

    // Clicking the visible overlay outside the drawer follows the same close path.
    await menu.click()
    await expect(adminNav).toBeVisible()
    const viewport = page.viewportSize()
    if (!viewport) throw new Error('fixed Playwright viewport is required')
    await page.mouse.click(viewport.width - 4, Math.floor(viewport.height / 2))
    await expect(adminNav).not.toBeInViewport()
    await expect(menu).toBeFocused()
    await expect.poll(() => page.evaluate(() => document.body.style.overflow)).toBe('')
    await expectNoHorizontalOverflow(page)
  })

  test('desktop Admin sidebar stays visible with the current route identified', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'desktop-only sidebar contract')
    await openOverview(page)

    await expect(page.getByRole('button', { name: 'Menu' })).toBeHidden()
    const adminNav = page.getByRole('navigation', { name: 'Admin' })
    await expect(adminNav).toBeVisible()
    await expect(adminNav.getByRole('link', { name: 'Overview' })).toHaveAttribute('aria-current', 'page')
    await expect(page.getByRole('heading', { level: 2, name: 'Node Health Summary' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('read-only Overview panels stay static under hover while summary links keep their feedback', async ({ page }) => {
    await openOverview(page)

    // A read-only panel must not gain a hover glow, opacity change, or lift.
    const panel = page.locator('[data-slot="overview-panel"]').first()
    const panelStyle = () =>
      panel.evaluate((element) => {
        const style = getComputedStyle(element)
        return {
          boxShadow: style.boxShadow,
          transform: style.transform,
          translate: style.translate,
          backgroundColor: style.backgroundColor,
        }
      })
    const before = await panelStyle()
    await panel.hover()
    await page.waitForTimeout(250)
    const after = await panelStyle()
    expect(after.boxShadow).toBe('none')
    expect(after.transform).toBe('none')
    expect(after.translate).toBe('none')
    expect(after.backgroundColor).toBe(before.backgroundColor)

    // The Overview summary band is genuine navigation, so hover-capable devices
    // keep its emerald glow and 2px lift. Touch devices intentionally receive no
    // hover feedback at all (Tailwind gates hover: behind (hover: hover)).
    const hoverCapable = await page.evaluate(() => window.matchMedia('(hover: hover)').matches)
    if (!hoverCapable) return
    const link = page.getByRole('navigation', { name: 'Overview summaries' }).getByRole('link').first()
    const linkStyle = () =>
      link.evaluate((element) => {
        const style = getComputedStyle(element)
        return {
          boxShadow: style.boxShadow,
          transform: style.transform,
          translate: style.translate,
        }
      })
    expect((await linkStyle()).boxShadow).toBe('none')
    await link.hover()
    await expect.poll(async () => (await linkStyle()).boxShadow).not.toBe('none')
    // Tailwind v4 emits the standalone `translate` property; older output uses
    // `transform`. Accept either so the lift assertion is robust.
    await expect
      .poll(async () => {
        const style = await linkStyle()
        return style.translate !== 'none' || style.transform !== 'none'
      })
      .toBe(true)
  })

  test('read-only panels keep their reading order without overlap', async ({ page }) => {
    await openOverview(page)

    // The workbench is a single column, so every read-only region must appear
    // in document order and never overlap the one before it: removing the
    // decorative hover affordance must not collapse the reading hierarchy.
    const regions = [
      page.getByRole('heading', { level: 2, name: 'Attention queue' }).locator('xpath=ancestor::article[1]'),
      page.getByRole('navigation', { name: 'Overview summaries' }),
      page.getByRole('heading', { level: 2, name: 'Node Health Summary' }).locator('xpath=ancestor::article[1]'),
      page.getByRole('heading', { level: 2, name: 'Agent inventory' }).locator('xpath=ancestor::article[1]'),
    ]
    const boxes: { top: number; bottom: number }[] = []
    for (const region of regions) {
      await expect(region).toBeVisible()
      const box = (await region.boundingBox())!
      boxes.push({ top: box.y, bottom: box.y + box.height })
    }
    for (let index = 1; index < boxes.length; index += 1) {
      expect(boxes[index].top, `region ${index} overlaps the previous region`).toBeGreaterThanOrEqual(boxes[index - 1].bottom - 1)
    }
    await expect(page.locator('[data-slot="overview-panel"]')).toHaveCount(3)
    await expectNoHorizontalOverflow(page)
  })
})

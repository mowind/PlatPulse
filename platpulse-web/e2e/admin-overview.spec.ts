import { expect, test } from '@playwright/test'
import {
  expectNoHorizontalOverflow,
  loginAs,
  setPageZoom,
} from './helpers'

const NODE_C = '0195f2a1-0016-4016-8016-000000000016'

/** Published count from the Server-owned summary strip ("N of 5 Nodes…"). */
async function publishedCount(page: Parameters<typeof loginAs>[0]): Promise<number> {
  const text = await page.locator('.summary-strip').textContent()
  const match = text?.match(/(\d+) of 5 Nodes are visible/)
  if (!match) throw new Error(`unexpected summary strip: ${text}`)
  return Number(match[1])
}

async function openOverview(page: Parameters<typeof loginAs>[0]) {
  await loginAs(page)
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
}

// This file owns the shared Node C visibility mutation; serial mode keeps
// its tests from overlapping each other on the same Server state.
test.describe.configure({ mode: 'serial' })

test.describe('Owner Overview (PAGE-ADMIN-OVERVIEW)', () => {
  test('shows the Server-owned attention queue and Node Health Summary', async ({ page }) => {
    await openOverview(page)

    // Server-provided attention: Node B has an RPC collection failure.
    await expect(page.locator('.attention-list')).toContainText(
      'RPC collection failed',
      { timeout: 15_000 },
    )

    // Node Health Summary comes from the Server (health + reason), with the
    // fixed freshness vocabulary and last-good values.
    const nodeARow = page.getByRole('row', { name: /Node A/ })
    await expect(nodeARow).toContainText('healthy')
    await expect(nodeARow).toContainText('Current')
    await expect(nodeARow).toContainText('12842019')

    const nodeBRow = page.getByRole('row', { name: /Node B \(private\)/ })
    await expect(nodeBRow).toContainText('unhealthy')
    // Last-good head remains visible next to the Error state.
    await expect(nodeBRow).toContainText('12842018')

    // The Agent dimension stays independent of Node state.
    await expect(page.getByRole('heading', { level: 2, name: 'Agents' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Attention queue' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('node detail expansion survives an SSE-triggered REST refetch', async ({ page }, testInfo) => {
    // The mutation mutates shared Server state, so only one project runs it
    // and restores the seeded visibility afterwards.
    test.skip(testInfo.project.name !== 'desktop-1280', 'visibility mutation runs once')
    await openOverview(page)

    // Expand Node A's component details.
    await page.getByRole('button', { name: 'Node A' }).click()
    await expect(page.getByText('Collapse details')).toBeVisible()
    await expect(page.getByText('platon/1.5.1 · 3 namespaces')).toBeVisible()

    // The published count is baseline-relative: the PAGE-ADMIN-NODE-VISIBILITY
    // workflow test publishes/retracts its own scratch Node concurrently, so
    // the exact total is only asserted to move by one around this mutation.
    const strip = page.locator('.summary-strip')
    await expect(strip).toContainText(/of 5 Nodes are visible/, { timeout: 15_000 })
    const publishedBefore = await publishedCount(page)

    try {
      // Publish Node C through the operations form. The Server publishes an
      // Admin invalidation; the shell refetches the authoritative REST
      // resources without a reload.
      await page.getByLabel('Node ID').fill(NODE_C)
      await page.getByLabel('Visibility').selectOption('public')
      await page.getByRole('button', { name: 'Update visibility' }).click()
      await expect(page.getByText(`${NODE_C} is now public.`)).toBeVisible()

      await expect
        .poll(async () => publishedCount(page), { timeout: 15_000 })
        .toBe(publishedBefore + 1)

      // Expansion and URL state survive the authorized refetch.
      await expect(page.getByText('Collapse details')).toBeVisible()
      await expect(page).toHaveURL(/\/admin$/)

      // Escape collapses the detail row; focus stays on the toggle.
      await page.getByRole('button', { name: 'Node A' }).focus()
      await page.keyboard.press('Escape')
      await expect(page.getByText('Collapse details')).toHaveCount(0)
      await expect(page.getByRole('button', { name: 'Node A' })).toBeFocused()
      await expectNoHorizontalOverflow(page)
    } finally {
      // Retract Node C so repeated runs and parallel projects keep the
      // seeded Server state.
      await page.getByLabel('Node ID').fill(NODE_C)
      await page.getByLabel('Visibility').selectOption('private')
      await page.getByRole('button', { name: 'Update visibility' }).click()
      await expect(page.getByText(`${NODE_C} is now private.`)).toBeVisible({ timeout: 15_000 })
      await expect.poll(async () => publishedCount(page), { timeout: 15_000 }).toBe(publishedBefore)
    }
  })

  test('browser offline is reported as You are offline and recovers', async ({ page }) => {
    await openOverview(page)
    await expect(page.getByText('Connected')).toBeVisible({ timeout: 15_000 })

    await page.context().setOffline(true)
    await expect(page.getByText('You are offline')).toBeVisible()

    await page.context().setOffline(false)
    await expect(page.getByText('Connected')).toBeVisible({ timeout: 15_000 })
    await expectNoHorizontalOverflow(page)
  })

  test('overview remains usable at 200% zoom', async ({ page }) => {
    await openOverview(page)
    await expect(page.locator('.attention-list')).toBeVisible({ timeout: 15_000 })

    await setPageZoom(page, 2)
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Node health' })).toBeVisible()
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - window.innerWidth,
    )
    expect(overflow).toBeLessThanOrEqual(1)
  })

  test('mobile Admin drawer traps focus and closes on Escape', async ({ page }, testInfo) => {
    // The drawer is the tablet/mobile navigation; desktop has a sidebar.
    test.skip(testInfo.project.name === 'desktop-1280', 'desktop uses the sidebar')
    await openOverview(page)

    const menu = page.getByRole('button', { name: 'Menu' })
    await expect(menu).toBeVisible()
    await menu.click()
    const adminNav = page.getByRole('navigation', { name: 'Admin' })
    await expect(adminNav).toBeVisible()
    // Opening moves focus inside the drawer.
    await expect(page.getByRole('link', { name: 'Overview' })).toBeFocused()
    // Tab stays inside the drawer and wraps at the last item.
    await page.keyboard.press('Tab')
    await expect(page.getByRole('link', { name: 'Agents' })).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(page.getByRole('link', { name: 'Nodes' })).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(page.getByRole('link', { name: 'Networks' })).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(page.getByRole('link', { name: 'Overview' })).toBeFocused()
    // Escape closes the drawer and restores focus to the opener.
    await page.keyboard.press('Escape')
    await expect(menu).toBeFocused()
    await expect(adminNav).not.toBeInViewport()
    await expectNoHorizontalOverflow(page)
  })
})

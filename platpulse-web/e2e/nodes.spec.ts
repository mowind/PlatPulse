import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs, setPageZoom } from './helpers'

/** Tablet/mobile use the drawer; desktop has the persistent sidebar. */
async function openAdminNav(page: Parameters<typeof loginAs>[0], linkName: string) {
  await loginAs(page)
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible({ timeout: 15_000 })
  const menu = page.getByRole('button', { name: 'Menu' })
  if (await menu.isVisible()) await menu.click()
  const adminNav = page.getByRole('navigation', { name: 'Admin' })
  await adminNav.getByRole('link', { name: linkName, exact: true }).click()
}

async function openNodes(page: Parameters<typeof loginAs>[0]) {
  await openAdminNav(page, 'Nodes')
  await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible({ timeout: 15_000 })
}

// The mutation test toggles shared Server state (Node C rename),
// so all tests in this file run serially in one worker.
test.describe.configure({ mode: 'serial' })

test.describe('Owner Node inventory (PAGE-ADMIN-NODES)', () => {
  test('lists every Node as its own row with Server-owned dimensions', async ({ page }) => {
    await openNodes(page)

    // Node A: healthy and current on the row; identity, visibility and the
    // redacted endpoint are one Details step away (webui.md §8.7).
    const nodeARow = page.getByRole('row', { name: /Node A/ }).first()
    await expect(nodeARow).toContainText('healthy', { timeout: 15_000 })
    await expect(nodeARow).toContainText('Current')
    await expect(nodeARow).toContainText('12842019')
    await expect(nodeARow).not.toContainText('Matched')
    await expect(nodeARow).not.toContainText('ws://')
    await nodeARow.getByRole('button', { name: 'Details for Node A' }).click()
    const details = page.locator('[data-slot="detail-row"]').first()
    await expect(details).toContainText('Matched')
    await expect(details).toContainText('Public')
    // Endpoints are redacted destination summaries, never complete URLs.
    await expect(details).toContainText('ws://[REDACTED_IP]:****')
    await expect(details).not.toContainText('6790')

    // Node B: unhealthy with its own Error/Stale/Mismatched dimensions.
    const nodeBRow = page.getByRole('row', { name: /Node B \(private\)/ })
    await expect(nodeBRow).toContainText('unhealthy')
    await expect(nodeBRow).toContainText('Stale')
    await expect(nodeBRow).toContainText('Mismatched')
    await expect(nodeBRow).toContainText('chain_id')

    // Node D: retired is its own row with explicit lifecycle.
    const nodeDRow = page.getByRole('row', { name: /Node D \(retired\)/ })
    await expect(nodeDRow).toContainText('Retired')

    await expectNoHorizontalOverflow(page)
  })

  test('filters are URL state and preserve back/forward', async ({ page }) => {
    await openNodes(page)

    await page.getByRole('button', { name: 'More filters' }).click()
    await page.getByLabel('Visibility').selectOption('public')
    await expect(page).toHaveURL(/visibility=public/)
    await expect(page.getByRole('row', { name: /Node A/ })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByRole('row', { name: /Node B \(private\)/ })).toHaveCount(0)
    await expect(page.getByRole('row', { name: /Node D \(retired\)/ })).toHaveCount(0)

    await page.getByLabel('Lifecycle').selectOption('retired')
    await expect(page).toHaveURL(/lifecycle=retired/)
    // Public + retired cannot both hold; empty filter result is explicit.
    await expect(page.getByText('No Nodes match these filters.')).toBeVisible({ timeout: 15_000 })

    await page.getByLabel('Lifecycle').selectOption('all')
    await page.getByLabel('Visibility').selectOption('all')
    // Collapsed, More filters names nothing once every filter is All.
    await page.getByRole('button', { name: 'More filters' }).click()
    await expect(page.getByRole('list', { name: 'Active filters from More filters' })).toHaveCount(0)
    await expect(page.getByRole('row', { name: /Node D \(retired\)/ })).toBeVisible({ timeout: 15_000 })
    // Sorting is real button state, not decoration: the default Health
    // order keeps Node B (unhealthy) last; the Node-name order moves it
    // into alphabetical position.
    const dataRows = page.locator('tbody tr')
    await expect(dataRows.first()).toContainText('Node A')
    await expect(dataRows.last()).toContainText('Node B (private)')
    await page.getByLabel('Sort by').selectOption('name')
    await expect(dataRows.nth(1)).toContainText('Node B (private)')
    await page.getByLabel('Sort by').selectOption('health')
    await expect(dataRows.last()).toContainText('Node B (private)')
    await expectNoHorizontalOverflow(page)
  })

  test('Node detail shows Server-owned metadata, matched identity, and administrative diagnostics', async ({
    page,
  }) => {
    await openNodes(page)

    await page.getByRole('link', { name: 'Node A' }).click()
    await expect(page.getByRole('heading', { level: 1, name: /Node A/ })).toBeVisible({ timeout: 15_000 })

    // Overview: lifecycle guidance on its own administrative panel; the
    // History, Diagnostics and Manage sections are one tab away (webui.md §8.7).
    await expect(page.getByRole('tab', { name: 'Overview' })).toHaveAttribute('aria-selected', 'true')
    await expect(page.getByText(/never pushes Endpoint or lifecycle changes/)).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Node Inventory & lifecycle' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'RPC diagnostics' })).toHaveCount(0)

    await page.getByRole('tab', { name: /^Manage/ }).click()
    await expect(page.getByRole('heading', { level: 2, name: 'Server-owned metadata' })).toBeVisible()
    await expect(page.getByRole('link', { name: 'Open the Audit log' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Danger zone' })).toBeVisible()

    // Identity disposition is typed and visible.
    await page.getByRole('tab', { name: /^Diagnostics/ }).click()
    await expect(page).toHaveURL(/section=diagnostics/)
    await expect(page.getByRole('heading', { level: 2, name: 'Network identity' })).toBeVisible()
    await expect(page.getByText('Matched', { exact: true }).first()).toBeVisible()
    await expect(page.getByText(/210425 \/ 210425/)).toBeVisible()

    // Admin keeps its detail administrative and leaves Home's full
    // observation cards on the public Node detail route.
    await expect(page.getByRole('heading', { level: 2, name: 'RPC diagnostics' })).toBeVisible()
    await expect(page.getByText('platon/1.5.1', { exact: true })).toBeVisible()
    await expect(page.getByText('admin, net, platon')).toBeVisible()
    await expect(page.getByText('Redacted RPC Endpoint')).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Per-Node observations' })).toHaveCount(0)
    // Node Transfer, per-Node Visibility, and operation controls are absent.
    await expect(page.getByRole('link', { name: 'Transfer ownership' })).toHaveCount(0)
    await expect(page.getByRole('link', { name: 'Publish to Home' })).toHaveCount(0)
    await expect(page.getByRole('link', { name: 'Make private' })).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })

  test('Node detail keeps last-good values beside the RPC error and identity mismatch', async ({
    page,
  }) => {
    await openNodes(page)

    await page.getByRole('link', { name: 'Node B (private)' }).click()
    await expect(page.getByRole('heading', { level: 1, name: /Node B \(private\)/ })).toBeVisible({ timeout: 15_000 })

    // The health summary carries the Server-owned reason.
    await expect(page.getByLabel('Current Node and Agent dimensions')).toContainText('RPC collection failed')
    // Last-good sync values remain visible with the Error context.
    await expect(page.getByText(/last-good head 12842018/)).toBeVisible()
    // Both findings are summarized above every section.
    const findings = page.getByRole('list', { name: 'Current Node findings' })
    await expect(findings).toContainText('Network identity contradicts the Registry on chain_id')
    await expect(findings).toContainText('RPC collection failed')
    // The mismatch is a blocking diagnostic distinct from the RPC error.
    await findings.getByRole('button', { name: 'Open Diagnostics' }).first().click()
    await expect(page.getByText(/Contradicts the Registry: chain_id/)).toBeVisible()
    await expect(page.getByText(/New history is not merged/)).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('list and detail work with reduced motion and at 200% zoom', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' })
    await openNodes(page)
    await expect(page.getByRole('row', { name: /Node A/ })).toContainText('healthy', { timeout: 15_000 })
    // Row expansion keeps detail available without navigation.
    const details = page.getByRole('button', { name: 'Details for Node A' })
    await details.click()
    await expect(details).toHaveAttribute('aria-expanded', 'true')
    await expect(page.getByText('Lifecycle guidance')).toBeVisible()
    await details.press('Escape')
    await expect(details).toBeFocused()
    await expect(page.getByText('Lifecycle guidance')).toHaveCount(0)
    await setPageZoom(page, 2)
    await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible()
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - window.innerWidth,
    )
    expect(overflow).toBeLessThanOrEqual(1)
  })

  test('retired Node shows the reactivation guidance without remote control', async ({ page }) => {
    await openNodes(page)

    await page.getByRole('link', { name: 'Node D (retired)' }).click()
    await expect(page.getByRole('heading', { level: 1, name: /Node D \(retired\)/ })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByRole('heading', { level: 2, name: 'Node Inventory & lifecycle' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText(/Reactivation requires declaring the same Node ID/)).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText(/the Server never changes Node lifecycle remotely/)).toBeVisible({ timeout: 15_000 })
    // No lifecycle mutation is offered.
    await expect(page.getByRole('button', { name: /Retire|Reactivate/ })).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })

  test('metadata mutation refetches the authoritative display name', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'metadata mutation runs once')
    await openNodes(page)

    await page.getByRole('link', { name: 'Node C' }).click()
    await expect(page.getByRole('heading', { level: 1, name: /Node C/ })).toBeVisible({ timeout: 15_000 })
    await page.getByRole('tab', { name: /^Manage/ }).click()
    try {
      await page.getByRole('button', { name: 'Edit' }).click()
      await page.getByLabel('Display name').fill('Node C (renamed)')
      await page.getByRole('button', { name: 'Save' }).click()
      // The confirmation step is explicit before the mutation runs.
      await expect(page.getByText(/Rename this Node in the Server-owned metadata\?/)).toBeVisible()
      await page.getByRole('button', { name: 'Confirm rename' }).click()
      await expect(page.getByText('Display name is now "Node C (renamed)".')).toBeVisible({
        timeout: 15_000,
      })
      // The refetched detail shows the new Server-owned name.
      await expect(page.getByRole('heading', { level: 1, name: /Node C \(renamed\)/ })).toBeVisible({
        timeout: 15_000,
      })
    } finally {
      // Restore the seeded name.
      await page.getByRole('button', { name: 'Edit' }).click()
      await page.getByLabel('Display name').fill('Node C')
      await page.getByRole('button', { name: 'Save' }).click()
      await expect(page.getByText(/Rename this Node in the Server-owned metadata\?/)).toBeVisible()
      await page.getByRole('button', { name: 'Confirm rename' }).click()
      await expect(page.getByRole('heading', { level: 1, name: /Node C/ })).toBeVisible({
        timeout: 15_000,
      })
    }
  })
})

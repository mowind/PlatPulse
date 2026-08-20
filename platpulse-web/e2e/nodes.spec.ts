import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs, setPageZoom } from './helpers'

/** Tablet/mobile use the drawer; desktop has the persistent sidebar. */
async function openAdminNav(page: Parameters<typeof loginAs>[0], linkName: string) {
  await loginAs(page)
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible({ timeout: 15_000 })
  const menu = page.getByRole('button', { name: 'Menu' })
  if (await menu.isVisible()) await menu.click()
  await page.getByRole('link', { name: linkName }).click()
}

async function openNodes(page: Parameters<typeof loginAs>[0]) {
  await openAdminNav(page, 'Nodes')
  await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible({ timeout: 15_000 })
}

// The mutation tests toggle shared Server state (Node C visibility/name),
// so all tests in this file run serially in one worker.
test.describe.configure({ mode: 'serial' })

test.describe('Owner Node inventory (PAGE-ADMIN-NODES)', () => {
  test('lists every Node as its own row with Server-owned dimensions', async ({ page }) => {
    await openNodes(page)

    // Node A: healthy, current, matched identity, public.
    const nodeARow = page.getByRole('row', { name: /Node A/ })
    await expect(nodeARow).toContainText('healthy', { timeout: 15_000 })
    await expect(nodeARow).toContainText('Current')
    await expect(nodeARow).toContainText('Matched')
    await expect(nodeARow).toContainText('Public')
    await expect(nodeARow).toContainText('12842019')
    // Endpoints are redacted destination summaries, never complete URLs.
    await expect(nodeARow).toContainText('ws://[REDACTED_IP]:****')
    await expect(nodeARow).not.toContainText('6790')

    // Node B: unhealthy with its own Error/Stale/Mismatched dimensions.
    const nodeBRow = page.getByRole('row', { name: /Node B \(private\)/ })
    await expect(nodeBRow).toContainText('unhealthy')
    await expect(nodeBRow).toContainText('Stale')
    await expect(nodeBRow).toContainText('Mismatched')
    await expect(nodeBRow).toContainText('chain_id')
    await expect(nodeBRow).toContainText('Private')

    // Node D: retired is its own row with explicit lifecycle.
    const nodeDRow = page.getByRole('row', { name: /Node D \(retired\)/ })
    await expect(nodeDRow).toContainText('Retired')

    await expectNoHorizontalOverflow(page)
  })

  test('filters are URL state and preserve back/forward', async ({ page }) => {
    await openNodes(page)

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

  test('Node detail shows Server-owned metadata, matched identity, and per-Node observations', async ({
    page,
  }) => {
    await openNodes(page)

    await page.getByRole('link', { name: 'Node A' }).click()
    await expect(page.getByRole('heading', { level: 1, name: /Node A/ })).toBeVisible({ timeout: 15_000 })

    // Server-owned metadata: display name, visibility, lifecycle guidance.
    await expect(page.getByRole('heading', { level: 2, name: 'Server-owned metadata' })).toBeVisible()
    await expect(page.getByText(/never pushes Endpoint or lifecycle changes/)).toBeVisible()

    // Identity disposition is typed and visible.
    await expect(page.getByRole('heading', { level: 2, name: 'Network identity' })).toBeVisible()
    await expect(page.getByText('Matched', { exact: true })).toBeVisible()
    await expect(page.getByText(/210425 \/ 210425/)).toBeVisible()

    // Admin keeps its detail administrative and leaves Home's full
    // observation cards on the public Node detail route.
    await expect(page.getByRole('heading', { level: 2, name: 'RPC diagnostics' })).toBeVisible()
    await expect(page.getByText('platon/1.5.1')).toBeVisible()
    await expect(page.getByText('admin, net, platon')).toBeVisible()
    await expect(page.getByText('Redacted RPC Endpoint')).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Per-Node observations' })).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })

  test('Node detail keeps last-good values beside the RPC error and identity mismatch', async ({
    page,
  }) => {
    await openNodes(page)

    await page.getByRole('link', { name: 'Node B (private)' }).click()
    await expect(page.getByRole('heading', { level: 1, name: /Node B \(private\)/ })).toBeVisible({ timeout: 15_000 })

    // The health summary carries the Server-owned reason.
    await expect(page.getByText('RPC collection failed')).toBeVisible()
    // Last-good sync values remain visible with the Error context.
    await expect(page.getByText(/last-good head 12842018/)).toBeVisible()
    // The mismatch is a blocking diagnostic distinct from the RPC error.
    await expect(page.getByText(/Contradicts the Registry: chain_id/)).toBeVisible()
    await expect(page.getByText(/New history is not merged/)).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('list and detail work with reduced motion and at 200% zoom', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' })
    await openNodes(page)
    await expect(page.getByRole('row', { name: /Node A/ })).toContainText('healthy', { timeout: 15_000 })
    // Row expansion keeps detail available without navigation.
    await page.getByRole('button', { name: 'Expand Node A' }).click()
    await expect(page.getByText('Lifecycle guidance')).toBeVisible()
    await page.getByRole('button', { name: 'Collapse Node A' }).press('Escape')
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
    await expect(page.getByText(/Reactivation requires declaring the same Node ID/)).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText(/the Server never changes Node lifecycle remotely/)).toBeVisible({ timeout: 15_000 })
    // No lifecycle mutation is offered.
    await expect(page.getByRole('button', { name: /Retire|Reactivate/ })).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })

  test('visibility workflow publishes and retracts with authoritative refetch', async ({
    page,
  }, testInfo) => {
    // The mutation mutates shared Server state; one project runs it and
    // restores the seeded visibility afterwards.
    test.skip(testInfo.project.name !== 'desktop-1280', 'visibility mutation runs once')
    await openNodes(page)

    try {
      await page.getByRole('link', { name: 'Node E (private)' }).click()
      await expect(page.getByRole('heading', { level: 1, name: /Node E \(private\)/ })).toBeVisible({ timeout: 15_000 })
      await expect(page.getByText('Private', { exact: true }).first()).toBeVisible({ timeout: 15_000 })
      await page.getByRole('link', { name: 'Publish to Home' }).click()

      // Dedicated confirmation workflow with explicit impact copy.
      await expect(page.getByRole('heading', { level: 1, name: 'Node visibility' })).toBeVisible({ timeout: 15_000 })
      await expect(page.getByText(/Publishing it adds it to the Home projection/)).toBeVisible()
      await page.getByRole('button', { name: 'Publish to Home' }).click()
      await expect(page.getByText(/is now public\. The Home projection was updated\./)).toBeVisible({
        timeout: 15_000,
      })

      // Authoritative refetch: the detail now shows Public.
      await page.getByRole('link', { name: 'Back to Node detail' }).click()
      await expect(page.getByRole('heading', { level: 1, name: /Node E \(private\)/ })).toBeVisible({ timeout: 15_000 })
      await expect(page.getByText('Public', { exact: true }).first()).toBeVisible({
        timeout: 15_000,
      })
      // The Home projection is non-leaking: endpoints stay hidden.
      await page.getByRole('link', { name: 'Home', exact: true }).click()
      await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible({ timeout: 15_000 })
      await expect(page.getByText('Node E (private)', { exact: true })).toBeVisible({ timeout: 15_000 })
      await expect(page.getByText('ws://127.0.0.1')).toHaveCount(0)
    } finally {
      // Retract Node E so repeated runs and parallel projects keep the
      // seeded Server state.
      await page.getByRole('link', { name: 'Admin', exact: true }).click()
      await page.getByRole('link', { name: 'Nodes' }).click()
      await page.getByRole('link', { name: 'Node E (private)' }).click()
      await page.getByRole('link', { name: 'Make private' }).click()
      await page.getByRole('button', { name: 'Make private' }).click()
      await expect(page.getByText(/is now private\./)).toBeVisible({ timeout: 15_000 })
    }
  })

  test('metadata mutation refetches the authoritative display name', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'metadata mutation runs once')
    await openNodes(page)

    await page.getByRole('link', { name: 'Node C' }).click()
    await expect(page.getByRole('heading', { level: 1, name: /Node C/ })).toBeVisible({ timeout: 15_000 })
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

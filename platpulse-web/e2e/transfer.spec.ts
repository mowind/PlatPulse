import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

/**
 * SCN-NODE-TRANSFER-PENDING, SCN-NODE-TRANSFER-IDENTITY-MISMATCH, and
 * SCN-NODE-TRANSFER-COMPLETED (design §11, §12): the two-phase ownership
 * workflow. The source stays authoritative while pending; a Network
 * Identity Mismatch is a blocking diagnostic that never switches ownership
 * or merges history; completion is shown after authoritative refetch.
 */

async function openTransferPage(page: Parameters<typeof loginAs>[0], nodeName: string) {
  await loginAs(page)
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible({ timeout: 15_000 })
  const menu = page.getByRole('button', { name: 'Menu' })
  if (await menu.isVisible()) await menu.click()
  await page.getByRole('link', { name: 'Nodes' }).click()
  await page.getByRole('link', { name: nodeName }).click()
  // Substring name match (no regex): node names may contain parentheses.
  await expect(page.getByRole('heading', { level: 1, name: nodeName })).toBeVisible({ timeout: 15_000 })
  await page.getByRole('link', { name: 'Transfer ownership', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Transfer Node ownership' })).toBeVisible({ timeout: 15_000 })
}

test.describe('two-phase Node Transfer (PAGE-ADMIN-NODE-TRANSFER)', () => {
  test('SCN-NODE-TRANSFER-PENDING: create, visible expiry, cancel preserves source', async ({ page }, testInfo) => {
    // The mutation mutates shared Server state (Node F history); one
    // project runs it and restores the seeded state by cancelling.
    test.skip(testInfo.project.name !== 'desktop-1280', 'transfer mutation runs once')
    await openTransferPage(page, 'Node F (transfer)')

    // Recover from a previous interrupted run: cancel any leftover pending
    // transfer before starting the fresh mutation.
    if (await page.getByRole('button', { name: 'Cancel transfer' }).isVisible()) {
      await page.getByRole('button', { name: 'Cancel transfer' }).click()
      await page.getByRole('button', { name: 'Confirm cancel' }).click()
      await expect(page.getByText(/cancelled · Audit #/)).toBeVisible({ timeout: 15_000 })
      await page.reload()
      await expect(page.getByRole('heading', { level: 1, name: 'Transfer Node ownership' })).toBeVisible({ timeout: 15_000 })
    }

    // Create form: only registered Agents are offered (the source owner is
    // excluded from the candidates).
    const targetSelect = page.getByLabel('Target Agent')
    await expect(targetSelect).toBeVisible({ timeout: 15_000 })
    const options = targetSelect.locator('option')
    await expect(options).toHaveCount(2)
    await expect(options.nth(1)).toContainText('0195f2a1…')
    await targetSelect.selectOption('0195f2a1-0021-4021-8021-000000000021')
    await page.getByLabel('Operator reason').fill('e2e transfer of node F')
    await page.getByRole('button', { name: 'Review transfer' }).click()

    // Explicit high-risk confirmation before the mutation runs.
    await expect(page.getByText(/Create a pending transfer to 0195f2a1…/)).toBeVisible()
    await page.getByRole('button', { name: 'Confirm transfer' }).click()

    // Typed pending state with the Server-authoritative expiry, and the
    // request/Audit references in the success view.
    await expect(page.getByRole('heading', { level: 2, name: 'Transfer created' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText(/is pending\. The source Agent stays authoritative/)).toBeVisible()
    await expect(page.getByText(/Server-authoritative, never auto-extends/)).toBeVisible()
    await expect(page.getByText(/Audit event/)).toBeVisible()
    // The list refetched authoritative REST: the pending entry is there.
    await expect(page.getByRole('heading', { level: 2, name: 'Transfer history' })).toBeVisible()
    await expect(page.getByText('Pending', { exact: true }).first()).toBeVisible({ timeout: 15_000 })

    // Cancel with confirmation; ownership never changed.
    await page.getByRole('button', { name: 'Cancel transfer' }).click()
    await expect(page.getByText(/Cancel this transfer\?/)).toBeVisible()
    await page.getByRole('button', { name: 'Confirm cancel' }).click()
    await expect(page.getByText(/cancelled · Audit #/)).toBeVisible({ timeout: 15_000 })
    // Back on the detail page the Server-owned panel shows the outcome.
    await page.getByRole('link', { name: 'Back to Node detail' }).click()
    await expect(page.getByRole('heading', { level: 2, name: 'Node transfer' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText('Cancelled', { exact: true }).first()).toBeVisible({ timeout: 15_000 })
    await expectNoHorizontalOverflow(page)
  })

  test('SCN-NODE-TRANSFER-IDENTITY-MISMATCH: blocking diagnostic, no ownership switch', async ({ page }) => {
    await openTransferPage(page, 'Node F (transfer)')

    // The typed identity_mismatch outcome is a blocking diagnostic distinct
    // from RPC Error or Node Offline: ownership never switched and no
    // history merged.
    await expect(page.getByRole('heading', { level: 2, name: 'Transfer history' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText('Identity mismatch').first()).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText(/Blocking diagnostic/)).toBeVisible()
    await expect(page.getByText(/genesis_hash, address_hrp/)).toBeVisible()
    await expect(page.getByText(/no new block history was merged into the registered Network/)).toBeVisible()
    await expect(page.getByText(/ownership never switched/)).toBeVisible()

    // Every other typed outcome is visible in the timeline.
    await expect(page.getByText('Cancelled', { exact: true }).first()).toBeVisible()
    await expect(page.getByText('Expired', { exact: true }).first()).toBeVisible()
    await expect(page.getByText('Conflict', { exact: true }).first()).toBeVisible()

    await expectNoHorizontalOverflow(page)
  })

  test('SCN-NODE-TRANSFER-COMPLETED: atomic switch shown without merging issues', async ({ page }) => {
    await openTransferPage(page, 'Node G (transferred)')

    await expect(page.getByRole('heading', { level: 2, name: 'Transfer history' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText('Completed', { exact: true }).first()).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText(/Ownership switched atomically/)).toBeVisible()
    await expect(page.getByText(/the Node ID, Network, history, and visibility are unchanged/)).toBeVisible()
    // The detail panel shows the completed outcome with source → target.
    await page.getByRole('link', { name: 'Back to Node detail' }).click()
    await expect(page.getByRole('heading', { level: 2, name: 'Node transfer' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText(/0195f2a1… → 0195f2a1…/)).toBeVisible({ timeout: 15_000 })
    await expectNoHorizontalOverflow(page)
  })
})

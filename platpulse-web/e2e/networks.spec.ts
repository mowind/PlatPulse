import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

/** Tablet/mobile use the drawer; desktop has the persistent sidebar. */
async function openAdminNav(page: Parameters<typeof loginAs>[0], linkName: string) {
  await loginAs(page)
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible({ timeout: 15_000 })
  const menu = page.getByRole('button', { name: 'Menu' })
  if (await menu.isVisible()) await menu.click()
  await page.getByRole('link', { name: linkName }).click()
}

async function openNetworks(page: Parameters<typeof loginAs>[0]) {
  await openAdminNav(page, 'Networks')
  await expect(page.getByRole('heading', { level: 1, name: 'Networks' })).toBeVisible({ timeout: 15_000 })
}

test.describe('Owner Network Registry (PAGE-ADMIN-NETWORKS)', () => {
  test('lists the complete validated identity tuple with counts and mismatch outcomes', async ({
    page,
  }) => {
    await openNetworks(page)

    const row = page.getByRole('row', { name: /PlatON E2E Network/ })
    await expect(row).toContainText('platon-e2e', { timeout: 15_000 })
    await expect(row).toContainText('210425')
    await expect(row).toContainText('lat')
    await expect(row).toContainText('4 active · 1 retired')
    // Node B observes a contradicting chain id; the mismatch is typed.
    await expect(row).toContainText('Mismatched')
    await expectNoHorizontalOverflow(page)
  })

  test('detail shows the expected tuple and per-Node identity dispositions', async ({ page }) => {
    await openNetworks(page)

    await page.getByRole('link', { name: 'PlatON E2E Network' }).click()
    await expect(page.getByRole('heading', { level: 1, name: /PlatON E2E Network/ })).toBeVisible({ timeout: 15_000 })

    await expect(page.getByRole('heading', { level: 2, name: 'Expected identity tuple' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText(/0x1{64}/)).toBeVisible()

    // Per-Node dispositions: A matched, B mismatched on chain_id.
    await expect(page.getByRole('heading', { level: 2, name: 'Nodes on this Network' })).toBeVisible()
    await expect(page.getByText(/1 Node observes an identity that contradicts/)).toBeVisible()
    const nodeARow = page.getByRole('row', { name: /Node A/ })
    await expect(nodeARow).toContainText('Matched')
    const nodeBRow = page.getByRole('row', { name: /Node B \(private\)/ })
    await expect(nodeBRow).toContainText('Mismatched')
    await expect(nodeBRow).toContainText('chain_id')
    await expectNoHorizontalOverflow(page)
  })

  test('registers a Network only through the explicit full-tuple workflow', async ({ page }) => {
    await openNetworks(page)

    const uniqueKey = `platon-e2e-${Date.now()}`
    await page.getByRole('button', { name: 'Register a Network' }).click()
    await page.getByLabel('Network key').fill(uniqueKey)
    await page.getByLabel('Display name').fill('PlatON E2E Scratch')
    await page.getByLabel('Genesis hash').fill('0x' + '2'.repeat(64))
    await page.getByLabel('Chain ID').fill('210427')
    await page.getByLabel('P2P network ID').fill('3')
    await page.getByLabel('Address HRP').fill('lat')
    await page.getByRole('button', { name: 'Register Network' }).click()

    // Success refetches the authoritative Registry list.
    await expect(page.getByText('Registered PlatON E2E Scratch.')).toBeVisible({ timeout: 15_000 })
    await expect(page.getByRole('row', { name: /PlatON E2E Scratch/ }).first()).toBeVisible({
      timeout: 15_000,
    })
    await expectNoHorizontalOverflow(page)
  })

  test('rejects an invalid tuple with a field-level error and preserves the draft', async ({
    page,
  }) => {
    await openNetworks(page)

    await page.getByRole('button', { name: 'Register a Network' }).click()
    await page.getByLabel('Network key').fill('bad key!')
    await page.getByLabel('Display name').fill('Draft Network')
    await page.getByLabel('Genesis hash').fill('not-a-hash')
    await page.getByLabel('Chain ID').fill('1')
    await page.getByLabel('P2P network ID').fill('1')
    await page.getByLabel('Address HRP').fill('lat')
    await page.getByRole('button', { name: 'Register Network' }).click()

    await expect(page.getByText('the Network identity tuple is invalid')).toBeVisible({
      timeout: 15_000,
    })
    // The draft survives the failed mutation.
    await expect(page.getByLabel('Display name')).toHaveValue('Draft Network')
  })

  test('edits the Registry tuple with an audited confirmation and refetch', async ({
    page,
  }, testInfo) => {
    // The mutation mutates shared Server state; one project runs it and
    // restores the seeded tuple afterwards.
    test.skip(testInfo.project.name !== 'desktop-1280', 'tuple edit runs once')
    await openNetworks(page)

    await page.getByRole('link', { name: 'PlatON E2E Network' }).click()
    await expect(page.getByRole('heading', { level: 1, name: /PlatON E2E Network/ })).toBeVisible({ timeout: 15_000 })
    try {
      await page.getByRole('button', { name: 'Edit tuple' }).click()
      await page.getByLabel('Display name').fill('PlatON E2E Network (edited)')
      await page.getByRole('button', { name: 'Save tuple' }).click()
      // The confirmation step is explicit before the identity tuple mutation.
      await expect(page.getByText(/Update the expected identity tuple\?/)).toBeVisible()
      await page.getByRole('button', { name: 'Confirm tuple update' }).click()
      await expect(page.getByText('Updated PlatON E2E Network (edited).')).toBeVisible({
        timeout: 15_000,
      })
      // The refetched detail shows the new Server-owned name.
      await expect(
        page.getByRole('heading', { level: 1, name: 'PlatON E2E Network (edited)' }),
      ).toBeVisible({ timeout: 15_000 })
    } finally {
      // Restore the seeded display name.
      await page.getByRole('button', { name: 'Edit tuple' }).click()
      await page.getByLabel('Display name').fill('PlatON E2E Network')
      await page.getByRole('button', { name: 'Save tuple' }).click()
      await expect(page.getByText(/Update the expected identity tuple\?/)).toBeVisible()
      await page.getByRole('button', { name: 'Confirm tuple update' }).click()
      await expect(page.getByText('Updated PlatON E2E Network.')).toBeVisible({
        timeout: 15_000,
      })
    }
  })
})

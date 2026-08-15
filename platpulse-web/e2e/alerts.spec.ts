import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

async function openAlerts(page: Parameters<typeof loginAs>[0], section: string) {
  await loginAs(page)
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible({ timeout: 15_000 })
  const menu = page.getByRole('button', { name: 'Menu' })
  const usesDrawer = await menu.isVisible()
  if (usesDrawer) await menu.click()
  await page.getByRole('link', { name: section }).click()
  // Wait for the mobile drawer to finish closing so overflow measurements
  // never catch the transition mid-flight (desktop keeps the sidebar).
  if (usesDrawer) {
    await expect(page.getByRole('navigation', { name: 'Admin' })).not.toBeInViewport()
  }
}

test.describe('Owner Alert surfaces (PAGE-ADMIN-ALERT-RULES/INCIDENTS)', () => {
  test('lists the seeded typed Rule catalog with evaluation state', async ({ page }) => {
    await openAlerts(page, 'Alert Rules')
    await expect(page.getByRole('heading', { level: 1, name: 'Alert Rules' })).toBeVisible({ timeout: 15_000 })

    // The catalog is seeded by the Server; rules are typed, not free-form.
    const row = page.getByRole('row', { name: /node\.rpc_unreachable/ })
    await expect(row).toBeVisible({ timeout: 15_000 })
    await expect(row).toContainText('node')
    await expect(row).toContainText('Warning')
    await expect(page.getByRole('row', { name: /agent\.offline/ })).toBeVisible()

    // Rule detail carries versions, overrides, and per-subject evaluation.
    await row.getByRole('link', { name: 'node.rpc_unreachable' }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'node.rpc_unreachable' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByRole('heading', { level: 2, name: 'Evaluation state' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Versions' })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Network and Node overrides' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('edits a Rule through the typed schema form and previews without writing', async ({ page }) => {
    await openAlerts(page, 'Alert Rules')
    await page
      .getByRole('row', { name: /node\.rpc_unreachable/ })
      .getByRole('link', { name: 'node.rpc_unreachable' })
      .click()
    await expect(page.getByRole('heading', { level: 1, name: 'node.rpc_unreachable' })).toBeVisible({ timeout: 15_000 })
    await page.getByRole('link', { name: 'Edit Rule' }).click()

    await expect(page.getByRole('heading', { level: 1, name: 'Edit node.rpc_unreachable' })).toBeVisible({ timeout: 15_000 })
    // The editor is generated from the Server schema; there is no script input.
    await expect(page.getByLabel(/Sustained firing/)).toHaveValue('60')
    await expect(page.getByLabel(/Sustained recovery/)).toHaveValue('120')

    // Preview evaluates current facts without creating Incidents.
    await page.getByRole('button', { name: 'Preview current facts' }).click()
    await expect(page.getByText(/evaluated — nothing was written/)).toBeVisible({ timeout: 15_000 })

    // Cancel the edit: no version is written (detail still shows version 1).
    await page.getByRole('link', { name: '← node.rpc_unreachable' }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'node.rpc_unreachable' })).toBeVisible({ timeout: 15_000 })
    const detail = page.locator('dl.detail-list')
    await expect(detail).toContainText('Version', { timeout: 15_000 })
    await expect(detail).toContainText('1')
    await expectNoHorizontalOverflow(page)
  })

  test('Incidents are durable read-only history with typed filters', async ({ page }) => {
    await openAlerts(page, 'Incidents')
    await expect(page.getByRole('heading', { level: 1, name: 'Incidents' })).toBeVisible({ timeout: 15_000 })
    // The state filter is Server-backed; resolved incidents cannot exist in
    // the seeded environment, so the empty state is stable.
    await page.getByLabel('State').selectOption({ label: 'Resolved' })
    await expect(page.getByText(/No resolved Incidents/)).toBeVisible({ timeout: 15_000 })

    // Open-state incidents may accumulate while the Server evaluates the
    // seeded identity mismatch, so the 'All' state must render either rows
    // or the explicit empty state; the filter bar itself is always present.
    await page.getByLabel('State').selectOption({ label: 'All' })
    await expect(
      page.getByRole('table').or(page.getByText(/No Incidents/)),
    ).toBeVisible({ timeout: 15_000 })
    await expectNoHorizontalOverflow(page)
  })

  test('creates and cancels a Silence with confirmation', async ({ page }) => {
    await openAlerts(page, 'Silences')
    await expect(page.getByRole('heading', { level: 1, name: 'Silences' })).toBeVisible({ timeout: 15_000 })

    const now = new Date()
    const starts = new Date(now.getTime() - 60_000).toISOString().replace(/\.\d{3}Z$/, 'Z')
    const ends = new Date(now.getTime() + 3_600_000).toISOString().replace(/\.\d{3}Z$/, 'Z')

    await page.getByRole('button', { name: 'Create a Silence' }).click()
    const reason = `e2e quiet window ${Date.now()}`
    await page.getByLabel('Applies to').selectOption('node')
    await page.getByLabel('Matcher value').fill('0195f2a1-0014-4014-8014-000000000014')
    await page.getByLabel('Reason').fill(reason)
    await page.getByLabel('Starts at').fill(starts)
    await page.getByLabel('Ends at (required)').fill(ends)
    await page.getByRole('button', { name: 'Create Silence' }).click()

    const row = page.getByRole('row', { name: new RegExp(reason) })
    await expect(row).toBeVisible({ timeout: 15_000 })
    await expect(row).toContainText('node · 0195f2a1-0014-4014-8014-000000000014')
    await expect(row).toContainText('Active')

    // Cancellation requires confirmation; the active list refetches and
    // drops the Silence, which then appears under the Cancelled filter.
    await row.getByRole('button', { name: 'Cancel Silence' }).click()
    await row.getByRole('button', { name: 'Confirm cancellation' }).click()
    await expect(row).toBeHidden({ timeout: 15_000 })
    await page.getByLabel('Status').selectOption({ label: 'Cancelled' })
    const cancelledRow = page.getByRole('row', { name: new RegExp(reason) })
    await expect(cancelledRow).toBeVisible({ timeout: 15_000 })
    await expect(cancelledRow).toContainText('Cancelled')
    await expectNoHorizontalOverflow(page)
  })

  test('schedules Maintenance with a typed expected-condition allowlist', async ({ page }) => {
    await openAlerts(page, 'Maintenance')
    await expect(page.getByRole('heading', { level: 1, name: 'Maintenance Windows' })).toBeVisible({ timeout: 15_000 })

    const now = new Date()
    const starts = new Date(now.getTime() - 60_000).toISOString().replace(/\.\d{3}Z$/, 'Z')
    const ends = new Date(now.getTime() + 3_600_000).toISOString().replace(/\.\d{3}Z$/, 'Z')

    await page.getByRole('button', { name: 'Schedule Maintenance' }).click()
    const reason = `e2e planned reboot ${Date.now()}`
    await page.getByLabel('Scope value').fill('0195f2a1-0014-4014-8014-000000000014')
    await page.getByLabel('node.rpc_unreachable').check()
    await page.getByLabel('Reason').fill(reason)
    await page.getByLabel('Starts at').fill(starts)
    await page.getByLabel('Ends at (required)').fill(ends)
    await page.getByRole('button', { name: 'Schedule Window' }).click()

    const row = page.getByRole('row', { name: new RegExp(reason) })
    await expect(row).toBeVisible({ timeout: 15_000 })
    await expect(row).toContainText('node · 0195f2a1-0014-4014-8014-000000000014')
    await expect(row).toContainText('node.rpc_unreachable')
    await expect(row).toContainText('Active')
    await expectNoHorizontalOverflow(page)
  })
})

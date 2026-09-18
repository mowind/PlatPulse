import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

/**
 * Issue #159: the Home Dashboard shows the current-selection cumulative
 * Validator block and gross-reward totals, grouped by Network and
 * deduplicated by Validator. The fixtures in e2e/start-server.sh give two
 * deterministic Networks: PlatON E2E Network has one linked Validator
 * (100 blocks, 10 rewards) and five unlinked Active Nodes; Home Convergence
 * Network has three linked Validators (90+80+85 blocks, 12+9+11 rewards) with
 * two retained last-good contributors and three unlinked Active Nodes. The
 * summary is directly visible at every fixed viewport without hover or
 * expansion, and the values come from the Server, never a browser Node sum.
 */

const E2E_NETWORK = 'platon-e2e'
const CONVERGENCE_NETWORK = 'home-convergence'

test.describe('Home Validator totals (#159)', () => {
  test('shows per-Network deduplicated cumulative totals, coverage, and unlinked counts', async ({ page }) => {
    await loginAs(page)

    const summary = page.locator('[data-slot="validator-summary"]')
    await expect(summary).toBeVisible()
    await expect(summary.getByRole('heading', { name: 'Validator totals' })).toBeVisible()

    // PlatON E2E Network: one linked Validator across one linked Node.
    const e2e = page.locator(
      `[data-slot="validator-summary-network"][data-network-key="${E2E_NETWORK}"]`,
    )
    await expect(e2e).toBeVisible()
    await expect(
      e2e.locator('[data-slot="validator-summary-blocks"] [data-slot="metric-row-value"]'),
    ).toHaveText('100')
    await expect(
      e2e.locator('[data-slot="validator-summary-rewards"] [data-slot="metric-row-value"]'),
    ).toHaveText('10')
    await expect(
      e2e.locator('[data-slot="validator-summary-blocks"]').getByText('1/1 Validators with values'),
    ).toBeVisible()
    await expect(
      e2e.locator('[data-slot="validator-summary-rewards"]').getByText('1/1 Validators with values'),
    ).toBeVisible()
    await expect(e2e.locator('[data-slot="validator-summary-unlinked"]')).toHaveText('5 unlinked Nodes')

    // Home Convergence Network: three distinct Validators, each counted once,
    // with two retained last-good contributors and three unlinked Nodes.
    const convergence = page.locator(
      `[data-slot="validator-summary-network"][data-network-key="${CONVERGENCE_NETWORK}"]`,
    )
    await expect(convergence).toBeVisible()
    await expect(
      convergence.locator('[data-slot="validator-summary-blocks"] [data-slot="metric-row-value"]'),
    ).toHaveText('255')
    await expect(
      convergence.locator('[data-slot="validator-summary-rewards"] [data-slot="metric-row-value"]'),
    ).toHaveText('32')
    await expect(
      convergence
        .locator('[data-slot="validator-summary-blocks"]')
        .getByText('3/3 Validators with values · 2 stale'),
    ).toBeVisible()
    await expect(
      convergence
        .locator('[data-slot="validator-summary-rewards"]')
        .getByText('3/3 Validators with values · 2 stale'),
    ).toBeVisible()
    await expect(convergence.locator('[data-slot="validator-summary-unlinked"]')).toHaveText(
      '3 unlinked Nodes',
    )
    // The scope is described in place: grouped per Network, deduplicated by
    // Validator, so Networks never combine.
    await expect(summary.getByText(/grouped by Network · deduplicated by Validator/)).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('scopes the summary to the selected Network filter', async ({ page }) => {
    await loginAs(page)
    await page.getByRole('tab', { name: 'PlatON E2E Network' }).click()

    await expect(
      page.locator(`[data-slot="validator-summary-network"][data-network-key="${E2E_NETWORK}"]`),
    ).toBeVisible()
    await expect(
      page.locator(`[data-slot="validator-summary-network"][data-network-key="${CONVERGENCE_NETWORK}"]`),
    ).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })
})

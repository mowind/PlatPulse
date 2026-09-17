import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

/**
 * Issues #154, #155, and #156: the linked-Validator cumulative block count,
 * gross cumulative rewards, Server-computed production rate, and PlatScan's
 * own 24-hour rate are visible on both Node views — the Home Node card and
 * Node detail — with identity, link role, and freshness, and they never
 * overflow the fixed viewports. The rewards value is the source's gross
 * cumulative reward, not the operator's net earnings; the two rates are
 * distinguishable by source, and the cumulative rate is displayed to two
 * decimals on cards and at full Server precision in detail.
 */

const PUBLIC_NODE_NAME = 'Node A'
const PUBLIC_NODE_ID = '0195f2a1-0014-4014-8014-000000000014'
const LINKED_BLOCK_COUNT = '100'
const LINKED_REWARD = '10'
// 100 actual / 110 scheduled = 90.909091%; cards default to two decimals.
const LINKED_BLOCK_RATE_CARD = '90.91%'
const LINKED_BLOCK_RATE_DETAIL = '90.909091%'
const LINKED_GEN_BLOCKS_RATE_CARD = '75.50%'
const LINKED_GEN_BLOCKS_RATE_DETAIL = '75.5%'

test.describe('Linked Validator metrics (#154, #155, #156)', () => {
  test('shows cumulative blocks, rewards, and both rates on the Home card and Node detail', async ({ page }) => {
    await loginAs(page)

    // Home Node card: identity, role, and both cumulative Validator values.
    const card = page.getByRole('link', { name: new RegExp(PUBLIC_NODE_NAME) }).first()
    await expect(card).toBeVisible()
    await expect(card.getByText('Linked Validator')).toBeVisible()
    await expect(card.getByText('Cumulative blocks')).toBeVisible()
    await expect(card.getByText('Cumulative rewards', { exact: true })).toBeVisible()
    await expect(
      card.getByText('Cumulative blocks').locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_BLOCK_COUNT)
    await expect(
      card.getByText('Cumulative rewards', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_REWARD)
    await expect(card.getByText('Primary')).toBeVisible()
    // The two rates stay distinguishable by label and source, and the
    // cumulative rate is abbreviated to two decimals on the card.
    await expect(
      card.getByText('Production rate', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_BLOCK_RATE_CARD)
    await expect(
      card.getByText('PlatScan 24h rate', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_GEN_BLOCKS_RATE_CARD)
    await expect(card.getByText(/not an exact missed-block rate/).first()).toBeVisible()
    await expect(card.getByText(/PlatScan口径/).first()).toBeVisible()

    // Node detail: the same linked Validator area plus the exact-value caveat
    // and provenance.
    await page.goto('/nodes/' + PUBLIC_NODE_ID)
    const detail = page.getByRole('region', { name: 'Linked Validator' })
    await expect(detail).toBeVisible()
    await expect(detail.getByText('Cumulative blocks')).toBeVisible()
    await expect(detail.getByText('Cumulative rewards', { exact: true })).toBeVisible()
    await expect(
      detail.getByText('Cumulative rewards', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_REWARD)
    await expect(detail.getByText(/not operator net earnings/)).toBeVisible()
    await expect(detail.getByText('Primary')).toBeVisible()
    await expect(detail.getByText('Last success')).toBeVisible()
    // Detail preserves the full Server-computed and source precision.
    await expect(
      detail.getByText('Production rate', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_BLOCK_RATE_DETAIL)
    await expect(
      detail.getByText('PlatScan 24h rate', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_GEN_BLOCKS_RATE_DETAIL)

    await expectNoHorizontalOverflow(page)
  })
})

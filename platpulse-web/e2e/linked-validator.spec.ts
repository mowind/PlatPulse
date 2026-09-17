import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

/**
 * Issues #154, #155, #156, #157, and #158: the linked-Validator cumulative
 * block count, gross cumulative rewards, Network-scoped PlatScan rank,
 * Server-computed production rate, PlatScan's own 24-hour rate, and effective
 * delegation reward share are visible on both Node views — the Home Node card
 * and Node detail — with identity, link role, and freshness, and they never
 * overflow the fixed viewports. The rewards value is the source's gross
 * cumulative reward, not the operator's net earnings; the two rates are
 * distinguishable by source, and the cumulative rate is displayed to two
 * decimals on cards and at full Server precision in detail. Rank is adopted
 * from the Network cohort and never recomputed from the Home filters.
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
// Source rewardPer 20 is 20%; cards default to two decimals and detail
// preserves the source digits.
const LINKED_DELEGATION_SHARE_CARD = '20.00%'
const LINKED_DELEGATION_SHARE_DETAIL = '20%'
// The seeded ranking result is a complete Network cohort position.
const LINKED_RANK = '#2'

test.describe('Linked Validator metrics (#154, #155, #156, #157, #158)', () => {
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
    await expect(
      card.getByText('Network rank', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_RANK)
    await expect(card.getByText(/complete live-staking ALL cohort/).first()).toBeVisible()
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
    await expect(
      card.getByText('Delegation reward share', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_DELEGATION_SHARE_CARD)
    await expect(card.getByText(/not annualized yield, operator commission/).first()).toBeVisible()

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
    await expect(detail.getByText('Last success', { exact: true })).toBeVisible()
    await expect(
      detail.getByText('Network rank', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_RANK)
    // Detail preserves the full Server-computed and source precision.
    await expect(
      detail.getByText('Production rate', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_BLOCK_RATE_DETAIL)
    await expect(
      detail.getByText('PlatScan 24h rate', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_GEN_BLOCKS_RATE_DETAIL)
    await expect(
      detail.getByText('Delegation reward share', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText(LINKED_DELEGATION_SHARE_DETAIL)

    await expectNoHorizontalOverflow(page)
  })

  test('distinguishes an unranked Validator from a retained last-good rank', async ({ page }) => {
    await loginAs(page)

    // Node M: a complete live-staking cohort list that omits the Validator is
    // authoritative Unranked, never zero and never a collection failure.
    const mCard = page.getByRole('link', { name: /Node M/ }).first()
    await expect(mCard).toBeVisible()
    await expect(
      mCard.getByText('Network rank', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText('Unranked')

    // Node N: the ranking list failed, so the last-good rank is retained and
    // explicitly marked as retained rather than shown as Unranked or zero.
    const nCard = page.getByRole('link', { name: /Node N/ }).first()
    await expect(nCard).toBeVisible()
    await expect(
      nCard.getByText('Network rank', { exact: true }).locator('..').locator('[data-slot="metric-row-value"]'),
    ).toHaveText('#2')
    await expect(nCard.getByText(/last successful rank is retained/)).toBeVisible()

    await expectNoHorizontalOverflow(page)
  })
})

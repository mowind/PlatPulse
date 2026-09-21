import { expect, test, type Locator } from '@playwright/test'
import type { PublicNetwork, PublicNode } from '../src/api/generated'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

/**
 * Issues #154, #155, #156, #157, and #158: the linked-Validator cumulative
 * block count, gross cumulative rewards, Network-scoped PlatScan rank,
 * Server-computed production rate, PlatScan's own 24-hour rate, and effective
 * delegation reward share are visible on both Node views — the Home Node card
 * and Node detail — with identity, Current Validator Status, and freshness, and they never
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

/** Assert rendered geometry, not a CSS class or visibility alone: six cells
 * must form three rows of two, with no overflowing labels or exact values. */
async function expectTwoColumnMetrics(scope: Locator) {
  const metrics = scope.getByRole('group', { name: 'Linked Validator metrics' })
  const cells = metrics.locator(':scope > [data-slot="metric-row"], :scope > [data-slot="validator-metric"]')
  await expect(cells).toHaveCount(6)
  const boxes = await cells.evaluateAll(elements => elements.map(element => {
    const box = element.getBoundingClientRect()
    return { x: box.x, y: box.y, right: box.right, bottom: box.bottom, width: box.width }
  }))
  for (let index = 0; index < boxes.length; index += 2) {
    const left = boxes[index]
    const right = boxes[index + 1]
    expect(left.width).toBeGreaterThan(0)
    expect(right.width).toBeCloseTo(left.width, 0)
    expect(right.x).toBeGreaterThan(left.right)
    expect(right.y).toBeCloseTo(left.y, 0)
    expect(left.x).toBeCloseTo(boxes[0].x, 0)
    expect(right.x).toBeCloseTo(boxes[1].x, 0)
    if (index > 0) {
      expect(left.y).toBeGreaterThanOrEqual(Math.max(boxes[index - 2].bottom, boxes[index - 1].bottom))
    }
  }
  const overflowing = await metrics.locator('*').evaluateAll(elements => elements
    .filter(element => element.scrollWidth > element.clientWidth + 1)
    .map(element => element.textContent))
  expect(overflowing, 'metric labels and values must wrap within their cells').toEqual([])
}

test.describe('Linked Validator metrics (#154, #155, #156, #157, #158)', () => {
  test('shows cumulative blocks, rewards, and both rates on the Home card and Node detail', async ({ page }) => {
    await loginAs(page)

    // Home Node card: identity, Current Validator Status, and both cumulative Validator values.
    const card = page.getByRole('link', { name: PUBLIC_NODE_NAME, exact: true }).locator('xpath=ancestor::article[1]')
    await expect(card).toBeVisible()
    await expect(card.getByText('Linked Validator')).toBeVisible()
    await expect(card.getByText('Cumulative blocks')).toBeVisible()
    await expect(card.getByText('Cumulative rewards', { exact: true })).toBeVisible()
    await expect(
      card.getByText('Cumulative blocks').locator('..').locator('strong'),
    ).toHaveText(LINKED_BLOCK_COUNT)
    await expect(
      card.getByText('Cumulative rewards', { exact: true }).locator('..').locator('strong'),
    ).toHaveText(LINKED_REWARD)
    // The card header carries the consensus role badge too, so scope the
    // Current Validator Status assertion to the linked-Validator section.
    await expect(
      card.locator('[data-slot="linked-validator"]').getByText('Validator', { exact: true }),
    ).toBeVisible()
    await expect(
      card.getByText('Network rank', { exact: true }).locator('..').locator('strong'),
    ).toHaveText(LINKED_RANK)
    // The two rates stay distinguishable by label and source, and the
    // cumulative rate is abbreviated to two decimals on the card.
    await expect(
      card.getByText('Production rate', { exact: true }).locator('..').locator('strong'),
    ).toHaveText(LINKED_BLOCK_RATE_CARD)
    await expect(
      card.getByText('PlatScan 24h rate', { exact: true }).locator('..').locator('strong'),
    ).toHaveText(LINKED_GEN_BLOCKS_RATE_CARD)
    await expect(
      card.getByText('Delegation reward share', { exact: true }).locator('..').locator('strong'),
    ).toHaveText(LINKED_DELEGATION_SHARE_CARD)
    await expectTwoColumnMetrics(card)
    await expectNoHorizontalOverflow(page)

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
    // The detail region is already scoped to the linked Validator, so the
    // status badge is the only exact Validator text inside it.
    await expect(detail.getByText('Validator', { exact: true })).toBeVisible()
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
    await expectTwoColumnMetrics(detail)

    await expectNoHorizontalOverflow(page)
  })

  test('keeps long Validator identities and exact rewards accessible with bounded metric cells', async ({ page }) => {
    const identity = '0x' + 'ab'.repeat(64)
    const reward = '123456789012345678901234567890.123456789012'
    const exactReward = '123,456,789,012,345,678,901,234,567,890.123456789012'
    const withLongValidator = (node: PublicNode): PublicNode => {
      if (node.nodeId !== PUBLIC_NODE_ID || !node.validator) return node
      return { ...node, validator: { ...node.validator, displayName: null, validatorNodeId: identity, rewardAmount: reward } }
    }
    // Alter only the public presentation fixture; never mutate the shared
    // Server database or make long-value rendering depend on live PlatScan.
    await page.route('**/api/public/v1/networks', async route => {
      const response = await route.fetch()
      const networks: PublicNetwork[] = await response.json()
      await route.fulfill({ response, json: networks.map(network => ({ ...network, nodes: network.nodes.map(withLongValidator) })) })
    })
    await page.route('**/api/public/v1/nodes/' + PUBLIC_NODE_ID, async route => {
      const response = await route.fetch()
      const node: PublicNode = await response.json()
      await route.fulfill({ response, json: withLongValidator(node) })
    })
    await loginAs(page)
    const card = page.getByRole('link', { name: PUBLIC_NODE_NAME, exact: true }).locator('xpath=ancestor::article[1]')
    await expect(card.getByText(identity, { exact: true })).toHaveCount(0)
    await expect(card.getByLabel('Validator identifier: ' + identity.slice(0, 10) + '…' + identity.slice(-8))).toBeVisible()
    const metrics = card.getByRole('group', { name: 'Linked Validator metrics' })
    await expect(metrics.locator('[data-slot="validator-metric"]')).toHaveCount(6)
    const rewardCell = metrics.getByText('Cumulative rewards', { exact: true }).locator('..')
    await expect(rewardCell.locator('strong')).toHaveText('123456789012345678.9T')
    const rewardBox = (await rewardCell.boundingBox())!
    const metricsBox = (await metrics.boundingBox())!
    expect(rewardBox.width, 'exceptionally long compact rewards use the full group width').toBeCloseTo(metricsBox.width, 0)
    const overflowing = await metrics.locator('*').evaluateAll(elements => elements
      .filter(element => element.scrollWidth > element.clientWidth + 1)
      .map(element => element.textContent))
    expect(overflowing, 'all six values remain bounded and untruncated').toEqual([])
    await expectNoHorizontalOverflow(page)

    await card.getByRole('button', { name: 'Open Validator details' }).click()
    const dialog = page.getByRole('dialog', { name: 'Validator details' })
    await expect(dialog.getByRole('textbox', { name: 'Full Validator identifier' })).toHaveValue(identity)
    await expect(dialog.getByText(exactReward, { exact: true })).toBeVisible()
    await expectNoHorizontalOverflow(page)
    await page.keyboard.press('Escape')
    await expect(dialog).toHaveCount(0)
    await expect(card.getByRole('button', { name: 'Open Validator details' })).toBeFocused()

    await page.goto('/nodes/' + PUBLIC_NODE_ID)
    const detail = page.getByRole('region', { name: 'Linked Validator' })
    await expect(detail.getByText(identity, { exact: true })).toBeVisible()
    await expect(detail.getByText(exactReward, { exact: true })).toBeVisible()
    await expectTwoColumnMetrics(detail)
    await expectNoHorizontalOverflow(page)
  })

  test('distinguishes an unranked Validator from a retained last-good rank', async ({ page }) => {
    await loginAs(page)

    // Node M: a complete live-staking cohort list that omits the Validator is
    // authoritative Unranked, never zero and never a collection failure.
    const mCard = page.getByRole('link', { name: /Node M/ }).locator('xpath=ancestor::article[1]')
    await expect(mCard).toBeVisible()
    await expect(
      mCard.getByText('Network rank', { exact: true }).locator('..').locator('strong'),
    ).toHaveText('Unranked')

    // Node N: the ranking list failed, so the last-good rank is retained and
    // explicitly marked as retained rather than shown as Unranked or zero.
    const nCard = page.getByRole('link', { name: /Node N/ }).locator('xpath=ancestor::article[1]')
    await expect(nCard).toBeVisible()
    await expect(
      nCard.getByText('Network rank', { exact: true }).locator('..').locator('strong'),
    ).toHaveText('#2')
    await expect(nCard.getByText('Network ranking collection failed.', { exact: true })).toBeVisible()
    await expect(nCard.getByText('Network rank stale · last ranking retained.', { exact: true })).toBeVisible()
    await nCard.getByRole('button', { name: 'Open Validator details' }).click()
    const dialog = page.getByRole('dialog', { name: 'Validator details' })
    await expect(dialog.getByText(/last successful rank is retained/)).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(dialog).toHaveCount(0)

    await expectNoHorizontalOverflow(page)
  })
})

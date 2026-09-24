import { expect, test, type Locator } from '@playwright/test'
import type { PublicNetwork, PublicNode } from '../src/api/generated'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

/**
 * Issues #154, #155, #156, #157, #158 and #173: the linked-Validator cumulative
 * block count, gross cumulative rewards, Network-scoped PlatScan rank,
 * Server-computed production rate, PlatScan's own 24-hour rate, and effective
 * delegation reward share are visible on both Node views — the Home Node card
 * and Node detail.
 *
 * The Home card carries only the six metrics plus one short empty state: the
 * Linked Validator heading, Current Validator Status, Provider data line, link
 * identifier, copy control, Details row and long explanations moved to Node
 * detail, which the whole card already links to. The card's own width still
 * decides whether the four parameters pair into two columns or stack.
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

/** The strong value of the Linked Validator cell carrying the given label, for
 *  both the emphasized cell and the compact key-value row. */
const metricValue = (scope: Locator, label: string) =>
  scope.locator('[data-slot="metric-row"], [data-slot="validator-metric"]').filter({ hasText: label }).locator('strong')

/** The Home card's Linked Validator anatomy: two emphasized cells plus four
 *  compact rows, with the card width choosing one or two columns. */
async function expectCardMetrics(scope: Locator) {
  const metrics = scope.getByRole('group', { name: 'Linked Validator metrics' })
  await expect(metrics.locator(':scope > [data-slot="validator-metric"]')).toHaveCount(2)
  await expect(metrics.locator(':scope > [data-slot="metric-row"]')).toHaveCount(4)
  const rows = await metrics.locator(':scope > [data-slot="metric-row"]').evaluateAll(elements => elements.map(row => {
    const rect = row.getBoundingClientRect()
    return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom }
  }))
  const paired = Math.abs(rows[1].top - rows[0].top) <= 1
  for (let index = 1; index < rows.length; index++) {
    if (paired && index % 2 === 1) {
      expect(rows[index].top).toBeCloseTo(rows[index - 1].top, 1)
      expect(rows[index].left).toBeGreaterThan(rows[index - 1].right)
    } else {
      expect(rows[index].top).toBeGreaterThanOrEqual(rows[index - 1].bottom)
      expect(rows[index].left).toBeCloseTo(rows[0].left, 1)
    }
  }
  const overflowing = await metrics.locator('*').evaluateAll(elements => elements
    .filter(element => element.scrollWidth > element.clientWidth + 1)
    .map(element => element.textContent))
  expect(overflowing, 'metric labels and values must wrap within their cells').toEqual([])
}

/** Detail metrics use two columns below lg and three at desktop widths. */
async function expectDetailMetrics(scope: Locator) {
  const metrics = scope.getByRole('group', { name: 'Linked Validator metrics' })
  const cells = metrics.locator(':scope > [data-slot="metric-row"]')
  await expect(cells).toHaveCount(6)
  for (const value of await cells.locator('[data-slot="metric-row-value"]').all()) {
    await expect(value).toHaveCSS('justify-content', 'flex-start')
    expect(await value.evaluate(element => getComputedStyle(element, '::before').display)).toBe('none')
  }
  for (const cell of await cells.all()) {
    const gap = await cell.evaluate(element => {
      const label = element.querySelector('[data-slot="metric-row-label"]')!.getBoundingClientRect()
      const value = element.querySelector('[data-slot="metric-row-value"]')!.getBoundingClientRect()
      return value.top - label.bottom
    })
    expect(gap, 'each value sits directly beneath its own label, even beside a warning').toBeGreaterThanOrEqual(0)
    expect(gap).toBeLessThanOrEqual(8)
  }
  const columns = await metrics.evaluate(() => window.innerWidth >= 1024 ? 3 : 2)
  const metricsBox = (await metrics.boundingBox())!
  // gap-x-4 between columns, so one column is the row minus its gutters.
  const columnWidth = (metricsBox.width - 16 * (columns - 1)) / columns
  const boxes = await cells.evaluateAll(elements => elements.map(element => {
    const box = element.getBoundingClientRect()
    return { x: box.x, y: box.y, right: box.right, bottom: box.bottom, width: box.width }
  }))
  for (let index = 0; index < boxes.length; index++) {
    const box = boxes[index]
    expect(box.width).toBeGreaterThan(0)
    if (box.width > columnWidth * 1.5) {
      // The documented escape hatch for an exceptionally long exact value: it
      // takes the whole row rather than being folded into a narrow column.
      expect(box.width, 'a spanning value uses the full group width').toBeCloseTo(metricsBox.width, 0)
      expect(box.x, 'a spanning value starts its own row').toBeCloseTo(metricsBox.x, 0)
    } else {
      expect(box.width).toBeCloseTo(columnWidth, 0)
      expect(box.x, 'an ordinary cell sits on a column edge').toBeGreaterThanOrEqual(metricsBox.x - 1)
      expect(box.x).toBeLessThanOrEqual(metricsBox.x + metricsBox.width - box.width + 1)
    }
    expect(box.right).toBeLessThanOrEqual(metricsBox.x + metricsBox.width + 1)
    const previous = boxes[index - 1]
    if (previous && box.y < previous.bottom - 1) {
      expect(box.x, 'cells on one row never overlap').toBeGreaterThanOrEqual(previous.right - 1)
    } else if (previous) {
      expect(box.x, 'every row starts at the first column').toBeCloseTo(metricsBox.x, 0)
      expect(box.y).toBeGreaterThanOrEqual(previous.bottom - 1)
    }
  }
  const overflowing = await metrics.locator('*').evaluateAll(elements => elements
    .filter(element => element.scrollWidth > element.clientWidth + 1)
    .map(element => element.textContent))
  expect(overflowing, 'metric labels and exact values must wrap within their cells').toEqual([])
}

test.describe('Linked Validator metrics (#154, #155, #156, #157, #158)', () => {
  test('shows the six metrics on the Home card and the full identity on Node detail', async ({ page }) => {
    await loginAs(page)

    const card = page.getByRole('link', { name: PUBLIC_NODE_NAME, exact: true }).locator('xpath=ancestor::article[1]')
    await expect(card).toBeVisible()
    // The removed association area is gone from the Home card.
    await expect(card.getByText('Linked Validator')).toHaveCount(0)
    await expect(card.getByText('Data: Current')).toHaveCount(0)
    await expect(card.getByRole('button', { name: 'Copy full Validator identifier' })).toHaveCount(0)
    await expect(card.getByRole('button', { name: 'Open Validator details' })).toHaveCount(0)
    // The six metrics stay directly visible.
    await expect(card.getByText('Cumulative blocks')).toBeVisible()
    await expect(card.getByText('Cumulative rewards', { exact: true })).toBeVisible()
    await expect(metricValue(card, 'Cumulative blocks')).toHaveText(LINKED_BLOCK_COUNT)
    await expect(metricValue(card, 'Cumulative rewards')).toHaveText(LINKED_REWARD)
    await expect(metricValue(card, 'Network rank')).toHaveText(LINKED_RANK)
    await expect(metricValue(card, 'Production rate')).toHaveText(LINKED_BLOCK_RATE_CARD)
    await expect(metricValue(card, 'PlatScan 24h rate')).toHaveText(LINKED_GEN_BLOCKS_RATE_CARD)
    await expect(metricValue(card, 'Delegation reward share')).toHaveText(LINKED_DELEGATION_SHARE_CARD)
    await expectCardMetrics(card)
    await expectNoHorizontalOverflow(page)

    // Node detail keeps the identity, the full identifier with its copy
    // control, all six metrics at source precision and the provenance.
    await page.goto('/nodes/' + PUBLIC_NODE_ID)
    const detail = page.getByRole('region', { name: 'Linked Validator' })
    await expect(detail).toBeVisible()
    await expect(detail.getByText('E2E Validator')).toBeVisible()
    await expect(detail.getByRole('button', { name: 'Copy full Validator identifier' })).toBeVisible()
    await expect(detail.getByLabel(/Validator identifier: 0x/)).toHaveCount(0)
    await detail.getByRole('button', { name: 'Show full ID' }).click()
    await expect(detail.getByLabel(/Validator identifier: 0x/)).toBeVisible()
    // Diagnostics starts closed and is independent of the identity disclosure.
    await expect(detail.locator('details')).not.toHaveAttribute('open')
    await expect(detail.getByLabel('Public Validator states')).toBeHidden()
    await detail.locator('summary').click()
    // The public state vocabulary and provenance remain available on demand.
    await expect(detail.getByLabel('Public Validator states')).toBeVisible()
    await expect(detail.getByText('Data freshness', { exact: true })).toBeVisible()
    await expect(detail.getByText('Cumulative blocks')).toBeVisible()
    await expect(detail.getByText('Cumulative rewards', { exact: true })).toBeVisible()
    await expect(metricValue(detail, 'Cumulative rewards')).toHaveText(LINKED_REWARD)
    await expect(detail.getByText('Validator', { exact: true })).toBeVisible()
    await expect(detail.getByText('Last success', { exact: true })).toBeVisible()
    await expect(metricValue(detail, 'Network rank')).toHaveText(LINKED_RANK)
    await expect(metricValue(detail, 'Production rate')).toHaveText(LINKED_BLOCK_RATE_DETAIL)
    await expect(metricValue(detail, 'PlatScan 24h rate')).toHaveText(LINKED_GEN_BLOCKS_RATE_DETAIL)
    await expect(metricValue(detail, 'Delegation reward share')).toHaveText(LINKED_DELEGATION_SHARE_DETAIL)
    await expectDetailMetrics(detail)

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
    const metrics = card.getByRole('group', { name: 'Linked Validator metrics' })
    await expect(metrics.locator(':scope > [data-slot="validator-metric"]')).toHaveCount(2)
    await expect(metrics.locator(':scope > [data-slot="metric-row"]')).toHaveCount(4)
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

    await page.goto('/nodes/' + PUBLIC_NODE_ID)
    const detail = page.getByRole('region', { name: 'Linked Validator' })
    await detail.getByRole('button', { name: 'Show full ID' }).click()
    await expect(detail.getByText(identity, { exact: true })).toBeVisible()
    await expect(detail.getByText(exactReward, { exact: true })).toBeVisible()
    await expectDetailMetrics(detail)
    await expectNoHorizontalOverflow(page)
  })

  test('supports keyboard and touch disclosures and reveals the full ID on clipboard failure', async ({ page, isMobile }, testInfo) => {
    const identity = '0X' + 'AbCd'.repeat(32)
    await page.addInitScript(() => {
      Object.defineProperty(navigator, 'clipboard', { configurable: true, value: {
        writeText: () => Promise.reject(new Error('Clipboard permission denied')),
      } })
    })
    await page.route('**/api/public/v1/nodes/' + PUBLIC_NODE_ID, async route => {
      const response = await route.fetch()
      const node: PublicNode = await response.json()
      await route.fulfill({ response, json: { ...node, validatorIdentityReason: 'Identity discovery failed.', validator: { ...node.validator,
        validatorNodeId: identity, state: 'error', freshness: 'stale', currentValidatorStatus: 'unknown',
        rankState: 'error', blockRateState: 'not_applicable', blockRate: null, counterState: 'counter_reset',
      } } })
    })
    await loginAs(page)
    await page.goto('/nodes/' + PUBLIC_NODE_ID)
    const detail = page.getByRole('region', { name: 'Linked Validator' })
    const summary = detail.locator('summary')
    const touch = Boolean(testInfo.project.use.hasTouch || isMobile)
    await expect(detail.getByText(identity, { exact: true })).toHaveCount(0)
    const show = detail.getByRole('button', { name: 'Show full ID' })
    if (touch) await show.tap()
    else { await show.focus(); await page.keyboard.press('Enter') }
    await expect(detail.getByText(identity, { exact: true })).toBeVisible()
    await expect(detail.locator('details')).not.toHaveAttribute('open')
    const hide = detail.getByRole('button', { name: 'Hide full ID' })
    if (touch) await hide.tap()
    else { await hide.focus(); await page.keyboard.press('Space') }
    await expect(detail.getByText(identity, { exact: true })).toHaveCount(0)
    await detail.getByRole('button', { name: 'Copy full Validator identifier' }).click()
    await expect(detail.getByRole('status', { name: 'Identifier copy status' })).toHaveText(/Copy failed.*manually/)
    await expect(detail.getByText(identity, { exact: true })).toBeVisible()
    await expect(detail.getByText(identity, { exact: true })).toHaveCSS('user-select', 'text')
    for (const reason of [/source could not be read.*retained and stale/, /last established association/,
      /not a negative conclusion/, /last successful rank is retained/, /zero scheduled-block denominator/, /prior value was not treated as normal growth/]) {
      await expect(detail.getByText(reason)).toBeVisible()
    }
    await expect(detail.getByLabel('Public Validator states')).toBeHidden()
    if (touch) await summary.tap()
    else { await summary.focus(); await page.keyboard.press('Enter') }
    await expect(detail.getByLabel('Public Validator states')).toBeVisible()
    await expect(detail.getByText('Last success', { exact: true })).toBeVisible()
    if (touch) await summary.tap()
    else await page.keyboard.press('Space')
    await expect(detail.getByLabel('Public Validator states')).toBeHidden()
    await expect(detail.getByText(identity, { exact: true })).toBeVisible()
    await expectDetailMetrics(detail)
    await expectNoHorizontalOverflow(page)
  })

  test('distinguishes an unranked Validator from a retained last-good rank', async ({ page }) => {
    await loginAs(page)

    // Node M: a complete live-staking cohort list that omits the Validator is
    // authoritative Unranked, never zero and never a collection failure.
    const mCard = page.getByRole('link', { name: /Node M/ }).locator('xpath=ancestor::article[1]')
    await expect(mCard).toBeVisible()
    await expect(metricValue(mCard, 'Network rank')).toHaveText('Unranked')

    // Node N: the ranking list failed, so the last-good rank is retained on the
    // card while the full explanation stays on Node detail.
    const nCard = page.getByRole('link', { name: /Node N/ }).locator('xpath=ancestor::article[1]')
    await expect(nCard).toBeVisible()
    await expect(metricValue(nCard, 'Network rank')).toHaveText('#2')
    await expect(nCard.getByRole('status', { name: /Validator data: Failed/ })).toBeVisible()
    await nCard.getByRole('link', { name: /Node N/ }).click()
    const detail = page.getByRole('region', { name: 'Linked Validator' })
    await expect(detail.getByText(/last successful rank is retained/)).toBeVisible()

    await expectNoHorizontalOverflow(page)
  })

  test('folds an authoritative no-live-Validator card without repeating its long explanation', async ({ page }) => {
    const foldValidator = (node: PublicNode): PublicNode => {
      if (node.nodeId !== PUBLIC_NODE_ID || !node.validator) return node
      return { ...node, validator: { ...node.validator, state: 'empty', freshness: 'fresh',
        currentValidatorStatus: 'not_validator', currentValidatorStatusState: 'current', currentValidatorStatusQualifier: null,
        blockCount: null, rewardAmount: null, rank: null, rankState: 'unranked', rankFreshness: 'fresh',
        blockRate: null, blockRateState: 'unknown', expectedBlockCount: null, genBlocksRate: null, delegationRewardPercentage: null } }
    }
    await page.route('**/api/public/v1/networks', async route => {
      const response = await route.fetch()
      const networks: PublicNetwork[] = await response.json()
      await route.fulfill({ response, json: networks.map(network => ({ ...network, nodes: network.nodes.map(foldValidator) })) })
    })
    await loginAs(page)
    const card = page.getByRole('link', { name: PUBLIC_NODE_NAME, exact: true }).locator('xpath=ancestor::article[1]')
    const linked = card.locator('[data-slot="linked-validator"]')
    // One short, accurate empty state keeps the region's reserved height.
    const empty = linked.locator('[data-slot="linked-validator-empty"]')
    await expect(empty).toBeVisible()
    await expect(empty).toHaveText('No validator metrics')
    await expect(linked.getByRole('group', { name: 'Linked Validator metrics' })).toHaveCount(0)
    // The removed association area is not replaced by a Not a Validator line.
    await expect(linked.getByText('Not a Validator', { exact: true })).toHaveCount(0)
    await expect(card.getByText('Data: No live Validator')).toHaveCount(0)
    await expect(card.getByRole('button', { name: 'Open Validator details' })).toHaveCount(0)
    // Only the empty message is centered in the full reserved metric region.
    const regionBox = (await card.locator('[data-node-region="validator"]').boundingBox())!
    const emptyBox = (await empty.boundingBox())!
    expect(Math.abs(emptyBox.y + emptyBox.height / 2 - regionBox.y - regionBox.height / 2)).toBeLessThanOrEqual(1)
    await expect(empty).toHaveCSS('text-align', 'center')
    expect(emptyBox.x + emptyBox.width / 2).toBeCloseTo(regionBox.x + regionBox.width / 2, 0)
    await expectNoHorizontalOverflow(page)
  })
})

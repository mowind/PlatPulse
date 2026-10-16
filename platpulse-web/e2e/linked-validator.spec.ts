import { expect, test, type Locator } from '@playwright/test'
import type { PublicNetwork, PublicNode } from '../src/api/generated'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

/**
 * Issues #154, #155, #156, #157, #158, #173 and #238: the Network-scoped
 * PlatScan rank, the cumulative block count, the gross cumulative rewards, the
 * Server-computed production rate, PlatScan's own 24-hour rate and the effective
 * delegation reward share are visible on Node detail, which keeps every metric
 * at source precision.
 *
 * The Home card carries only the three key summaries a confirmed Validator
 * owns — rank, lifetime blocks and gross lifetime rewards — or one short
 * identity line when the Server's Current Validator Status confirms a
 * non-Validator or cannot confirm one. Identity, the identifier, the copy
 * control, the Details row and the long explanations live on Node detail, which
 * the whole card already links to.
 */

const PUBLIC_NODE_NAME = 'Node A'
const PUBLIC_NODE_ID = '0195f2a1-0014-4014-8014-000000000014'
const LINKED_BLOCK_COUNT = '100'
const LINKED_REWARD = '10'
// 100 actual / 110 scheduled = 90.909091%; Node detail keeps the source digits.
const LINKED_BLOCK_RATE_DETAIL = '90.909091%'
const LINKED_GEN_BLOCKS_RATE_DETAIL = '75.5%'
// Source rewardPer 20 is 20%; detail preserves the source digits.
const LINKED_DELEGATION_SHARE_DETAIL = '20%'
// The seeded ranking result is a complete Network cohort position.
const LINKED_RANK = '#2'

/** The strong value of the Linked Validator metric carrying the given label,
 *  for both the Home card's summary cell and Node detail's metric rows. */
const metricValue = (scope: Locator, label: string) =>
  scope.locator('[data-slot="metric-row"], [data-slot="validator-summary-cell"]').filter({ hasText: label }).locator('strong')

/** The Home card's Linked Validator summary (#238): the three key metrics in
 *  the two or three columns the card's own width holds, with the third cell
 *  keeping the whole row when only two fit. */
async function expectCardMetrics(scope: Locator, rewardsTakeFullRow = false) {
  const metrics = scope.getByRole('group', { name: 'Linked Validator metrics' })
  const cells = metrics.locator(':scope > [data-slot="validator-summary-cell"]')
  await expect(cells).toHaveCount(3)
  const layout = await metrics.evaluate(element => {
    const group = element.getBoundingClientRect()
    return {
      tracks: getComputedStyle(element).gridTemplateColumns.split(' '),
      group: { left: group.left, right: group.right },
      cells: [...element.querySelectorAll(':scope > [data-slot="validator-summary-cell"]')].map(cell => {
        const box = cell.getBoundingClientRect()
        return { left: box.left, right: box.right, top: box.top, bottom: box.bottom }
      }),
    }
  })
  expect(layout.tracks.length, 'the summary uses two or three columns').toBeGreaterThanOrEqual(2)
  expect(layout.tracks.length).toBeLessThanOrEqual(3)
  expect(layout.cells[1].left, 'the first two cells share one row').toBeGreaterThan(layout.cells[0].right)
  if (layout.tracks.length === 3 && !rewardsTakeFullRow) {
    expect(layout.cells[2].left).toBeGreaterThan(layout.cells[1].right)
    expect(layout.cells[2].top).toBeCloseTo(layout.cells[0].top, 1)
  } else {
    // The reward cell keeps the whole row when only two columns fit, or when
    // its exceptional source value cannot fit an ordinary summary cell.
    expect(layout.cells[2].top).toBeGreaterThan(layout.cells[0].bottom)
    expect(layout.cells[2].left).toBeCloseTo(layout.group.left, 0)
    expect(layout.cells[2].right).toBeCloseTo(layout.group.right, 0)
  }
  const overflowing = await metrics.locator('*').evaluateAll(elements => elements
    .filter(element => element.scrollWidth > element.clientWidth + 1)
    .map(element => element.textContent))
  expect(overflowing, 'metric labels and values must wrap within their cells').toEqual([])
}

/** The summary's label vocabulary: every cell keeps the full metric name as its
 *  accessible name and title, and all three paint the same one of their two
 *  label forms. Returns the names the cells actually paint. */
async function expectCardLabels(scope: Locator) {
  const metrics = scope.getByRole('group', { name: 'Linked Validator metrics' })
  const labels = await metrics.locator(':scope > [data-slot="validator-summary-cell"]').evaluateAll(cells => cells.map(cell => {
    const full = cell.querySelector('[data-full-label]')!
    const short = cell.querySelector('[data-short-label]')!
    return {
      metric: cell.getAttribute('data-metric'),
      full: full.textContent,
      short: short.textContent,
      accessible: short.getAttribute('aria-label'),
      title: short.getAttribute('title'),
      paintsFull: getComputedStyle(full).display !== 'none',
    }
  }))
  expect(labels.map(label => label.metric)).toEqual(['rank', 'blocks', 'rewards'])
  for (const label of labels) {
    expect(label.accessible).toBe(label.full)
    expect(label.title).toBe(label.full)
  }
  expect(new Set(labels.map(label => label.paintsFull)).size, 'the three cells paint one label vocabulary').toBe(1)
  return labels.map(label => label.paintsFull ? label.full! : label.short!)
}

/** Six independent metrics: two columns on phones and tablets, strict desktop 3×2.
 *  A value too long for one column spans the whole row, so geometry is checked per row. */
async function expectDetailMetrics(scope: Locator) {
  const metrics = scope.getByRole('group', { name: 'Validator performance metrics' })
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
  await expect(cells.locator('[data-slot="metric-row-label"]')).toHaveText([
    'Cumulative blocks', 'Cumulative rewards', 'Network rank',
    'Production rate', 'PlatScan 24h rate', 'Delegation reward share',
  ])
  await expect(metrics).toHaveCSS('column-gap', '16px')
  const metricsBox = (await metrics.boundingBox())!
  const columnGap = 16
  const columnWidth = (metricsBox.width - columnGap * (columns - 1)) / columns
  const boxes = await cells.evaluateAll(elements => elements.map(element => {
    const box = element.getBoundingClientRect()
    return { x: box.x, y: box.y, bottom: box.bottom, width: box.width }
  }))
  // Walk each rendered row: a metric fills its column, and one whose value
  // cannot fit spans the whole row; either way rows fill left to right in order.
  let row: typeof boxes = []
  const checkRow = () => {
    if (row.length === 0) return
    let expectedX = metricsBox.x
    for (const box of row) {
      const full = Math.abs(box.width - metricsBox.width) <= 1
      expect(box.x, 'metrics fill their row from the left in row-major order').toBeCloseTo(expectedX, 0)
      expect(box.width, 'a metric occupies one column unless a long value spans the row').toBeCloseTo(full ? metricsBox.width : columnWidth, 0)
      expectedX += full ? metricsBox.width : columnWidth + columnGap
    }
    row = []
  }
  for (const box of boxes) {
    if (row.length > 0 && Math.abs(box.y - row[0].y) > 1) checkRow()
    row.push(box)
  }
  checkRow()
  const overflowing = await metrics.locator('*').evaluateAll(elements => elements
    .filter(element => element.scrollWidth > element.clientWidth + 1)
    .map(element => element.textContent))
  expect(overflowing, 'metric labels and exact values must wrap within their cells').toEqual([])
}

test.describe('Linked Validator metrics (#154, #155, #156, #157, #158)', () => {
  test('shows the three summaries on the Home card and every metric on Node detail', async ({ page }) => {
    await loginAs(page)

    const card = page.getByRole('link', { name: PUBLIC_NODE_NAME, exact: true }).locator('xpath=ancestor::article[1]')
    await expect(card).toBeVisible()
    // The removed association area is gone from the Home card.
    await expect(card.getByText('Linked Validator')).toHaveCount(0)
    await expect(card.getByText('Data: Current')).toHaveCount(0)
    await expect(card.getByRole('button', { name: 'Copy full Validator identifier' })).toHaveCount(0)
    await expect(card.getByRole('button', { name: 'Open Validator details' })).toHaveCount(0)
    // The three key summaries stay directly visible, in one label vocabulary.
    await expectCardMetrics(card)
    expect(await expectCardLabels(card)).toEqual(['Rank', 'Blocks', 'Rewards'])
    await expect(metricValue(card, 'Cumulative blocks')).toHaveText(LINKED_BLOCK_COUNT)
    await expect(metricValue(card, 'Cumulative rewards')).toHaveText(LINKED_REWARD)
    await expect(metricValue(card, 'Network rank')).toHaveText(LINKED_RANK)
    // The three metrics #238 retired from the card stay on Node detail.
    await expect(card.getByText('Production rate')).toHaveCount(0)
    await expect(card.getByText('PlatScan 24h rate')).toHaveCount(0)
    await expect(card.getByText('Delegation reward share')).toHaveCount(0)
    await expectNoHorizontalOverflow(page)

    // Node detail keeps the identity, the full identifier with its copy
    // control, all six metrics at source precision and the provenance.
    await page.goto('/nodes/' + PUBLIC_NODE_ID)
    const detail = page.getByRole('region', { name: 'Validator performance' })
    await expect(detail).toBeVisible()
    await expect(detail.getByRole('button', { name: 'Copy full Validator identifier' })).toBeVisible()
    await expect(detail.getByLabel(/Validator identifier: 0x/)).toHaveCount(0)
    await detail.getByRole('button', { name: 'Show full ID' }).click()
    await expect(detail.getByLabel(/Validator identifier: 0x/)).toBeVisible()
    // Diagnostics starts closed and is independent of the identity disclosure.
    // #225 moved it out of the performance card to sit after the chart deck, so
    // the page's own disclosure is what these assertions address.
    const diagnostics = page.locator('details[data-slot="disclosure"]', { hasText: 'Validator diagnostics' })
    await expect(diagnostics).toHaveCount(1)
    await expect(diagnostics).not.toHaveAttribute('open')
    await expect(diagnostics.getByLabel('Public Validator states')).toBeHidden()
    await diagnostics.locator('summary').click()
    // The public state vocabulary and provenance remain available on demand.
    await expect(diagnostics.getByLabel('Public Validator states')).toBeVisible()
    await expect(diagnostics.getByText('E2E Validator', { exact: true })).toBeVisible()
    await expect(diagnostics.getByText('Data freshness', { exact: true })).toBeVisible()
    await expect(detail.getByText('Cumulative blocks')).toBeVisible()
    await expect(detail.getByText('Cumulative rewards', { exact: true })).toBeVisible()
    await expect(metricValue(detail, 'Cumulative rewards')).toHaveText(LINKED_REWARD)
    await expect(diagnostics.getByLabel('Public Validator states').getByText('Current staking status', { exact: true })).toBeVisible()
    await expect(diagnostics.getByText('Last success', { exact: true })).toBeVisible()
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
    await expectCardMetrics(card, true)
    const rewardCell = metrics.locator(':scope > [data-slot="validator-summary-cell"][data-metric="rewards"]')
    const rewardValue = rewardCell.locator('[data-slot="validator-summary-value"]')
    await expect(rewardValue).toHaveText('123456789012345678.9T')
    // The abbreviation stays honest: the exact source digits are one hover away.
    await expect(rewardValue.locator('[title]')).toHaveAttribute('title', exactReward)
    expect((await rewardCell.boundingBox())!.width, 'exceptionally long compact rewards take the whole row instead of splitting digits').toBeCloseTo((await metrics.boundingBox())!.width, 0)
    const overflowing = await metrics.locator('*').evaluateAll(elements => elements
      .filter(element => element.scrollWidth > element.clientWidth + 1)
      .map(element => element.textContent))
    expect(overflowing, 'all three values remain bounded and untruncated').toEqual([])
    await expectNoHorizontalOverflow(page)

    await page.goto('/nodes/' + PUBLIC_NODE_ID)
    const detail = page.getByRole('region', { name: 'Validator performance' })
    await detail.getByRole('button', { name: 'Show full ID' }).click()
    await expect(detail.getByText(identity, { exact: true })).toBeVisible()
    await expect(metricValue(detail, 'Cumulative rewards')).toHaveText('123,456,789,012,345,678,901,234,567,890.1234')
    await expect(detail.getByTitle(exactReward, { exact: true })).toBeVisible()
    await expectDetailMetrics(detail)
    await expectNoHorizontalOverflow(page)
  })

  for (const reward of ['10', '305614.775625389856']) {
    test('keeps the ' + reward + ' reward overview compact and disclosures consistently collapsed', async ({ page }, testInfo) => {
      await page.route('**/api/public/v1/nodes/' + PUBLIC_NODE_ID, async route => {
        const response = await route.fetch()
        const node: PublicNode = await response.json()
        const receivedAt = new Date().toISOString()
        await route.fulfill({ response, json: { ...node, validatorIdentityReason: null, validator: { ...node.validator,
          displayName: null, state: 'fresh', freshness: 'fresh', receivedAt,
          currentValidatorStatus: 'validator', currentValidatorStatusState: 'current', currentValidatorStatusQualifier: null,
          activity: 'active', activityState: 'current', counterState: 'normal',
          blockCount: 32949, rewardAmount: reward, rank: 191, rankState: 'ranked', rankFreshness: 'fresh', rankReceivedAt: receivedAt,
          blockRate: '100.027322', blockRateState: 'ok', genBlocksRate: '100', delegationRewardPercentage: '0',
        } } })
      })
      await loginAs(page)
      await page.goto('/nodes/' + PUBLIC_NODE_ID)
      const detail = page.getByRole('region', { name: 'Validator performance' })
      await expect(detail).toBeVisible()
      const summaries = page.locator('summary').filter({ hasText: 'Validator diagnostics' })
      await expect(summaries).toHaveCount(1)
      const disclosures = summaries.locator('..')
      // The diagnostics sits outside the performance card since #225, so the
      // full-precision reward is read from the page-level disclosure.
      const fullReward = disclosures.getByText('Cumulative rewards (full precision)', { exact: true }).locator('xpath=following-sibling::dd')
      await expect(fullReward).toHaveText(reward === '10' ? '10' : '305,614.775625389856')
      const expectClosed = async () => {
        for (const disclosure of await disclosures.all()) {
          await expect(disclosure).not.toHaveAttribute('open')
          await expect(disclosure.locator(':scope > div')).toBeHidden()
        }
        await expect(detail.getByLabel(/Validator identifier:/)).toHaveCount(0)
        await expect(fullReward).toBeHidden()
      }
      await expectClosed()
      const historyHeading = page.getByRole('heading', { name: 'Latest 60 seconds', exact: true })
      const documentY = () => historyHeading.evaluate(element => element.getBoundingClientRect().y + window.scrollY)
      const collapsedHistoryY = await documentY()
      const collapsedHeight = (await detail.boundingBox())!.height
      if (reward !== '10') {
        const path = testInfo.outputPath('linked-validator-overview.png')
        await detail.screenshot({ path })
        await testInfo.attach('Linked Validator overview — ' + testInfo.project.name, { path, contentType: 'image/png' })
        if (testInfo.project.name === 'desktop-1280') {
          const pagePath = testInfo.outputPath('node-detail-collapsed.png')
          await page.evaluate(() => window.scrollTo(0, 0))
          await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(0)
          await page.screenshot({ path: pagePath, fullPage: true })
          await testInfo.attach('Node detail with collapsed disclosures', { path: pagePath, contentType: 'image/png' })
        }
      }
      await expect(metricValue(detail, 'Cumulative rewards')).toHaveText(reward === '10' ? '10' : '305,614.7756')
      await expect(detail.getByTitle(reward === '10' ? '10' : '305,614.775625389856', { exact: true })).toBeVisible()
      await expectDetailMetrics(detail)
      const width = page.viewportSize()!.width
      expect((await detail.boundingBox())!.height, 'closed overview stays compact').toBeLessThanOrEqual(width >= 1024 ? 360 : width >= 640 ? 440 : 640)
      const identity = detail.locator('code').first()
      const identityLabel = detail.getByText('Validator ID', { exact: true })
      const copy = detail.getByRole('button', { name: 'Copy full Validator identifier' })
      const show = detail.getByRole('button', { name: 'Show full ID' })
      const textBox = await identity.evaluate(element => {
        const range = document.createRange()
        range.selectNodeContents(element)
        const rect = range.getBoundingClientRect()
        // Wrapped ID lines leave natural whitespace before the adjacent controls.
        // For a single line, measure the text itself to reject a stretched desktop ID.
        const wrapped = rect.height > parseFloat(getComputedStyle(element).lineHeight) + 1
        return { right: wrapped ? element.getBoundingClientRect().right : rect.right, y: rect.y, bottom: rect.bottom }
      })
      const labelBox = (await identityLabel.boundingBox())!
      const copyBox = (await copy.boundingBox())!
      const showBox = (await show.boundingBox())!
      if (copyBox.y >= textBox.bottom - 1) {
        // Narrow widths wrap the paired controls beneath the ID; they keep the
        // row start rather than drifting to the far edge.
        expect(copyBox.x, 'wrapped copy aligns with the identity row start').toBeCloseTo(labelBox.x, 0)
      } else {
        expect(copyBox.x - textBox.right, 'copy stays adjacent to the shortened ID, not at the far edge').toBeGreaterThanOrEqual(0)
        expect(copyBox.x - textBox.right).toBeLessThanOrEqual(12)
        expect(copyBox.y).toBeLessThanOrEqual(textBox.bottom)
        expect(copyBox.y + copyBox.height).toBeGreaterThanOrEqual(textBox.y)
      }
      expect(showBox.x - copyBox.x - copyBox.width).toBeGreaterThanOrEqual(0)
      expect(showBox.x - copyBox.x - copyBox.width).toBeLessThanOrEqual(12)
      expect(showBox.y).toBeCloseTo(copyBox.y, 0)
      await show.click()
      for (const summary of await summaries.all()) await summary.click()
      for (const disclosure of await disclosures.all()) {
        await expect(disclosure).toHaveAttribute('open', '')
        await expect(disclosure.locator(':scope > div')).toBeVisible()
      }
      await expect(detail.getByLabel(/Validator identifier:/)).toBeVisible()
      await expect(disclosures.getByText('Cumulative rewards (full precision)', { exact: true })).toBeVisible()
      await expect(fullReward).toBeVisible()
      await testInfo.attach('Overview layout measurements', {
        body: JSON.stringify({ viewport: page.viewportSize(), collapsedHeight, collapsedHistoryY, expandedHistoryY: await documentY() }),
        contentType: 'application/json',
      })
      await page.reload()
      await expect(detail).toBeVisible()
      await expectClosed()
      await expectNoHorizontalOverflow(page)
      if (reward !== '10' && testInfo.project.name === 'desktop-1280') {
        // Exercise both sides of each breakpoint, not only the fixed project sizes.
        for (const width of [639, 640, 1023, 1024]) {
          await page.setViewportSize({ width, height: 900 })
          await expectDetailMetrics(detail)
          await expectNoHorizontalOverflow(page)
        }
      }
    })
  }

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
        validatorNodeId: identity, state: 'error', freshness: 'stale', currentValidatorStatus: 'validator',
        currentValidatorStatusState: 'current', currentValidatorStatusQualifier: null,
        rankState: 'error', blockRateState: 'not_applicable', blockRate: null, counterState: 'counter_reset',
      } } })
    })
    await loginAs(page)
    await page.goto('/nodes/' + PUBLIC_NODE_ID)
    const detail = page.getByRole('region', { name: 'Validator performance' })
    // The diagnostics disclosure lives after the chart deck since #225.
    const summary = page.locator('summary').filter({ hasText: 'Validator diagnostics' })
    const touch = Boolean(testInfo.project.use.hasTouch || isMobile)
    await expect(detail.getByText(identity, { exact: true })).toHaveCount(0)
    const show = detail.getByRole('button', { name: 'Show full ID' })
    if (touch) await show.tap()
    else { await show.focus(); await page.keyboard.press('Enter') }
    await expect(detail.getByText(identity, { exact: true })).toBeVisible()
    await expect(summary.locator('..')).not.toHaveAttribute('open')
    const hide = detail.getByRole('button', { name: 'Hide full ID' })
    if (touch) await hide.tap()
    else { await hide.focus(); await page.keyboard.press('Space') }
    await expect(detail.getByText(identity, { exact: true })).toHaveCount(0)
    await detail.getByRole('button', { name: 'Copy full Validator identifier' }).click()
    await expect(detail.getByRole('status', { name: 'Identifier copy status' })).toHaveText(/Copy failed.*manually/)
    await expect(detail.getByText(identity, { exact: true })).toBeVisible()
    await expect(detail.getByText(identity, { exact: true })).toHaveCSS('user-select', 'text')
    for (const reason of [/source could not be read.*retained and stale/, /last established association/,
      /last successful rank is retained/, /zero scheduled-block denominator/, /prior value was not treated as normal growth/]) {
      await expect(detail.getByText(reason)).toBeVisible()
    }
    await expect(page.getByLabel('Public Validator states')).toBeHidden()
    if (touch) await summary.tap()
    else { await summary.focus(); await page.keyboard.press('Enter') }
    await expect(page.getByLabel('Public Validator states')).toBeVisible()
    await expect(page.getByText('Last success', { exact: true })).toBeVisible()
    if (touch) await summary.tap()
    else await page.keyboard.press('Space')
    await expect(page.getByLabel('Public Validator states')).toBeHidden()
    await expect(detail.getByText(identity, { exact: true })).toBeVisible()
    await expectDetailMetrics(detail)
    await expectNoHorizontalOverflow(page)
  })

  test('distinguishes Unranked, an unconfirmed identity and a retained last-good rank', async ({ page }) => {
    // A confirmed producing Validator that a complete live-staking cohort list
    // omits is authoritative Unranked, never zero and never a failure.
    await page.route('**/api/public/v1/networks', async route => {
      const response = await route.fetch()
      const networks: PublicNetwork[] = await response.json()
      await route.fulfill({ response, json: networks.map(network => ({ ...network, nodes: network.nodes.map(node => (
        node.validator?.displayName === 'Home Producing Validator'
          ? { ...node, validator: { ...node.validator, rank: null, rankState: 'unranked', rankFreshness: 'fresh' } }
          : node)) })) })
    })
    await loginAs(page)
    const hCard = page.getByRole('link', { name: /Node H/ }).locator('xpath=ancestor::article[1]')
    await expect(hCard).toBeVisible()
    await expect(metricValue(hCard, 'Network rank')).toHaveText('Unranked')

    // Node M: the Server's authoritative non-Validator verdict replaces the
    // whole summary with one short line, and the ranking evidence that proves it
    // is Unranked stays on Node detail rather than on the card.
    const mCard = page.getByRole('link', { name: /Node M/ }).locator('xpath=ancestor::article[1]')
    await expect(mCard).toBeVisible()
    await expect(mCard.locator('[data-slot="linked-validator-identity"]')).toHaveText(/^Non-validator/)
    await expect(mCard.getByRole('group', { name: 'Linked Validator metrics' })).toHaveCount(0)

    // Node N: the ranking list failed, so the last-good rank is retained on the
    // card and the full explanation stays on Node detail.
    const nCard = page.getByRole('link', { name: /Node N/ }).locator('xpath=ancestor::article[1]')
    await expect(nCard).toBeVisible()
    await expect(metricValue(nCard, 'Network rank')).toHaveText('#2')
    await expect(nCard.getByRole('status', { name: /Validator data: Failed/ })).toBeVisible()

    const mName = await mCard.getByRole('link', { name: /Node M/ }).textContent()
    expect(mName).not.toBeNull()
    const hPath = await hCard.getByRole('link', { name: /Node H/ }).getAttribute('href')
    expect(hPath).not.toBeNull()
    await page.route('**/api/public/v1' + hPath, async route => {
      const response = await route.fetch()
      const node: PublicNode = await response.json()
      expect(node.validator?.currentValidatorStatus).toBe('validator')
      await route.fulfill({ response, json: { ...node, validator: { ...node.validator,
        rank: null, rankState: 'unranked', rankFreshness: 'fresh' } } })
    })
    await mCard.getByRole('link', { name: /Node M/ }).click()
    // A confirmed non-Validator has no performance panel on Node Detail either.
    await expect(page.getByRole('heading', { name: mName!, exact: true })).toBeVisible()
    await expect(page.getByRole('region', { name: 'Validator performance' })).toHaveCount(0)
    // The same Unranked evidence remains readable with all six metrics on an
    // eligible Validator's detail, rather than inventing a panel for Node M.
    await page.goto(hPath!)
    const detail = page.getByRole('region', { name: 'Validator performance' })
    await expect(metricValue(detail, 'Network rank')).toHaveText('Unranked')
    await expectDetailMetrics(detail)

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
    // One short identity line replaces the whole summary: no six-row Unknown
    // placeholder, no reserved metric height and no centering (#238).
    const identity = linked.locator('[data-slot="linked-validator-identity"]')
    await expect(identity).toBeVisible()
    await expect(identity.locator('[data-slot="linked-validator-identity-label"]')).toHaveText('Non-validator')
    // The reason stops repeating itself on the card face (#238 round 2): one
    // word stays visible and the sentence moves to the line's own title, where
    // it is still reachable without crowding the identity row.
    await expect(identity).toHaveText('Non-validator')
    await expect(identity).toHaveAttribute('title', 'No current staking identity.')
    await expect(identity.locator('[data-slot="linked-validator-identity-reason"]')).toHaveCount(0)
    await expect(linked.getByRole('group', { name: 'Linked Validator metrics' })).toHaveCount(0)
    await expect(card.getByText('No validator metrics')).toHaveCount(0)
    await expect(card.getByText('Loading Validator metrics…')).toHaveCount(0)
    // The removed association area is not replaced by a Not a Validator line.
    await expect(linked.getByText('Not a Validator', { exact: true })).toHaveCount(0)
    await expect(card.getByText('Data: No live Validator')).toHaveCount(0)
    await expect(card.getByRole('button', { name: 'Open Validator details' })).toHaveCount(0)
    const regionBox = (await card.locator('[data-node-region="validator"]').boundingBox())!
    const identityBox = (await identity.boundingBox())!
    expect(regionBox.height, 'the identity line is the whole region').toBeLessThanOrEqual(48)
    expect(identityBox.y - regionBox.y).toBeLessThanOrEqual(13)
    const cardHeight = await card.evaluate(element => element.getBoundingClientRect().height)
    expect(cardHeight, 'a card without a confirmed Validator keeps its natural height').toBeLessThanOrEqual(300)
    await expectNoHorizontalOverflow(page)
  })
})

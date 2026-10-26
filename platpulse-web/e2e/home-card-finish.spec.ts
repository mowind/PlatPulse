import { expect, test, type Locator } from '@playwright/test'
import type { PublicNetwork, PublicValidatorInsight } from '../src/api/generated'
import { closeHomeSurface, expectNoHorizontalOverflow, loginAs, openHomeSort, waitForAnimationsToSettle } from './helpers'

// Measure the painted Home cards, not an internal layout hook. Row membership
// comes from their actual top coordinates, so one-column layouts work too.
async function cardGeometry(cards: Locator) {
  await waitForAnimationsToSettle(cards.page())
  await expect.poll(() => cards.evaluateAll(elements => elements.every(card =>
    Math.abs(card.getBoundingClientRect().top - card.parentElement!.getBoundingClientRect().top) <= 1,
  ))).toBe(true)
  const geometry = await cards.evaluateAll(elements => elements.map(card => {
    const box = card.getBoundingClientRect()
    const regions = ['identity', 'resource', 'chain', 'validator'].map(name => {
      const region = card.querySelector('[data-node-region="' + name + '"]')!.getBoundingClientRect()
      return { top: region.top - box.top, height: region.height }
    })
    const content = card.querySelector('[data-slot="card-x"]')!.getBoundingClientRect()
    const speed = card.querySelector('[data-slot="host-network-speed"]')!.getBoundingClientRect()
    const chain = card.querySelector('[data-slot="node-business-metrics"]')!.getBoundingClientRect()
    const cells = [...card.querySelectorAll('[data-slot="validator-summary-cell"]')]
    return {
      y: box.y, bottom: box.bottom, width: box.width, height: box.height, regions,
      contentHeight: content.height,
      speedGap: chain.top - speed.bottom,
      summaryRows: new Set(cells.map(cell => Math.round(cell.getBoundingClientRect().top))).size,
      metadataHeight: card.querySelector('[data-slot="node-identity-meta"] p')!.getBoundingClientRect().height,
      diagnosticHeight: card.querySelector('[data-slot="node-diagnostic"]')?.getBoundingClientRect().height ?? 0,
    }
  }))
  for (const card of geometry) {
    expect(card.contentHeight, 'the full content remains inside the natural card height').toBeLessThanOrEqual(card.height + 1)
    const row = geometry.filter(peer => Math.abs(peer.y - card.y) <= 1)
    for (const peer of row) expect(Math.abs(peer.bottom - card.bottom), 'same-row bottoms coordinate').toBeLessThanOrEqual(1)
    // Compaction must not migrate into a Speed spacer or reserved region gaps.
    expect(card.speedGap).toBeCloseTo(10, 0)
    expect(card.regions[2].top - card.regions[1].top - card.regions[1].height).toBeCloseTo(10, 0)
    expect(card.regions[3].top - card.regions[2].top - card.regions[2].height).toBeCloseTo(10, 0)
    // Measure padding against the content surface, not a shorter card stretched
    // legitimately to its own row's bottom.
    expect(card.contentHeight - card.regions[3].top - card.regions[3].height).toBeCloseTo(16, 0)
  }
  return geometry
}

test('ordinary Validator cards are compact and an exception only grows its own row without a Speed spacer', async ({ page }, testInfo) => {
  await loginAs(page)
  let exceptional = false
  const longName = 'B very long Node name that must remain accessible on a narrow screen'
  const diagnostic = 'RPC observation failed. This sanitized diagnostic deliberately wraps across several readable lines while preserving the current health reason and leaving every unrelated grid row at its ordinary content-driven height.'
  await page.route('**/api/public/v1/networks', async route => {
    const response = await route.fetch()
    const [network]: PublicNetwork[] = await response.json()
    const source = network.nodes.find(node => node.validator?.blockCount != null)!
    const base = {
      ...source, health: 'healthy', healthReason: 'Fresh component observations',
      rpcState: 'ok', syncState: 'ok', freshness: new Date().toISOString(), resyncState: 'normal', currentHead: 1000, historicalHighWatermark: 2000,
      networkReferenceHead: 1000, networkReferenceConfidence: 'high', lastReportAt: new Date().toISOString(),
      processCpuPercent: 12, processMemoryPercent: 24, processUptimeMs: 3_600_000,
      nodeDataDirectorySizeBytes: 25_000, nodeDataDirectoryCapacityBytes: 100_000,
      hostNetworkTxBytesPerSec: 1250, hostNetworkRxBytesPerSec: 2500,
      peers: { ...source.peers, peerCount: 3, state: 'ok', freshness: 'current' },
      validator: { ...source.validator!, currentValidatorStatus: 'validator', currentValidatorStatusState: 'current',
        state: 'ok', freshness: 'fresh', rank: 2, rankState: 'ranked', rankFreshness: 'fresh',
        blockCount: 255, rewardAmount: '32', activity: 'active' },
    }
    const nodes = Array.from({ length: 7 }, (_, index) => ({
      ...base, nodeId: 'finish-' + index, displayName: String.fromCharCode(65 + index),
      ...(exceptional && index === 1 ? { displayName: longName, health: 'unhealthy', healthReason: diagnostic, resyncState: 'resyncing' } : {}),
    }))
    await route.fulfill({ response, json: [{ ...network, displayName: 'Mainnet', nodes }] })
  })
  await page.goto('/?sort=name')
  const cards = page.locator('[data-slot="node-card"]')
  await expect(cards).toHaveCount(7)
  for (const dark of [false, true]) {
    exceptional = false
    await page.goto('/?sort=name')
    await expect(cards).toHaveCount(7)
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    await page.mouse.move(0, 0)
    await expect(cards.first().getByText('At observed head', { exact: true })).toBeVisible()
    await expect(cards.locator('[data-slot="node-peers-cue"]')).toHaveCount(0)
    const ordinary = await cardGeometry(cards)
    console.log(`[home-card-finish] ${testInfo.project.name} ${dark ? 'dark' : 'light'} ordinary target=250–290: ${JSON.stringify(ordinary.map(card => ({ ...card, growthReason: card.height > 290 ? 'narrow two-row Validator summary; metadata height=' + card.metadataHeight : null })))}`)
    for (const card of ordinary) {
      expect(card.regions[2].height, 'ordinary chain is one compact Head line').toBeLessThanOrEqual(40)
      if (card.width >= 400) {
        // 250–290px is a compactness aim, not a minimum-height reservation. A
        // quieter fully-current card may be shorter while all three metrics stay.
        expect(card.height, 'ordinary complete Validator compactness target').toBeLessThanOrEqual(290)
      } else if (card.height > 290) {
        // Narrow cards retain the third metric on another row, not a hard crop.
        expect(card.summaryRows, 'natural growth has measured narrow-summary wrapping').toBeGreaterThan(1)
      }
    }
    exceptional = true
    await page.goto('/?sort=name')
    await expect(cards).toHaveCount(7)
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    // Round 2: the card face keeps one short diagnostic line, and the Server's whole
    // sentence stays reachable from that line's own title and from the identity
    // entry the card already opens. The layout assertions below then measure the one
    // line this adds.
    const diagnosticLine = cards.nth(1).locator('[data-slot="node-diagnostic"]')
    await expect(diagnosticLine).toHaveAttribute('title', diagnostic)
    const shortDiagnostic = (await diagnosticLine.textContent()) ?? ''
    expect(shortDiagnostic, 'one short line, not the whole Server sentence').not.toBe(diagnostic)
    expect(shortDiagnostic.startsWith('RPC observation failed.'), 'the Server wording still opens the line').toBe(true)
    expect(shortDiagnostic.endsWith('…'), 'a shortened sentence says so').toBe(true)
    expect(shortDiagnostic.length, 'the shortened reason stays one line').toBeLessThanOrEqual(41)
    await cards.nth(1).getByRole('button', { name: 'Node identity details' }).click()
    await expect(page.getByRole('dialog').getByText('Current health diagnostic: ' + diagnostic)).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(page.getByRole('dialog')).toHaveCount(0)
    await page.mouse.move(0, 0)
    const expanded = await cardGeometry(cards)
    for (const [index, card] of expanded.entries()) {
      const inExceptionalRow = Math.abs(ordinary[index].y - ordinary[1].y) <= 1
      if (inExceptionalRow) expect(card.height, 'only the diagnostic row grows').toBeGreaterThan(ordinary[index].height)
      else expect(Math.abs(card.height - ordinary[index].height), 'unrelated row keeps its ordinary height').toBeLessThanOrEqual(1)
    }
    expect(expanded[1].diagnosticHeight, 'specific growth cause is the readable diagnostic').toBeGreaterThan(0)
    expect(expanded[1].diagnosticHeight, 'the diagnostic is one line, however long the reason').toBeLessThanOrEqual(20)
    console.log(`[home-card-finish] ${testInfo.project.name} ${dark ? 'dark' : 'light'} exception: long accessible name, wrapped diagnostic and independent Resyncing; ${JSON.stringify(expanded)}`)
    await expect(cards.nth(1).getByRole('link', { name: longName, exact: true })).toHaveAttribute('title', longName)
    expect(await cards.locator('h2 a').evaluateAll(links => links.map(link => link.getAttribute('href'))))
      .toEqual(Array.from({ length: 7 }, (_, index) => '/nodes/finish-' + index))
    for (const card of await cards.all()) {
      await expect(card.getByRole('group', { name: 'Linked Validator metrics' })).toBeVisible()
      const overflow = await card.locator('[data-slot="metric-row-value"], [data-slot="resource-value"], [data-slot="resource-label"], [data-slot="validator-summary-value"], [data-slot="validator-summary-label"]')
        .evaluateAll(elements => elements.filter(el => el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1).map(el => el.textContent))
      expect(overflow, 'natural growth must not clip a metric').toEqual([])
    }
    await expectNoHorizontalOverflow(page)
  }
})

test('mixed Validator identity states keep per-row heights without empty placeholders', async ({ page }, testInfo) => {
  await loginAs(page)
  await page.route('**/api/public/v1/networks', async route => {
    const response = await route.fetch()
    const [network]: PublicNetwork[] = await response.json()
    const base = network.nodes.find(node => node.validator?.blockCount != null)!
    const good: PublicValidatorInsight = {
      ...base.validator!, currentValidatorStatus: 'validator', currentValidatorStatusState: 'current',
      state: 'ok', freshness: 'fresh', rank: 2, rankState: 'ranked', rankFreshness: 'fresh',
      blockCount: 255, rewardAmount: '32',
    }
    const missing: PublicValidatorInsight = {
      // rank=null alone is not Unknown: the Server's rankState is independent
      // evidence and a complete successful cohort may explicitly be Unranked.
      ...good, blockCount: null, rewardAmount: null, rank: null, rankState: 'unknown',
      rankFreshness: 'unknown', rankReceivedAt: null, rankCohortSize: null,
      blockRate: null, blockRateState: 'unknown', expectedBlockCount: null,
      genBlocksRate: null, delegationRewardPercentage: null,
    }
    // The first three cards carry one short identity line each — a confirmed
    // non-Validator with a current and with a retained verdict, and the case with
    // no Link at all. The rest keep the three-cell summary, including one whose
    // metrics are still Unknown and one that retains last-good values after the
    // Provider failed (#238).
    const validators = [
      { ...missing, currentValidatorStatus: 'not_validator', currentValidatorStatusState: 'current' },
      { ...missing, currentValidatorStatus: 'not_validator', currentValidatorStatusState: 'stale' },
      null,
      missing,
      good,
      { ...good, rewardAmount: '42.5' },
      { ...good, state: 'error', freshness: 'stale' },
      { ...good, rankState: 'error', rankFreshness: 'stale' },
    ]
    await route.fulfill({ response, json: [{ ...network, displayName: 'Mainnet', nodes: validators.map((validator, index) => ({
      ...base, nodeId: 'state-' + index, displayName: 'State ' + index, validator,
      validatorIdentityState: null,
    })) }] })
  })
  await page.goto('/')
  await (await openHomeSort(page)).selectOption('name')
  await closeHomeSurface(page)
  await page.mouse.move(0, 0)
  const cards = page.locator('[data-slot="node-card"]')
  await expect(cards).toHaveCount(8)
  for (const dark of [false, true]) {
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    await expect.poll(() => cards.evaluateAll(elements => elements
      .map(card => card.querySelectorAll('[data-slot="validator-summary-cell"]').length).join(',')))
      .toBe('0,0,0,3,3,3,3,3')
    // The six-row Unknown placeholder and the centered empty state are gone from
    // the Home grid (#238): an unconfirmed identity is one short line, not a
    // reserved metrics panel.
    for (const removed of ['No validator metrics', 'Loading Validator metrics…']) {
      await expect(page.getByText(removed)).toHaveCount(0)
    }
    expect(await cards.locator('[data-slot="linked-validator-identity"]').evaluateAll(elements => elements.map(element => element.textContent)))
      .toEqual([
        // A confirmed non-Validator keeps one word: its reason is carried by the
        // line's own title instead of repeating in the footer (#238 round 2),
        // while the unconfirmed identities keep their reason on the face.
        'Non-validator',
        'Validator unknown — Confirmed non-Validator status retained.',
        'Validator unknown — Identity has not been observed yet.',
      ])
    // A line that keeps its reason off the face carries that reason as its own
    // title (#238 round 2). The absent-Link line already states its whole short
    // reason on the face, so it needs no tooltip and the Server's long
    // association-identity paragraph stays on Node detail.
    expect(await cards.locator('[data-slot="linked-validator-identity"]').evaluateAll(elements => elements.map(element => element.getAttribute('title'))))
      .toEqual([
        'No current staking identity.',
        'Confirmed non-Validator status retained.',
        null,
      ])
    await waitForAnimationsToSettle(page)
    const geometry = await cards.evaluateAll(elements => elements.map(card => {
      const box = card.getBoundingClientRect()
      const region = card.querySelector('[data-node-region="validator"]')!.getBoundingClientRect()
      return { y: Math.round(box.y), width: Math.round(box.width), height: Math.round(box.height), validator: Math.round(region.height) }
    }))
    // Cards on one row still line up, while the grid no longer forces one shared
    // height on every row: an identity line reserves no metric height at all.
    for (const y of new Set(geometry.map(card => card.y))) {
      const heights = geometry.filter(card => card.y === y).map(card => card.height)
      expect(new Set(heights).size, 'one row shares one card height: ' + JSON.stringify(heights)).toBe(1)
    }
    expect(new Set(geometry.map(card => card.height)).size, 'rows are not equalized with each other').toBeGreaterThan(1)
    for (const card of geometry.slice(0, 3)) {
      expect(card.validator, 'an identity line reserves no metric height').toBeLessThan(60)
      // Whole-card height may include narrow metadata wrapping or a taller peer
      // in this row; the actual identity region must stay short, not reserved.
    }
    // Three Unknown values are real values, never filler, and a Provider failure
    // keeps the last-good value it still has.
    // The value's textContent also includes its screen-reader-only reason.
    // Keep exact Unknown + explanation assertions; Unranked or zero cannot pass.
    await expect(cards.nth(3).locator('[data-slot="validator-summary-value"]')).toHaveText([
      'Unknown: No Network ranking has been observed yet.',
      'Unknown: No cumulative block count is available from the Validator source.',
      'Unknown: No cumulative reward amount is available from the Validator source.',
    ])
    await expect(cards.nth(3).locator('[data-metric="rank"] [aria-describedby]'))
      .toHaveAccessibleDescription(': No Network ranking has been observed yet.')
    await expect(cards.nth(5).locator('[data-slot="validator-summary-cell"][data-metric="rewards"] [data-slot="validator-summary-value"]')).toHaveText('42.5')
    await expect(cards.nth(6).locator('[data-slot="validator-summary-cell"][data-metric="blocks"] [data-slot="validator-summary-value"]')).toHaveText('255')
    // A failed ranking list keeps the last-good Network rank on the card.
    await expect(cards.nth(7).locator('[data-slot="validator-summary-cell"][data-metric="rank"] [data-slot="validator-summary-value"]')).toHaveText('#2')
    await expect(cards.nth(7).getByRole('status', { name: /Validator data/ })).toBeVisible()
    const overflowing = await cards.locator('[data-slot="linked-validator"]')
      .evaluateAll(elements => elements.filter(el => el.scrollWidth > el.clientWidth + 1).map(el => el.textContent))
    expect(overflowing).toEqual([])
    await expectNoHorizontalOverflow(page)
    console.log(`[home-card-finish] ${testInfo.project.name}${dark ? ' dark' : ' light'}: ${JSON.stringify(geometry)}`)
  }
})


test('a constrained Home container chooses readable tracks independently of viewport width', async ({ page }, testInfo) => {
  await loginAs(page)
  await page.route('**/api/public/v1/networks', async route => {
    const response = await route.fetch()
    const [network]: PublicNetwork[] = await response.json()
    const source = network.nodes.find(node => node.validator?.blockCount != null)!
    const nodes = Array.from({ length: 4 }, (_, index) => ({
      ...source, nodeId: 'constrained-' + index, displayName: 'Constrained ' + index,
    }))
    await route.fulfill({ response, json: [{ ...network, displayName: 'Mainnet', nodes }] })
  })
  await page.goto('/?sort=name')
  const cards = page.locator('[data-slot="node-card"]')
  await expect(cards).toHaveCount(4)
  for (const dark of [false, true]) {
    // Constrain the rendered page, not its viewport or a test-only component prop.
    await page.locator('main').evaluate(element => { element.style.maxWidth = '600px' })
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    await waitForAnimationsToSettle(page)
    const geometry = await cards.evaluateAll(elements => elements.map(card => {
      const rect = card.getBoundingClientRect()
      return { x: rect.x, y: rect.y, width: rect.width }
    }))
    expect(new Set(geometry.map(card => Math.round(card.x))).size, '600px container cannot fit two readable 360px tracks').toBe(1)
    for (const [index, card] of geometry.entries()) {
      expect(card.width).toBeLessThanOrEqual(600)
      if (index > 0) expect(card.y).toBeGreaterThan(geometry[index - 1].y)
    }
    await expect(cards.first().getByRole('group', { name: 'Linked Validator metrics' })).toBeVisible()
    const overflow = await cards.locator('[data-slot="resource-value"], [data-slot="validator-summary-value"]')
      .evaluateAll(elements => elements.filter(el => el.scrollWidth > el.clientWidth + 1).map(el => el.textContent))
    expect(overflow, 'readable values stay inside the constrained container').toEqual([])
    await expectNoHorizontalOverflow(page)
    console.log('[home-container] ' + testInfo.project.name + ' ' + (dark ? 'dark' : 'light') + ': ' + JSON.stringify(geometry))
  }
})

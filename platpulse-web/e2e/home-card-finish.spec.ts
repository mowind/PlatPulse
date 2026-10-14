import { expect, test } from '@playwright/test'
import type { PublicNetwork, PublicValidatorInsight } from '../src/api/generated'
import { expectNoHorizontalOverflow, loginAs, openHomeSort } from './helpers'

test('resource content determines the shared card height without a Speed spacer', async ({ page }, testInfo) => {
  await loginAs(page)
  await page.route('**/api/public/v1/networks', async route => {
    const response = await route.fetch()
    const [network]: PublicNetwork[] = await response.json()
    const base = network.nodes.find(node => node.validator?.blockCount != null)!
    const nodes = Array.from({ length: 7 }, (_, index) => ({
      ...base, nodeId: 'finish-' + index,
      displayName: index === 1 ? 'B very long Node name that must remain accessible on a narrow screen' : String.fromCharCode(65 + index),
      resyncState: index === 1 ? 'resyncing' : base.resyncState,
    }))
    await route.fulfill({ response, json: [{ ...network, nodes }] })
  })
  await page.goto('/')
  await (await openHomeSort(page)).selectOption('name')
  const cards = page.locator('[data-slot="node-card"]')
  await expect(cards).toHaveCount(7)
  await expect.poll(async () => cards.evaluateAll(elements => elements.map(card => {
    const speed = card.querySelector('[data-slot="host-network-speed"]')!.getBoundingClientRect()
    const chain = card.querySelector('[data-slot="node-business-metrics"]')!.getBoundingClientRect()
    return chain.top - speed.bottom
  }))).toEqual(Array(7).fill(10))
  const geometry = await cards.evaluateAll(elements => elements.map(card => {
    const box = card.getBoundingClientRect()
    const regions = ['identity', 'resource', 'chain', 'validator'].map(name => {
      const region = card.querySelector('[data-node-region="' + name + '"]')!.getBoundingClientRect()
      return { top: region.top - box.top, height: region.height }
    })
    return { width: box.width, height: box.height, regions }
  }))
  for (const card of geometry) {
    expect(card).toEqual(geometry[0])
    // Saved resource space must not migrate into region gaps or bottom padding.
    expect(card.regions[2].top - card.regions[1].top - card.regions[1].height).toBe(10)
    expect(card.regions[3].top - card.regions[2].top - card.regions[2].height).toBe(10)
    expect(card.height - card.regions[3].top - card.regions[3].height).toBe(16)
    // The compaction is structural, not a size trick: the chain block is the one
    // compact Head line it now carries (#237).
    expect(card.regions[2].height, 'the chain block is one compact Head line').toBeLessThanOrEqual(40)
  }
  console.log(`[home-card-finish] ${testInfo.project.name}: card ${Math.round(geometry[0].width)}x${Math.round(geometry[0].height)}, resource ${Math.round(geometry[0].regions[1].height)}, chain ${Math.round(geometry[0].regions[2].height)}`)
  expect(await cards.locator('h2 a').evaluateAll(links => links.map(link => link.getAttribute('href'))))
    .toEqual(Array.from({ length: 7 }, (_, index) => '/nodes/finish-' + index))
  for (const card of await cards.all()) {
    await expect(card.getByRole('group', { name: 'Linked Validator metrics' })).toBeVisible()
    const overflow = await card.locator('[data-slot="metric-row-value"], [data-slot="resource-value"], [data-slot="resource-label"], [data-slot="validator-summary-value"], [data-slot="validator-summary-label"]')
      .evaluateAll(elements => elements.filter(el => el.scrollWidth > el.clientWidth + 1).map(el => el.textContent))
    expect(overflow).toEqual([])
  }
  await expectNoHorizontalOverflow(page)
})

test('mixed Validator identity states keep per-row heights without empty placeholders', async ({ page }, testInfo) => {
  await loginAs(page)
  await page.route('**/api/public/v1/networks', async route => {
    const response = await route.fetch()
    const [network]: PublicNetwork[] = await response.json()
    const base = network.nodes.find(node => node.validator?.blockCount != null)!
    const good = base.validator!
    const missing: PublicValidatorInsight = {
      ...good, blockCount: null, rewardAmount: null, rank: null, rankState: 'unknown',
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
    await route.fulfill({ response, json: [{ ...network, nodes: validators.map((validator, index) => ({
      ...base, nodeId: 'state-' + index, displayName: 'State ' + index, validator,
      validatorIdentityState: null,
    })) }] })
  })
  await page.goto('/')
  await (await openHomeSort(page)).selectOption('name')
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
        'Non-validator — No current staking identity.',
        'Validator unknown — Confirmed non-Validator status retained.',
        'Validator unknown — Identity has not been observed yet.',
      ])
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
      expect(card.height, 'a card without a confirmed Validator keeps its natural height').toBeLessThanOrEqual(300)
    }
    // Three Unknown values are real values, never filler, and a Provider failure
    // keeps the last-good value it still has.
    await expect(cards.nth(3).locator('[data-slot="validator-summary-value"]')).toHaveText(['Unknown', 'Unknown', 'Unknown'])
    await expect(cards.nth(5).locator('[data-slot="validator-summary-cell"][data-metric="rewards"] [data-slot="validator-summary-value"]')).toHaveText('42.5')
    await expect(cards.nth(6).locator('[data-slot="validator-summary-cell"][data-metric="blocks"] [data-slot="validator-summary-value"]')).not.toHaveText('Unknown')
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

import { expect, test } from '@playwright/test'
import type { PublicNetwork, PublicValidatorInsight } from '../src/api/generated'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

test('resource content determines the shared card height without a Speed spacer', async ({ page }) => {
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
  await page.getByLabel('Sort', { exact: true }).selectOption('name')
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
  }
  expect(await cards.locator('h2 a').evaluateAll(links => links.map(link => link.getAttribute('href'))))
    .toEqual(Array.from({ length: 7 }, (_, index) => '/nodes/finish-' + index))
  for (const card of await cards.all()) {
    await expect(card.getByRole('group', { name: 'Linked Validator metrics' })).toBeVisible()
    const overflow = await card.locator('[data-slot="metric-row-value"], [data-slot="validator-metric"] strong')
      .evaluateAll(elements => elements.filter(el => el.scrollWidth > el.clientWidth + 1).map(el => el.textContent))
    expect(overflow).toEqual([])
  }
  await expectNoHorizontalOverflow(page)
})

test('mixed Validator states retain aligned regions and center only empty messages', async ({ page }) => {
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
    const validators = [
      good,
      { ...missing, state: 'empty', freshness: 'fresh' },
      { ...missing, state: 'loading', freshness: 'unknown' },
      { ...missing, state: 'error', freshness: 'stale' },
      null,
      { ...missing, rewardAmount: '42.5' },
      { ...good, state: 'error', freshness: 'stale', currentValidatorStatus: 'not_validator' },
    ]
    await route.fulfill({ response, json: [{ ...network, nodes: validators.map((validator, index) => ({
      ...base, nodeId: 'state-' + index, displayName: 'State ' + index, validator,
      validatorIdentityState: null,
    })) }] })
  })
  await page.goto('/')
  await page.getByLabel('Sort', { exact: true }).selectOption('name')
  const cards = page.locator('[data-slot="node-card"]')
  await expect(cards).toHaveCount(7)
  for (const dark of [false, true]) {
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    await expect.poll(() => cards.evaluateAll(elements => {
      const shapes = elements.map(card => {
        const box = card.getBoundingClientRect()
        return JSON.stringify([box.width, box.height, ...['identity', 'resource', 'chain', 'validator'].flatMap(name => {
          const region = card.querySelector('[data-node-region="' + name + '"]')!.getBoundingClientRect()
          return [region.top - box.top, region.height]
        })])
      })
      return new Set(shapes).size
    })).toBe(1)
    const messages = [
      'No validator metrics', 'Loading Validator metrics…',
      'The Validator source could not be read; no metrics are available.',
      'Validator identity has not been observed yet.',
    ]
    for (let index = 1; index <= 4; index++) {
      const card = cards.nth(index)
      const empty = card.locator('[data-slot="linked-validator-empty"]')
      await expect(empty).toHaveText(messages[index - 1])
      const region = (await card.locator('[data-node-region="validator"]').boundingBox())!
      const message = (await empty.boundingBox())!
      expect(Math.abs(message.y + message.height / 2 - region.y - region.height / 2)).toBeLessThanOrEqual(1)
      expect(message.x + message.width / 2).toBeCloseTo(region.x + region.width / 2, 0)
      await expect(empty).toHaveCSS('text-align', 'center')
      await expect(card.getByRole('group', { name: 'Linked Validator metrics' })).toHaveCount(0)
    }
    for (const index of [0, 5, 6]) {
      const metrics = cards.nth(index).getByRole('group', { name: 'Linked Validator metrics' })
      await expect(metrics).toBeVisible()
      await expect(metrics.locator(':scope > *')).toHaveCount(6)
      await expect(metrics).not.toHaveCSS('text-align', 'center')
      const region = (await cards.nth(index).locator('[data-node-region="validator"]').boundingBox())!
      const content = (await metrics.boundingBox())!
      expect(content.y + content.height).toBeLessThanOrEqual(region.y + region.height)
    }
    await expect(cards.nth(5).getByText('42.5', { exact: true })).toBeVisible()
    const overflowing = await cards.locator('[data-slot="linked-validator"]')
      .evaluateAll(elements => elements.filter(el => el.scrollWidth > el.clientWidth + 1).map(el => el.textContent))
    expect(overflowing).toEqual([])
    await expectNoHorizontalOverflow(page)
  }
})

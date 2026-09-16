import { expect, test } from '@playwright/test'
import { loginAs } from './helpers'

test('business rows preserve full values, order and neutral roles across widths and themes', async ({ page }) => {
  await loginAs(page)
  await page.route('**/api/public/v1/networks*', async route => {
    const response = await route.fetch()
    const [network] = await response.json()
    const base = network.nodes[0]
    const nodes = [true, false, null].map((validator, i) => ({
      ...base, nodeId: `business-${i}`, displayName: `Node ${i} Extremely Long Production Node Name That Must Not Squeeze The Role`,
      currentHead: 9007199254740991, latestBlockTransactionCount: i === 0 ? 9007199254740991 : 12,
      peers: { ...base.peers, peerCount: i === 0 ? 9007199254740991 : 3 },
      consensus: { state: 'ok', freshness: 'current', validator, highestQcBlock: 9007199254740991, highestLockBlock: 9007199254740990, highestCommitBlock: 9007199254740989 },
    }))
    await route.fulfill({ response, json: [{ ...network, nodes }] })
  })
  await page.goto('/')
  const cards = page.locator('[data-slot="node-card"]')
  await expect(cards).toHaveCount(3)
  for (const dark of [false, true]) {
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    for (const width of [300, 360, 560]) {
      // Exercise card widths independently of the page's viewport/grid breakpoints.
      await cards.evaluateAll((els, w) => els.forEach(el => { Object.assign((el as HTMLElement).style, { width: `${w}px`, transition: 'none' }) }), width)
      for (let i = 0; i < 3; i++) {
        const card = cards.nth(i)
        const badge = card.locator('[data-slot="card-x-header"] [data-slot="validator-role"]')
        await expect(badge).toHaveText(['Validator', 'Non-validator', 'Unknown'][i])
        const metrics = card.locator('[data-slot="node-business-metrics"]')
        await expect(metrics.locator('[data-slot="metric-row-label"]')).toHaveText(['Head', 'QC', 'Locked', 'Committed', 'Txs', 'Peers'])
        const geometry = await metrics.locator('[data-slot="metric-row"]').evaluateAll(els => els.map(el => {
          const label = el.querySelector('[data-slot="metric-row-label"]')!
          const value = el.querySelector('[data-slot="metric-row-value"]')!
          const range = document.createRange()
          range.selectNodeContents(value)
          const rect = el.getBoundingClientRect()
          return { right: value.getBoundingClientRect().right, top: rect.top, bottom: rect.bottom,
            textLines: range.getClientRects().length, fontSize: getComputedStyle(value).fontSize,
            labelRight: label.getBoundingClientRect().right, textLeft: range.getBoundingClientRect().left,
            overflow: el.scrollWidth > el.clientWidth }
        }))
        for (const row of geometry) {
          expect(row.textLines).toBe(1)
          expect(row.fontSize).toBe('14px')
          expect(row.overflow, JSON.stringify({ width, i, geometry })).toBe(false)
          expect(row.textLeft).toBeGreaterThan(row.labelRight)
        }
        for (let j = 1; j < 4; j++) {
          expect(geometry[j].right).toBeCloseTo(geometry[0].right, 1)
          expect(geometry[j].top - geometry[j - 1].top).toBe(28)
        }
        expect(geometry[4].top).toBe(geometry[3].bottom)
        if (i === 0 && width < 560) expect(geometry[5].top).toBe(geometry[4].bottom)
        else expect(geometry[5].top, JSON.stringify({ width, i, geometry })).toBe(geometry[4].top)
        await expect(card.locator('[data-short-label]')).toHaveCount(0)
        expect(await badge.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true)
      }
    }
  }
})

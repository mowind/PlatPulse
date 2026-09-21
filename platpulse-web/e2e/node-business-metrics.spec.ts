import { expect, test } from '@playwright/test'
import type { PublicNetwork } from '../src/api/generated'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

test('business metrics preserve full values, stacked heights, inline counts and neutral roles', async ({ page }, testInfo) => {
  await loginAs(page)
  await page.route('**/api/public/v1/networks*', async route => {
    const response = await route.fetch()
    const [network]: PublicNetwork[] = await response.json()
    const base = network.nodes[0]
    const nodes = [true, false, null].map((validator, i) => ({
      ...base, nodeId: 'business-' + i, displayName: 'Node ' + i + ' Extremely Long Production Node Name That Must Not Squeeze The Role',
      currentHead: i === 0 ? 9007199254740991 : 123456789,
      latestBlockTransactionCount: i === 0 ? 9007199254740991 : 12,
      peers: { ...base.peers, peerCount: i === 0 ? 9007199254740991 : 3 },
      consensus: { state: 'ok', freshness: 'current', validator,
        highestQcBlock: i === 0 ? 9007199254740991 : 123456788,
        highestLockBlock: i === 0 ? 9007199254740990 : 123456787,
        highestCommitBlock: i === 0 ? 9007199254740989 : 123456786 },
    }))
    await route.fulfill({ response, json: [{ ...network, nodes }] })
  })
  await page.goto('/')
  const cards = page.locator('[data-slot="node-card"]')
  await expect(cards).toHaveCount(3)
  for (const dark of [false, true]) {
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    for (const width of [300, 360, 560]) {
      // Deliberately exercise the component independently of page breakpoints.
      await cards.evaluateAll((els, w) => els.forEach(el => {
        Object.assign((el as HTMLElement).style, { width: w + 'px', transition: 'none' })
      }), width)
      for (let i = 0; i < 3; i++) {
        const card = cards.nth(i)
        const badge = card.locator('[data-slot="card-x-header"] [data-slot="validator-role"]')
        await expect(badge).toHaveAttribute('aria-label', 'Role: ' + ['Validator', 'Non-validator', 'Unknown'][i])
        const metrics = card.locator('[data-slot="node-business-metrics"]')
        await expect(metrics.locator('[data-slot="metric-row-label"]')).toHaveText(['Head', 'QC', 'Locked', 'Committed', 'Txs', 'Peers'])
        await expect(metrics.locator('[data-slot="metric-row-value"]')).toHaveText(i === 0
          ? ['9,007,199,254,740,991', '9,007,199,254,740,991', '9,007,199,254,740,990', '9,007,199,254,740,989', '9,007,199,254,740,991', '9,007,199,254,740,991']
          : ['123,456,789', ...(i === 1 ? ['123,456,788', '123,456,787', '123,456,786'] : ['Unknown', 'Unknown', 'Unknown']), '12', '3'])
        const geometry = await metrics.locator('[data-slot="metric-row"]').evaluateAll(els => els.map(el => {
          const label = el.querySelector('[data-slot="metric-row-label"]')!
          const value = el.querySelector('[data-slot="metric-row-value"]')!
          const range = document.createRange()
          range.selectNodeContents(value)
          const rect = el.getBoundingClientRect()
          const labelBox = label.getBoundingClientRect()
          const textBox = range.getBoundingClientRect()
          return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom,
            textTop: textBox.top, textRight: textBox.right, textLeft: textBox.left,
            labelTop: labelBox.top, labelBottom: labelBox.bottom, labelRight: labelBox.right,
            textLines: range.getClientRects().length, fontSize: getComputedStyle(value).fontSize,
            wordBreak: getComputedStyle(label).wordBreak,
            overflow: el.scrollWidth > el.clientWidth }
        }))
        for (const row of geometry) {
          expect(row.textLines).toBe(1)
          expect(row.overflow, JSON.stringify({ width, i, geometry })).toBe(false)
          expect(row.textRight).toBeLessThanOrEqual(row.right + 1)
          expect(row.textLeft).toBeGreaterThanOrEqual(row.left - 1)
          expect(row.wordBreak).toBe('normal')
        }
        for (const row of geometry.slice(0, 4)) {
          expect(row.fontSize).toBe('14px')
          expect(row.textTop).toBeGreaterThanOrEqual(row.labelBottom)
        }
        if (i === 0) {
          // Exceptionally long safe-integer heights/counts span both columns, never truncate.
          expect(geometry[5].left).toBeCloseTo(geometry[4].left, 1)
          expect(geometry[5].right).toBeCloseTo(geometry[4].right, 1)
          expect(geometry[5].top).toBeGreaterThanOrEqual(geometry[4].bottom)
          for (let j = 1; j < 4; j++) {
            expect(geometry[j].left).toBeCloseTo(geometry[0].left, 1)
            expect(geometry[j].right).toBeCloseTo(geometry[0].right, 1)
            expect(geometry[j].top).toBeGreaterThanOrEqual(geometry[j - 1].bottom)
          }
        } else {
          expect(geometry[1].top).toBeCloseTo(geometry[0].top, 1)
          expect(geometry[1].left).toBeGreaterThan(geometry[0].right)
          expect(geometry[2].left).toBeCloseTo(geometry[0].left, 1)
          expect(geometry[3].top).toBeCloseTo(geometry[2].top, 1)
          expect(geometry[3].left).toBeCloseTo(geometry[1].left, 1)
          expect(geometry[2].top).toBeGreaterThan(geometry[0].bottom)
          expect(geometry[5].top).toBeCloseTo(geometry[4].top, 1)
          expect(geometry[5].left).toBeGreaterThan(geometry[4].right)
        }
        for (const row of geometry.slice(4)) {
          expect(row.top).toBeGreaterThanOrEqual(geometry[3].bottom)
          expect(Math.abs(row.textTop - row.labelTop)).toBeLessThanOrEqual(4)
          expect(row.textLeft).toBeGreaterThan(row.labelRight)
        }
        await expect(card.locator('[data-short-label]')).toHaveCount(0)
        expect(await badge.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true)
      }
    }
    // Restore actual responsive widths before page-overflow checks/evidence.
    await cards.evaluateAll(els => els.forEach(el => { (el as HTMLElement).style.removeProperty('width') }))
    await expectNoHorizontalOverflow(page)
    if (process.env.EMERALD_EVIDENCE === '1') await page.screenshot({ path: testInfo.outputPath(dark ? 'business-dark.png' : 'business-light.png'), fullPage: true })
  }
})

test('Linked Validator has six label-over-value metrics and independent details and copy actions', async ({ page }, testInfo) => {
  await loginAs(page)
  const linked = page.locator('[data-slot="node-card"] [data-slot="linked-validator"]').filter({ has: page.getByRole('group', { name: 'Linked Validator metrics' }) }).first()
  await expect(linked).toBeVisible()
  for (const dark of [false, true]) {
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    const metrics = linked.getByRole('group', { name: 'Linked Validator metrics' })
    const cells = metrics.locator(':scope > div')
    await expect(cells).toHaveCount(6)
    await expect(cells.locator(':scope > span')).toHaveText(['Cumulative blocks', 'Cumulative rewards', 'Network rank', 'Production rate', 'PlatScan 24h rate', 'Delegation reward share'])
    const geometry = await cells.evaluateAll(elements => elements.map(el => {
      const label = el.querySelector('span')!
      const value = el.querySelector('strong')!
      const rect = el.getBoundingClientRect()
      return { left: rect.left, right: rect.right, top: rect.top,
        labelBottom: label.getBoundingClientRect().bottom, valueTop: value.getBoundingClientRect().top,
        wordBreak: getComputedStyle(label).wordBreak, overflowWrap: getComputedStyle(label).overflowWrap,
        overflow: el.scrollWidth > el.clientWidth }
    }))
    for (const cell of geometry) {
      expect(cell.valueTop).toBeGreaterThanOrEqual(cell.labelBottom)
      expect(cell.wordBreak).toBe('normal')
      expect(cell.overflowWrap).toBe('normal')
      expect(cell.overflow).toBe(false)
    }
    for (let index = 0; index < 6; index += 2) {
      expect(geometry[index + 1].top).toBeCloseTo(geometry[index].top, 1)
      expect(geometry[index + 1].left).toBeGreaterThan(geometry[index].right)
    }
    await expect(linked.getByRole('status', { name: 'Validator Provider data state' })).toBeVisible()
    await linked.getByRole('button', { name: 'Copy full Validator identifier' }).click()
    await expect(linked.getByRole('status', { name: 'Identifier copy status' })).toHaveText(/Validator identifier copied.|Copy failed./)
    await expect(page).toHaveURL(/\/$/)
    await linked.getByRole('button', { name: 'Open Validator details' }).click()
    const dialog = page.getByRole('dialog', { name: 'Validator details', exact: true })
    await expect(dialog.getByLabel('Full Validator identifier', { exact: true })).not.toHaveValue('')
    await expect(dialog.getByRole('region', { name: 'Linked Validator', exact: true })).toBeVisible()
    await expect(dialog.getByLabel('Public Validator states')).toBeVisible()
    await expectNoHorizontalOverflow(page)
    if (process.env.EMERALD_EVIDENCE === '1') await page.screenshot({ path: testInfo.outputPath(dark ? 'validator-details-dark.png' : 'validator-details-light.png'), fullPage: true })
    await dialog.getByRole('button', { name: 'Close', exact: true }).click()
  }
})

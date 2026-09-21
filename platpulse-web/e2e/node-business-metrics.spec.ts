import { expect, test } from '@playwright/test'
import type { PublicNetwork } from '../src/api/generated'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

/**
 * The Home Node card now keeps one emphasized label-over-value form only for
 * the cumulative metrics and uses a compact left-label/right-value key-value
 * line for ordinary parameters. Whether a group is two columns or one
 * full-width line per metric is decided by the card's own width through a CSS
 * container query, so the same DOM is measured here at forced card widths.
 */
test('business metrics preserve full values, compact rows and neutral roles', async ({ page }, testInfo) => {
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
        const layout = await metrics.evaluate(el => {
          const rows = [...el.querySelectorAll('[data-slot="metric-row"]')].map(row => {
            const label = row.querySelector('[data-slot="metric-row-label"]')!
            const value = row.querySelector('[data-slot="metric-row-value"]')!
            const range = document.createRange()
            range.selectNodeContents(value)
            const rect = row.getBoundingClientRect()
            const labelBox = label.getBoundingClientRect()
            const textBox = range.getBoundingClientRect()
            return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom,
              textTop: textBox.top, textRight: textBox.right, textLeft: textBox.left,
              labelTop: labelBox.top, labelBottom: labelBox.bottom, labelRight: labelBox.right,
              textLines: range.getClientRects().length, fontSize: getComputedStyle(value).fontSize,
              overflow: row.scrollWidth > row.clientWidth }
          })
          const counts = el.querySelector('[data-slot="node-counts"]')!
          return {
            rows,
            columns: getComputedStyle(el).gridTemplateColumns.split(' ').length,
            countsColumns: getComputedStyle(counts).gridTemplateColumns.split(' ').length,
          }
        })
        for (const row of layout.rows) {
          // A compact key-value line: one line, right-aligned, label at the left.
          expect(row.textLines, JSON.stringify({ width, i, layout })).toBe(1)
          expect(row.overflow, JSON.stringify({ width, i, layout })).toBe(false)
          expect(row.textRight).toBeLessThanOrEqual(row.right + 1)
          expect(row.textLeft).toBeGreaterThanOrEqual(row.left - 1)
          expect(row.fontSize).toBe('13px')
          expect(Math.abs(row.textTop - row.labelTop)).toBeLessThanOrEqual(4)
          expect(row.textLeft).toBeGreaterThan(row.labelRight)
        }
        const [head, qc, locked, committed, txs, peers] = layout.rows
        if (layout.columns === 1) {
          // A narrow card (or an over-long safe integer) gives each of the four
          // chain metrics its own full-width line; nothing is split or clipped.
          for (let j = 1; j < 4; j++) {
            expect(layout.rows[j].left).toBeCloseTo(layout.rows[0].left, 1)
            expect(layout.rows[j].right).toBeCloseTo(layout.rows[0].right, 1)
            expect(layout.rows[j].top).toBeGreaterThanOrEqual(layout.rows[j - 1].bottom)
          }
        } else {
          expect(qc.top).toBeCloseTo(head.top, 1)
          expect(qc.left).toBeGreaterThan(head.right)
          expect(locked.left).toBeCloseTo(head.left, 1)
          expect(committed.top).toBeCloseTo(locked.top, 1)
          expect(committed.left).toBeCloseTo(qc.left, 1)
          expect(locked.top).toBeGreaterThan(head.bottom)
        }
        expect(txs.top).toBeGreaterThanOrEqual(committed.bottom)
        if (layout.countsColumns === 1) {
          expect(peers.top).toBeGreaterThanOrEqual(txs.bottom)
          expect(peers.left).toBeCloseTo(txs.left, 1)
        } else {
          expect(peers.top).toBeCloseTo(txs.top, 1)
          expect(peers.left).toBeGreaterThan(txs.right)
          expect(Math.abs(peers.textTop - txs.textTop)).toBeLessThanOrEqual(4)
        }
        await expect(card.locator('[data-slot="node-business-metrics"] [data-short-label]')).toHaveCount(0)
        expect(await badge.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true)
      }
    }
    // Restore actual responsive widths before page-overflow checks/evidence.
    await cards.evaluateAll(els => els.forEach(el => { (el as HTMLElement).style.removeProperty('width') }))
    await expectNoHorizontalOverflow(page)
    if (process.env.EMERALD_EVIDENCE === '1') await page.screenshot({ path: testInfo.outputPath(dark ? 'business-dark.png' : 'business-light.png'), fullPage: true })
  }
})

test('Linked Validator keeps two emphasized cells and four compact parameter rows', async ({ page }, testInfo) => {
  await loginAs(page)
  const linked = page.locator('[data-slot="node-card"] [data-slot="linked-validator"]').filter({ has: page.getByRole('group', { name: 'Linked Validator metrics' }) }).first()
  await expect(linked).toBeVisible()
  for (const dark of [false, true]) {
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    const metrics = linked.getByRole('group', { name: 'Linked Validator metrics' })
    // Cumulative blocks and rewards keep the emphasized label-over-value cell.
    const emphasized = metrics.locator(':scope > [data-slot="validator-metric"]')
    await expect(emphasized).toHaveCount(2)
    await expect(emphasized.locator(':scope > span')).toHaveText(['Cumulative blocks', 'Cumulative rewards'])
    const emphasizedGeometry = await emphasized.evaluateAll(elements => elements.map(el => {
      const label = el.querySelector('span')!
      const value = el.querySelector('strong')!
      return {
        valueTop: value.getBoundingClientRect().top,
        labelBottom: label.getBoundingClientRect().bottom,
        fontWeight: getComputedStyle(value).fontWeight,
        overflow: el.scrollWidth > el.clientWidth,
      }
    }))
    for (const cell of emphasizedGeometry) {
      expect(cell.valueTop).toBeGreaterThanOrEqual(cell.labelBottom - 1)
      expect(cell.fontWeight).toBe('600')
      expect(cell.overflow).toBe(false)
    }

    // The other four are compact key-value rows that always keep their full
    // field name available, even when the card paints the short name.
    const compact = metrics.locator(':scope > [data-slot="metric-row"]')
    await expect(compact).toHaveCount(4)
    const fullNames = await metrics.locator('[data-full-label]').evaluateAll(elements => elements.map(el => el.textContent))
    expect(fullNames).toEqual(['Network rank', 'Production rate', 'PlatScan 24h rate', 'Delegation reward share'])
    const shortNames = await metrics.locator('[data-short-label]').evaluateAll(elements => elements.map(el => el.getAttribute('aria-label')))
    expect(shortNames).toEqual(fullNames)

    const rows = await metrics.locator(':scope > [data-slot="metric-row"]').evaluateAll(elements => elements.map(row => {
      const label = row.querySelector('[data-slot="metric-row-label"]')!
      const value = row.querySelector('[data-slot="metric-row-value"]')!
      const rect = row.getBoundingClientRect()
      const labelBox = label.getBoundingClientRect()
      const valueBox = value.getBoundingClientRect()
      const lineHeight = parseFloat(getComputedStyle(value).lineHeight) || 22
      return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom,
        valueTop: valueBox.top, valueRight: valueBox.right, labelTop: labelBox.top,
        lines: Math.max(1, Math.round(valueBox.height / lineHeight)), overflow: row.scrollWidth > row.clientWidth }
    }))
    for (const row of rows) {
      // A compact key-value line: one line, right-aligned against the row edge.
      expect(row.lines, JSON.stringify(rows)).toBe(1)
      expect(row.overflow, JSON.stringify(rows)).toBe(false)
      expect(Math.abs(row.valueTop - row.labelTop)).toBeLessThanOrEqual(4)
      expect(Math.abs(row.valueRight - row.right)).toBeLessThanOrEqual(1.5)
    }
    const paired = Math.abs(rows[1].top - rows[0].top) <= 1
    if (paired) {
      // Wide card: rank/production and PlatScan/delegation form two rows of two.
      expect(rows[1].left).toBeGreaterThan(rows[0].right)
      expect(rows[3].top).toBeCloseTo(rows[2].top, 1)
      expect(rows[3].left).toBeGreaterThan(rows[2].right)
      expect(rows[2].top).toBeGreaterThan(rows[0].bottom)
    } else {
      // Narrow card: four full-width key-value lines, all default-visible.
      for (let index = 1; index < 4; index++) {
        expect(rows[index].left).toBeCloseTo(rows[0].left, 1)
        expect(rows[index].right).toBeCloseTo(rows[0].right, 1)
        expect(rows[index].top).toBeGreaterThanOrEqual(rows[index - 1].bottom)
      }
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

import { expect, test } from '@playwright/test'
import type { PublicNetwork } from '../src/api/generated'
import { expectMetricRowsAligned, expectNoHorizontalOverflow, expectResourceColumnsAligned, homeNodeColumns, loginAs } from './helpers'

/**
 * The Home Node card keeps one compact key-value line for the current Block
 * Height (#237) and reads CPU, Memory and Disk as three resource columns whose
 * label and value share a line while they fit, with one shared row of Emerald
 * thin bars underneath. The card's own width decides whether a value can share
 * its label's line, so the same DOM is measured at forced card widths instead
 * of relying on a page breakpoint.
 */
test('the chain block keeps one full-value Head line and three resource columns', async ({ page }, testInfo) => {
  await loginAs(page)
  // Three distinct canonical Activity values exercise the badge's named states
  // alongside the compact metric lines (ADR 0006 replaced the consensus badge).
  const activities = ['producing', 'verifying', 'observing']
  // Each card reads its own resource fields. The Host storage percentage differs
  // from every Disk reading, so a swapped source would be visible here, and the
  // third card has no attested ratio at all.
  const resourceFixtures = [
    { processCpuPercent: 12.5, processMemoryPercent: 45.25, nodeDataDirectorySizeBytes: 12_884_901_888, nodeDataDirectoryCapacityBytes: 51_539_607_552, hostStoragePercent: 80 },
    { processCpuPercent: 0, processMemoryPercent: 0, nodeDataDirectorySizeBytes: 0, nodeDataDirectoryCapacityBytes: 51_539_607_552, hostStoragePercent: 12.5 },
    { processCpuPercent: null, processMemoryPercent: null, nodeDataDirectorySizeBytes: null, nodeDataDirectoryCapacityBytes: null, hostStoragePercent: null },
  ]
  const expectedResourceValues = [['12.5%', '45.3%', '25.0%'], ['0.0%', '0.0%', '0.0%'], ['Unknown', 'Unknown', 'Unknown']]
  const expectedBarValues = [['13', '45', '25'], ['0', '0', '0'], [null, null, null]]
  await page.route('**/api/public/v1/networks*', async route => {
    const response = await route.fetch()
    const [network]: PublicNetwork[] = await response.json()
    const base = network.nodes[0]
    const nodes = activities.map((activity, i) => ({
      ...base, ...resourceFixtures[i], nodeId: 'business-' + i, displayName: 'Node ' + i + ' Extremely Long Production Node Name That Must Not Squeeze The Role',
      // The first card carries the spec's own 11-character example height, so the
      // tightest track is exercised with the longest ordinary chain value.
      currentHead: i === 0 ? 9007199254740991 : 159320291,
      latestBlockTransactionCount: i === 0 ? 9007199254740991 : 12,
      peers: { ...base.peers, peerCount: i === 0 ? 9007199254740991 : i === 1 ? 0 : null },
      validator: base.validator ? { ...base.validator, activity, activityState: 'current' } : base.validator,
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
        const badge = card.locator('[data-slot="card-x-header"] [data-slot="validator-activity"]')
        await expect(badge).toHaveAttribute('data-activity', activities[i])
        await expect(badge.locator('[data-slot="validator-activity-label"]')).toHaveText(['Producing', 'Verifying', 'Observing'][i])

        // Chain: one compact key-value line carrying the current Block Height alone.
        const chain = card.locator('[data-slot="node-business-metrics"]')
        await expect(chain.locator('[data-slot="metric-row-label"]')).toHaveText(['Head'])
        await expect(chain.locator('[data-slot="metric-row-value"]')).toHaveText(i === 0 ? '9,007,199,254,740,991' : '159,320,291')
        for (const removed of ['QC', 'Locked', 'Committed', 'Txs']) {
          await expect(card.getByText(removed, { exact: true })).toHaveCount(0)
        }
        const chainLine = await chain.locator('[data-slot="metric-row-value"]').evaluate(el => {
          const range = document.createRange()
          range.selectNodeContents(el)
          const box = el.getBoundingClientRect()
          const row = (el.closest('[data-slot="metric-row"]') as HTMLElement).getBoundingClientRect()
          return { lines: range.getClientRects().length, right: box.right, rowRight: row.right, left: box.left, labelRight: (el.parentElement!.querySelector('[data-slot="metric-row-label"]') as HTMLElement).getBoundingClientRect().right, fontSize: getComputedStyle(el).fontSize, overflow: el.scrollWidth > el.clientWidth + 1 }
        })
        // An 11-character height stays one unclipped, right-aligned line as well.
        expect(chainLine.lines, JSON.stringify({ width, i })).toBe(1)
        expect(chainLine.overflow, JSON.stringify({ width, i })).toBe(false)
        expect(Math.abs(chainLine.right - chainLine.rowRight), JSON.stringify({ width, i })).toBeLessThanOrEqual(1.5)
        expect(chainLine.left).toBeGreaterThan(chainLine.labelRight)
        expect(chainLine.fontSize).toBe('13px')

        // Resources: three columns, each reading its own source, value and bar.
        const resources = card.locator('[data-slot="node-resource-region"]')
        await expect(resources.locator('[data-slot="resource-label"]')).toHaveText(['CPU', 'Memory', 'Disk'])
        await expect(resources.locator('[data-slot="resource-value"]')).toHaveText(expectedResourceValues[i])
        await expectMetricRowsAligned(card)
        await expectResourceColumnsAligned(card)
        // The Disk bar names the Node data directory and the capacity that holds
        // it; the Host's own storage percentage never becomes the Disk reading.
        await expect(resources.getByRole('progressbar', { name: /^Disk: Node data directory size as a share of the filesystem capacity/ })).toHaveCount(1)
        const bars = await resources.locator('[role="progressbar"]').evaluateAll(elements => elements.map(el => {
          const box = el.getBoundingClientRect()
          return { top: box.top, width: box.width, valuenow: el.getAttribute('aria-valuenow') }
        }))
        expect(bars).toHaveLength(3)
        // One shared bar row: the three tracks start on the same line, each spans
        // its own column, and an unattested ratio publishes no value at all.
        expect(Math.max(...bars.map(bar => bar.top)) - Math.min(...bars.map(bar => bar.top)), JSON.stringify({ width, i })).toBeLessThanOrEqual(1)
        expect(bars[0].width).toBeCloseTo(bars[1].width, 0)
        expect(bars.map(bar => bar.valuenow)).toEqual(expectedBarValues[i])
        // An attested percentage always shares its label's line; only a value too
        // wide for its column may take its own right-aligned line (never overflow).
        const sameLine = await resources.locator('[data-slot="resource-cell"]').evaluateAll(cells => cells.map(cell => {
          const label = (cell.querySelector('[data-slot="resource-label"]') as HTMLElement).getBoundingClientRect()
          const value = (cell.querySelector('[data-slot="resource-value"]') as HTMLElement).getBoundingClientRect()
          return Math.abs(label.top - value.top) <= 8
        }))
        if (i < 2) {
          // The attested percentages share their labels' lines. CPU and Disk do so
          // at every width and font; the longest label (Memory) can only share the
          // narrowest track while the card's own font leaves room, so it is
          // asserted strictly on a wide card and tolerated below it — the alignment
          // helper still proves the value stays unclipped and right-aligned.
          expect([sameLine[0], sameLine[2]], `CPU and Disk share their labels' lines at ${width}px`).toEqual([true, true])
          if (width === 560) expect(sameLine[1], 'Memory shares its label line on a wide card').toBe(true)
        } else expect(sameLine.filter(Boolean).length, JSON.stringify({ width, i, sameLine })).toBeGreaterThanOrEqual(2)
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

    // The Home card no longer carries the association identity, the identifier,
    // a copy control or a Details row: those live on Node detail now.
    await expect(linked.getByRole('status', { name: 'Validator Provider data state' })).toHaveCount(0)
    await expect(linked.getByRole('button', { name: 'Copy full Validator identifier' })).toHaveCount(0)
    await expect(linked.getByRole('button', { name: 'Open Validator details' })).toHaveCount(0)
    await expect(page).toHaveURL(/\/$/)
    await expectNoHorizontalOverflow(page)
    if (process.env.EMERALD_EVIDENCE === '1') await page.screenshot({ path: testInfo.outputPath(dark ? 'validator-card-dark.png' : 'validator-card-light.png'), fullPage: true })
  }
})

/**
 * Density regression for the auto-fill minimum track. The grid minimum is
 * Emerald's own 300px (HomeView.vue's `minmax(300px, 1fr)`), so a desktop card
 * is about 303px. The chain block is one Head line at every card width (#237),
 * the three resource columns keep three bars on one shared row, and the
 * Validator group keeps the one/two-column pairing its own 22.5rem width switch
 * allows. The assertions read geometry, not the CSS rule, and adapt to every
 * fixed project instead of hardcoding a desktop column count.
 */
test('auto-filled cards keep one chain line, one bar row and unclipped cells', async ({ page }) => {
  await loginAs(page)
  const grid = page.locator('[data-slot="node-grid"]')
  const cards = grid.locator('[data-slot="node-card"]')
  await expect(cards.first()).toBeVisible()
  const gridMath = await grid.evaluate(el => {
    const style = getComputedStyle(el)
    return { width: el.getBoundingClientRect().width, gap: parseFloat(style.columnGap), columns: style.gridTemplateColumns.split(' ').length }
  })
  expect(gridMath.columns).toBe(homeNodeColumns(gridMath.width, gridMath.gap))

  // Count visually distinct rows rather than grid tracks: a narrow card keeps
  // the two cumulative cells side by side while a wide card pairs four
  // parameters into two rows, so track counts alone cannot describe it.
  const count = await cards.count()
  for (let index = 0; index < count; index++) {
    const cardGeometry = await cards.nth(index).evaluate(cardEl => {
      const rowCount = (el: Element, selector: string) => {
        const tops = [...el.querySelectorAll(selector)].map(item => item.getBoundingClientRect().top)
        return tops.reduce<number[]>((rows, top) => rows.some(existing => Math.abs(existing - top) <= 2) ? rows : [...rows, top], []).length
      }
      const business = cardEl.querySelector('[data-slot="node-business-metrics"]')
      const resource = cardEl.querySelector('[data-slot="node-resource-region"]')
      const validator = cardEl.querySelector('[data-slot="linked-validator-metrics"]')
      return {
        width: cardEl.getBoundingClientRect().width,
        businessRows: business ? rowCount(business, ':scope > [data-slot="metric-row"]') : 0,
        resourceCells: resource ? resource.querySelectorAll('[data-slot="resource-cell"]').length : 0,
        bars: resource ? resource.querySelectorAll('[role="progressbar"]').length : 0,
        barRows: resource ? rowCount(resource, '[role="progressbar"]') : 0,
        validatorRows: validator ? rowCount(validator, ':scope > [data-slot="metric-row"], :scope > [data-slot="validator-metric"]') : null,
      }
    })
    const expectedRows = cardGeometry.width >= 360 ? 3 : 5
    const where = ' on card ' + index + ' at ' + cardGeometry.width + 'px'
    // The chain block is one Head line at every card width, and the three
    // resource bars share one row however narrow the card gets.
    expect(cardGeometry.businessRows, 'chain rows' + where).toBe(1)
    expect(cardGeometry.resourceCells, 'resource columns' + where).toBe(3)
    expect(cardGeometry.bars, 'resource bars' + where).toBe(3)
    expect(cardGeometry.barRows, 'resource bar rows' + where).toBe(1)
    if (cardGeometry.validatorRows != null) expect(cardGeometry.validatorRows, 'Validator rows' + where).toBe(expectedRows)
    // Nothing may overflow its cell at the tightest track, including an
    // 11-character chain height and an unattested resource value.
    const overflow = await cards.nth(index)
      .locator('[data-slot="metric-row-value"], [data-slot="metric-row-label"], [data-slot="resource-value"], [data-slot="resource-label"], [data-slot="validator-metric"] strong')
      .evaluateAll(elements => elements.filter(element => element.scrollWidth > element.clientWidth + 1).map(element => element.textContent))
    expect(overflow, 'no metric label or value overflows its cell' + where).toEqual([])
  }
  await expectNoHorizontalOverflow(page)
})


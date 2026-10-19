import { expect, test } from '@playwright/test'
import type { PublicNetwork } from '../src/api/generated'
import { expectMetricRowsAligned, expectNoHorizontalOverflow, expectResourceColumns, expectResourceColumnsAligned, homeNodeColumns, loginAs } from './helpers'

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
  // Each card carries its own Observed Network Head, so one screen drives all
  // three states the evidence gate can support (#239): the reference's own
  // height, a lag inside the warning band, and a Node ahead of the reference —
  // a leading state that is never clamped to the reference or called synced. The
  // widest value at the tightest track gets the longest comparison, so a hint
  // that cannot share the height's line has to take its own right-aligned one.
  const headComparisonFixtures = [
    { reference: 9_007_199_254_740_991, label: 'At observed head', tone: 'success', toneClass: 'text-success' },
    { reference: 159_320_301, label: 'Behind 10 blocks', tone: 'warning', toneClass: 'text-warning-foreground dark:text-warning' },
    { reference: 12, label: 'Ahead 159,320,279 blocks', tone: 'ahead', toneClass: 'text-muted-foreground' },
  ]
  await page.route('**/api/public/v1/networks*', async route => {
    const response = await route.fetch()
    const [network]: PublicNetwork[] = await response.json()
    const base = network.nodes[0]
    const nodes = activities.map((activity, i) => ({
      ...base, ...resourceFixtures[i], nodeId: 'business-' + i, displayName: 'Node ' + i + ' Extremely Long Production Node Name That Must Not Squeeze The Role',
      networkReferenceHead: headComparisonFixtures[i].reference, networkReferenceConfidence: 'high',
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

        // The comparison rides beside that absolute height, right-aligned to the
        // same line's edge whether it shares the line or drops below it, on one
        // unclipped line of its own, with the tone the detail page paints.
        const hint = chain.locator('[data-slot="node-head-comparison"]')
        await expect(hint).toHaveText(headComparisonFixtures[i].label)
        await expect(hint).toHaveAttribute('data-tone', headComparisonFixtures[i].tone)
        expect(await hint.getAttribute('class')).toContain(headComparisonFixtures[i].toneClass)
        await expect(hint).toHaveAttribute('title', /Observed Network Head/)
        const hintBox = await hint.evaluate(el => {
          const box = el.getBoundingClientRect()
          const line = (el.closest('[data-slot="node-head-line"]') as HTMLElement).getBoundingClientRect()
          return { right: box.right, left: box.left, lineRight: line.right, lineLeft: line.left, overflow: el.scrollWidth > el.clientWidth + 1 }
        })
        expect(Math.abs(hintBox.right - hintBox.lineRight), JSON.stringify({ width, i })).toBeLessThanOrEqual(1.5)
        expect(hintBox.left).toBeGreaterThanOrEqual(hintBox.lineLeft - 1)
        expect(hintBox.overflow, JSON.stringify({ width, i })).toBe(false)
        // The deployment-local reference is never named a canonical, public or
        // explorer head, and a leading Node is never read as synchronized.
        await expect(card).not.toContainText(/canonical|explorer|Synced/)

        // Resources: three columns, each reading its own source, value and bar.
        const resources = card.locator('[data-slot="node-resource-region"]')
        await expectResourceColumns(card, expectedResourceValues[i])
        await expectMetricRowsAligned(card)
        await expectResourceColumnsAligned(card)
        // The Disk bar names the Node data directory and the capacity that holds
        // it; the Host's own storage percentage never becomes the Disk reading.
        await expect(resources.getByRole('progressbar', { name: 'Disk: Node data directory size as a share of the filesystem capacity holding it, not whole-filesystem fullness', exact: true })).toHaveCount(1)
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
        // Every attested percentage shares its label's line at every card width
        // and font: the three columns' shortest honest labels are what keeps that
        // true at the auto-fill grid's own 300px minimum track. An unattested word
        // may still take its own right-aligned line (never overflow, clip or
        // shrink), which is the only case the columns' fallback exists for.
        const cellsGeometry = await resources.locator('[data-slot="resource-cell"]').evaluateAll(cells => cells.map(cell => {
          const box = cell.getBoundingClientRect()
          const labelEl = cell.querySelector('[data-slot="resource-label"]') as HTMLElement
          const valueEl = cell.querySelector('[data-slot="resource-value"]') as HTMLElement
          const label = labelEl.getBoundingClientRect()
          const value = valueEl.getBoundingClientRect()
          return {
            sameLine: Math.abs(label.top - value.top) <= 8,
            belowLabel: value.top >= label.bottom - 1,
            rightAligned: Math.abs(value.right - box.right) <= 1.5,
            inside: value.left >= box.left - 1 && value.right <= box.right + 1,
            overflow: valueEl.scrollWidth > valueEl.clientWidth + 1,
            fontSize: getComputedStyle(valueEl).fontSize,
          }
        }))
        const sameLine = cellsGeometry.map(cell => cell.sameLine)
        if (i < 2) expect(sameLine, `the attested percentages share their labels' lines at ${width}px`).toEqual([true, true, true])
        // How many unattested words fit beside their labels depends on the
        // reader's font; wherever one does not, it takes its own line under the
        // label, flush right, whole and at the same size.
        for (const cell of cellsGeometry) {
          const where = JSON.stringify({ width, i, cell })
          if (!cell.sameLine) expect(cell.belowLabel, where).toBe(true)
          expect(cell.rightAligned, where).toBe(true)
          expect(cell.inside, where).toBe(true)
          expect(cell.overflow, where).toBe(false)
          expect(cell.fontSize, where).toBe('13px')
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

  // The comparison adds no control of its own, so Node navigation keeps its one
  // link per card; and its boundary stays in the card's own explanation entry,
  // reached by keyboard and, on a touch project, by a real tap (#239).
  const first = cards.nth(0)
  await expect(first.locator('a[href="/nodes/business-0"]')).toHaveCount(1)
  await expect(first.locator('[data-slot="node-head-line"] a, [data-slot="node-head-line"] button')).toHaveCount(0)
  await expect(first.locator('[data-slot="node-head-retained"]')).toHaveCount(0)
  const identity = first.getByRole('button', { name: 'Node identity details' })
  const explanation = page.getByRole('dialog', { name: /Node 0/ })
  await identity.focus()
  await page.keyboard.press('Enter')
  await expect(explanation).toBeVisible()
  await expect(explanation).toContainText('Shown here: At observed head')
  await expect(explanation).toContainText('Observed Network Head')
  await expect(explanation).toContainText('not the public chain head')
  await expect(explanation).toContainText('never a health, alert or Attention rule')
  await expect(explanation).not.toContainText(/canonical|explorer/i)
  await page.keyboard.press('Escape')
  await expect(explanation).toBeHidden()
  await expect(identity).toBeFocused()
  if (testInfo.project.use.hasTouch) await identity.tap()
  else await identity.click()
  await expect(explanation).toBeVisible()
  await explanation.getByRole('button', { name: 'Close', exact: true }).click()
  await expect(explanation).toBeHidden()
})

test('Linked Validator keeps the three summary cells a confirmed Validator owns', async ({ page }, testInfo) => {
  await loginAs(page)
  const linked = page.locator('[data-slot="node-card"] [data-slot="linked-validator"]').filter({ has: page.getByRole('group', { name: 'Linked Validator metrics' }) }).first()
  await expect(linked).toBeVisible()
  for (const dark of [false, true]) {
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    const metrics = linked.getByRole('group', { name: 'Linked Validator metrics' })
    // The three summary cells are one content-driven row of label-over-value
    // cells: the short name is what the cell paints, while the full metric name
    // stays its accessible name and title (#238).
    const cells = metrics.locator(':scope > [data-slot="validator-summary-cell"]')
    await expect(cells).toHaveCount(3)
    const fullNames = await metrics.locator('[data-full-label]').evaluateAll(elements => elements.map(el => el.textContent))
    expect(fullNames).toEqual(['Network rank', 'Cumulative blocks', 'Cumulative rewards'])
    const shortNames = await metrics.locator('[data-short-label]').evaluateAll(elements => elements.map(el => el.getAttribute('aria-label')))
    expect(shortNames).toEqual(fullNames)
    const geometry = await cells.evaluateAll(elements => elements.map(el => {
      const label = el.querySelector('[data-slot="validator-summary-label"]')!
      const value = el.querySelector('[data-slot="validator-summary-value"]')!
      const box = el.getBoundingClientRect()
      return {
        left: box.left, right: box.right, top: box.top, bottom: box.bottom,
        valueTop: value.getBoundingClientRect().top,
        labelBottom: label.getBoundingClientRect().bottom,
        fontWeight: getComputedStyle(value).fontWeight,
        overflow: value.scrollWidth > value.clientWidth + 1 || label.scrollWidth > label.clientWidth + 1,
      }
    }))
    for (const cell of geometry) {
      // Each value sits directly beneath its own label, at the emphasized weight.
      expect(cell.valueTop).toBeGreaterThanOrEqual(cell.labelBottom - 1)
      expect(cell.fontWeight).toBe('500')
      expect(cell.overflow).toBe(false)
    }
    // Two columns while the card is narrow, three once the card's own width
    // genuinely holds them, and the third cell keeps the whole row width
    // whenever it cannot.
    const columns = await metrics.evaluate(el => getComputedStyle(el).gridTemplateColumns.split(' ').length)
    expect(columns, 'the summary uses two or three columns').toBeLessThanOrEqual(3)
    if (columns === 3) {
      expect(geometry[1].left).toBeGreaterThan(geometry[0].right)
      expect(geometry[2].left).toBeGreaterThan(geometry[1].right)
      expect(geometry[2].top).toBeCloseTo(geometry[0].top, 1)
    } else {
      expect(geometry[1].left).toBeGreaterThan(geometry[0].right)
      expect(geometry[2].top).toBeGreaterThan(geometry[0].bottom)
      expect(geometry[2].left).toBeCloseTo(geometry[0].left, 0)
      expect(geometry[2].right).toBeCloseTo(geometry[1].right, 0)
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
 * Density regression for the auto-fill minimum track. The grid minimum is the
 * card's own 22.5rem content switch (#238), so a desktop card fills a 408px
 * track, a tablet card a 362px one. The chain block is one Head line at every
 * card width (#237), the three resource columns keep three bars on one shared
 * row, and the Validator summary keeps the two or three columns that switch
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
        businessRows: business ? rowCount(business, '[data-slot="node-head-line"] > [data-slot="metric-row"]') : 0,
        resourceCells: resource ? resource.querySelectorAll('[data-slot="resource-cell"]').length : 0,
        bars: resource ? resource.querySelectorAll('[role="progressbar"]').length : 0,
        barRows: resource ? rowCount(resource, '[role="progressbar"]') : 0,
        validatorRows: validator ? rowCount(validator, ':scope > [data-slot="validator-summary-cell"]') : null,
      }
    })
    // Three summary columns need the card's own 25rem switch; below it the two
    // columns and their full-width third cell make two rows.
    const expectedRows = cardGeometry.width >= 400 ? 1 : 2
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
      .locator('[data-slot="metric-row-value"], [data-slot="metric-row-label"], [data-slot="node-head-comparison"], [data-slot="resource-value"], [data-slot="resource-label"], [data-slot="validator-summary-value"], [data-slot="validator-summary-label"]')
      .evaluateAll(elements => elements.filter(element => element.scrollWidth > element.clientWidth + 1).map(element => element.textContent))
    expect(overflow, 'no metric label or value overflows its cell' + where).toEqual([])
  }
  await expectNoHorizontalOverflow(page)
})


import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test'
import type { PublicNetwork } from '../src/api/generated'
import { closeHomeSurface, expectNoHorizontalOverflow, homeNodeColumns, loginAs, openHomeSort } from './helpers'

const labels = ['Active Nodes', 'Healthy Nodes', 'Cumulative blocks', 'Attention', 'Networks', 'Cumulative rewards']

async function box(locator: Locator) {
  const result = await locator.boundingBox()
  expect(result).not.toBeNull()
  return result!
}

async function overviewGeometry(page: Page) {
  const width = page.viewportSize()!.width
  const home = page.getByRole('region', { name: 'Home', exact: true })
  const summary = page.locator('[aria-label="Home summary"]')
  const cards = summary.locator('[data-slot="summary-card"]')
  await expect(cards).toHaveCount(6)
  expect(await cards.evaluateAll(elements => elements.map(el => el.getAttribute('aria-label')))).toEqual(labels)
  const columns = width >= 640 ? 3 : 2
  const boxes = await Promise.all(labels.map(label => box(summary.getByRole('article', { name: label, exact: true }))))
  for (let index = 0; index < boxes.length; index++) {
    const current = boxes[index]
    expect(current.width).toBeCloseTo(boxes[0].width, 0)
    expect(current.height).toBeCloseTo(boxes[0].height, 0)
    expect(current.x).toBeCloseTo(boxes[index % columns].x, 0)
    expect(current.y).toBeCloseTo(boxes[Math.floor(index / columns) * columns].y, 0)
    if (index % columns) expect(current.x).toBeGreaterThanOrEqual(boxes[index - 1].x + boxes[index - 1].width)
    if (index >= columns) expect(current.y).toBeGreaterThanOrEqual(boxes[index - columns].y + boxes[index - columns].height)
  }
  // Every tile starts its value directly under the uniform 44px header, so a
  // caption that wraps on the two cumulative tiles cannot push their values off
  // the shared baseline. Compare each value's offset from its own card top, so
  // the per-row page offset is not mistaken for a baseline difference.
  const valueOffsets = await summary.locator('[data-slot="summary-card"]').evaluateAll(cards => cards.map(card => {
    const value = card.querySelector('[data-slot="summary-value"]')
    return value ? value.getBoundingClientRect().top - card.getBoundingClientRect().top : Number.NaN
  }))
  for (const offset of valueOffsets) expect(Math.abs(offset - valueOffsets[0])).toBeLessThanOrEqual(1)
  // #240: the trailing marker or control is one shared slot in every header, so
  // the four metric icons and the two Breakdown controls occupy the same column
  // on the title's own line with the same 44px target, instead of each kind
  // hanging at its own offset. Measured against each card's own top/right, so
  // the per-row page offset is not mistaken for an alignment difference.
  const markers = await summary.locator('[data-slot="summary-card"]').evaluateAll(cards => cards.map(card => {
    const box = card.getBoundingClientRect()
    const marker = card.querySelector('[data-slot="summary-marker"]')
    const title = card.querySelector('[data-slot="card-x-content"] > div:first-child > span')
    const glyph = marker?.querySelector('svg')
    if (!marker || !title || !glyph) return null
    const markerBox = marker.getBoundingClientRect()
    const glyphBox = glyph.getBoundingClientRect()
    // The title's own first line, not its box: a title that wraps in a narrow
    // tile still hangs the marker on its first line, so a two-line title's box
    // centre is not the measurement that matters.
    const line = document.createRange()
    line.selectNodeContents(title)
    const lineBox = line.getClientRects()[0]
    if (!lineBox) return null
    return {
      title: title.textContent,
      width: markerBox.width,
      height: markerBox.height,
      markerRight: box.right - markerBox.right,
      titleLeft: title.getBoundingClientRect().left - box.left,
      glyphCenter: glyphBox.top + glyphBox.height / 2 - box.top,
      titleCenter: lineBox.top + lineBox.height / 2 - box.top,
    }
  }))
  expect(markers.every(marker => marker !== null)).toBe(true)
  const fitted = markers as Array<NonNullable<(typeof markers)[number]>>
  expect(fitted.map(marker => marker.title)).toEqual(labels)
  for (const marker of fitted) {
    expect(marker.width, 'every tile reserves the same marker target').toBeGreaterThanOrEqual(44)
    expect(marker.height).toBeGreaterThanOrEqual(44)
    // The six titles therefore share one left edge, one marker column, one
    // marker target and one first-line height, whichever tile kind they are.
    for (const measurement of ['width', 'height', 'markerRight', 'titleLeft', 'glyphCenter', 'titleCenter'] as const) {
      expect(Math.abs(marker[measurement] - fitted[0][measurement]), measurement).toBeLessThanOrEqual(1)
    }
    // The marker hangs on the title's own line, not a dozen pixels below it.
    expect(Math.abs(marker.glyphCenter - marker.titleCenter), 'marker and title share one line').toBeLessThanOrEqual(2)
  }
  const summaryBox = await box(summary)
  const mapBox = await box(page.locator('[data-slot="home-map"]'))
  if (width >= 1024) {
    expect(mapBox.x).toBeGreaterThanOrEqual(summaryBox.x + summaryBox.width)
    // The map owns the larger half of the band, matching the Emerald reference.
    expect(mapBox.width / summaryBox.width).toBeCloseTo(6 / 5, 1)
    expect(mapBox.y + mapBox.height).toBeGreaterThan(summaryBox.y)
    expect(summaryBox.y + summaryBox.height).toBeGreaterThan(mapBox.y)
  } else {
    // Narrow layouts place the map first and stack the six statistics below it.
    expect(summaryBox.y).toBeGreaterThanOrEqual(mapBox.y + mapBox.height)
  }
  // Check the painted chart container as well as its grid track: a translated
  // canvas must not visually drift back over the summary or Node controls.
  const chart = page.locator('[data-slot="geo-chart"]')
  await expect(chart).toBeVisible()
  const chartBox = await box(chart)
  expect(chartBox.x).toBeGreaterThanOrEqual(mapBox.x - 1)
  expect(chartBox.y).toBeGreaterThanOrEqual(mapBox.y - 1)
  expect(chartBox.x + chartBox.width).toBeLessThanOrEqual(mapBox.x + mapBox.width + 1)
  expect(chartBox.y + chartBox.height).toBeLessThanOrEqual(mapBox.y + mapBox.height + 1)
  const controls = await box(page.locator('[aria-label="Node filters and sorting"]'))
  expect(controls.y).toBeGreaterThanOrEqual(chartBox.y + chartBox.height)
  const homeBox = await box(home)
  expect(homeBox.width).toBeLessThanOrEqual(1280)
  if (width >= 1280) {
    expect(homeBox.width).toBeCloseTo(1280, 0)
    expect(homeBox.x).toBeCloseTo((width - 1280) / 2, 0)
  }
  const grid = page.locator('[data-slot="node-grid"]')
  const gridBox = await box(grid)
  const geometry = await grid.evaluate(el => {
    const style = getComputedStyle(el)
    return { columns: style.gridTemplateColumns.split(' ').length, gap: parseFloat(style.columnGap) }
  })
  // The auto-fill minimum track is the card's own 22.5rem content switch (#238),
  // so the grid resolves to floor division on the measured content width and
  // never hardcodes a column count. Home is capped at max-w-1280 (1248px of
  // content), which fits three 408px tracks at desktop: a fourth would need
  // 1488px and the AC only takes a fourth column once it genuinely fits. A phone
  // column stays one card wide, a tablet two, and no track is narrower than the
  // three-cell summary the card holds.
  const expectedColumns = homeNodeColumns(gridBox.width, geometry.gap)
  expect(geometry.columns).toBe(expectedColumns)
  const nodes = grid.locator('[data-slot="node-card"]')
  for (let index = 0; index < expectedColumns; index++) {
    const nodeBox = await box(nodes.nth(index))
    expect(nodeBox.width).toBeCloseTo((gridBox.width - geometry.gap * (expectedColumns - 1)) / expectedColumns, 0)
  }
  // The six tile titles, never the marker slot that now shares each header with
  // them (#240): a title is never broken mid-word, so the two labels that wrap
  // on a narrow tile stay whole words on their own lines.
  const labelStyles = await cards.locator('[data-slot="card-x-content"] > div:first-child > span:first-child').evaluateAll(elements => elements.map(el => ({
    wordBreak: getComputedStyle(el).wordBreak, overflowWrap: getComputedStyle(el).overflowWrap,
  })))
  expect(labelStyles).toHaveLength(6)
  for (const style of labelStyles) {
    expect(style.wordBreak).toBe('normal')
    expect(style.overflowWrap).toBe('normal')
  }
  await expectNoHorizontalOverflow(page)
}

async function evidence(page: Page, testInfo: TestInfo, name: string) {
  if (process.env.EMERALD_EVIDENCE === '1') {
    await page.screenshot({ path: testInfo.outputPath(name + '.png'), fullPage: true })
  }
}

test('six overview cards, separate map and unchanged auto-fill geometry in light and dark', async ({ page }, testInfo) => {
  await loginAs(page)
  for (const dark of [false, true]) {
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    await overviewGeometry(page)
    await evidence(page, testInfo, dark ? 'home-dark' : 'home-light')
  }
})

test('1920 desktop remains max1280 with proportional map and auto-fill Nodes', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1440', 'One extra-wide check, without changing the configured matrix')
  await page.setViewportSize({ width: 1920, height: 1080 })
  await loginAs(page)
  for (const dark of [false, true]) {
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    await overviewGeometry(page)
    await evidence(page, testInfo, dark ? 'home-1920-dark' : 'home-1920-light')
  }
})

test('long cumulative values and the longest coverage line stay inside their tiles', async ({ page }, testInfo) => {
  await loginAs(page)
  // The widest the two cumulative tiles ever get (#240): a 20-digit block count,
  // the longest coverage line the face can print (a missing Network summary
  // beside a partial denominator and a stale contributor), and the abbreviated
  // reward form, all measured at every fixed viewport in both themes.
  await page.route('**/api/public/v1/networks*', async route => {
    const response = await route.fetch()
    const networks: PublicNetwork[] = await response.json()
    await route.fulfill({ response, json: networks.map((network, index) => index === 0 ? {
      ...network,
      validatorSummary: {
        ...network.validatorSummary,
        blocks: { expectedCount: 8, knownSum: '18446744073709551615', staleCount: 1, state: 'partial', valuedCount: 5 },
        rewards: { expectedCount: 8, knownSum: '123456789012345678.123456789012345678', staleCount: 1, state: 'partial', valuedCount: 5 },
      },
    } : { ...network, validatorSummary: null }) })
  })
  await page.goto('/')
  const blocks = page.getByRole('article', { name: 'Cumulative blocks', exact: true })
  const rewards = page.getByRole('article', { name: 'Cumulative rewards', exact: true })
  await expect(blocks.locator('[data-slot="summary-value"]')).toHaveText('18,446,744,073,709,551,615')
  await expect(blocks.locator('[data-slot="summary-caption"]'))
    .toHaveText('2 Networks · 5/8 reporting · 1 Network summary missing · 1 stale')
  await expect(rewards.locator('[data-slot="summary-caption"]'))
    .toHaveText('2 Networks · 5/8 reporting · 1 Network summary missing · 1 stale')
  await expect(rewards.locator('[data-slot="summary-value"]')).not.toHaveText('Unknown')
  for (const dark of [false, true]) {
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    // No value and no coverage line is clipped: the value wraps at its own
    // separators and the coverage line keeps its own wrapped rows.
    const clipped = await page.locator('[data-slot="summary-card"] [data-slot="summary-value"], [data-slot="summary-card"] [data-slot="summary-caption"]')
      .evaluateAll(elements => elements.filter(el => el.scrollWidth > el.clientWidth + 1).map(el => el.textContent))
    expect(clipped, 'a long cumulative value wraps inside its tile').toEqual([])
    // Nor do they leave the shared surface they belong to.
    const escaped = await page.locator('[data-slot="summary-card"]').evaluateAll(cards => cards.flatMap(card => {
      const box = card.getBoundingClientRect()
      return [...card.querySelectorAll('[data-slot="summary-value"], [data-slot="summary-caption"]')]
        .filter(el => { const rect = el.getBoundingClientRect(); return rect.right > box.right + 1 || rect.bottom > box.bottom + 1 })
        .map(el => el.textContent)
    }))
    expect(escaped, 'no value or coverage line leaves its tile').toEqual([])
    // The long text must not cost the six tiles their shared geometry, and the
    // complete explanation stays reachable from the tile's own control.
    await overviewGeometry(page)
    await blocks.getByRole('button', { name: 'Cumulative blocks breakdown and exact values' }).click()
    const dialog = page.getByRole('dialog')
    await expect(dialog.getByText('Exact known-value subtotal: 18,446,744,073,709,551,615', { exact: true })).toBeVisible()
    await expect(dialog.getByText('1 Network summary missing from this coverage.')).toBeVisible()
    await dialog.getByRole('button', { name: 'Close', exact: true }).click()
    await evidence(page, testInfo, dark ? 'home-long-values-dark' : 'home-long-values-light')
  }
})

test('short and long Node names retain identity actions and per-row card heights', async ({ page }, testInfo) => {
  await loginAs(page)
  const longName = 'B Very Long Production Node Name With Readable Words And A Distinct Identity'
  await page.route('**/api/public/v1/networks*', async route => {
    const response = await route.fetch()
    const [network]: PublicNetwork[] = await response.json()
    const base = network.nodes[0]
    const nodes = ['A', longName, 'C', 'D', 'E', 'F'].map((displayName, index) => ({
      ...base, nodeId: 'layout-' + index, displayName, validator: undefined, validatorIdentityReason: undefined,
      health: index === 1 ? 'unhealthy' : 'healthy',
      healthReason: index === 1 ? 'Collection failed. This deliberately longer sanitized diagnostic must wrap naturally without stretching the other Node cards to match its height.' : undefined,
    }))
    await route.fulfill({ response, json: [{ ...network, nodes }] })
  })
  await page.goto('/')
  await (await openHomeSort(page)).selectOption('name')
  // Close the surface again: the identity action clicked below lives in a card
  // the open surface would otherwise overlay (issue #232).
  await closeHomeSurface(page)
  const cards = page.locator('[data-slot="node-card"]')
  await expect(cards).toHaveCount(6)
  for (const dark of [false, true]) {
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    await overviewGeometry(page)
    // #238 removed the grid-wide equal height: cards on one row still line up
    // with their own row, while the row holding the long name and its longer
    // diagnostic grows on its own instead of stretching every other row.
    expect(await expectPerRowCardHeights(cards)).toHaveLength(6)
    await expect(cards.nth(0).getByRole('link', { name: 'A', exact: true })).toHaveAttribute('href', '/nodes/layout-0')
    await expect(cards.nth(1).getByRole('link', { name: longName, exact: true })).toHaveAttribute('href', '/nodes/layout-1')
    const linkGeometry = await cards.nth(1).getByRole('link', { name: longName, exact: true }).evaluate(el => {
      const after = getComputedStyle(el, '::after')
      return { position: after.position, inset: after.inset }
    })
    expect(linkGeometry).toEqual({ position: 'absolute', inset: '0px' })
    await cards.nth(1).getByRole('button', { name: 'Node identity details' }).click()
    const dialog = page.getByRole('dialog', { name: longName, exact: true })
    await expect(dialog).toBeVisible()
    await expect(dialog.getByText(/Node role describes the Node’s consensus membership/)).toBeVisible()
    // The identity dialog never leaves Home, and the sort the reader chose lives
    // in that ordinary Home URL (issue #222), so Home is the path rather than
    // the whole address.
    expect(new URL(page.url()).pathname).toBe('/')
    expect(new URL(page.url()).searchParams.get('sort')).toBe('name')
    await expectNoHorizontalOverflow(page)
    await dialog.getByRole('button', { name: 'Close', exact: true }).click()
    await evidence(page, testInfo, dark ? 'node-natural-dark' : 'node-natural-light')
  }
})

import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test'
import type { PublicNetwork } from '../src/api/generated'
import { expectNoHorizontalOverflow, homeNodeColumns, loginAs } from './helpers'

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
  const summaryBox = await box(summary)
  const mapBox = await box(page.locator('[data-slot="home-map"]'))
  if (width >= 1024) {
    expect(mapBox.x).toBeGreaterThanOrEqual(summaryBox.x + summaryBox.width)
    // 4:3 restores the original four-card tile width inside the six-card grid.
    expect(mapBox.width / summaryBox.width).toBeCloseTo(3 / 4, 1)
    expect(mapBox.y + mapBox.height).toBeGreaterThan(summaryBox.y)
    expect(summaryBox.y + summaryBox.height).toBeGreaterThan(mapBox.y)
  } else {
    expect(mapBox.y).toBeGreaterThanOrEqual(summaryBox.y + summaryBox.height)
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
  // The auto-fill minimum track is the same 22.5rem (360px at the default root)
  // a card's own container query needs for its two-column consensus and
  // Validator grids, so an auto-filled card is never narrower than its
  // width-driven rules require (an over-long value still falls back on purpose).
  // Four columns are exposed only when the content column can hold four of those
  // tracks; since Home is capped at max-w-1280 (1248px of content), desktop
  // yields three wide cards rather than four narrow ones. Never hardcoded.
  const expectedColumns = homeNodeColumns(gridBox.width, geometry.gap)
  expect(geometry.columns).toBe(expectedColumns)
  const nodes = grid.locator('[data-slot="node-card"]')
  for (let index = 0; index < expectedColumns; index++) {
    const nodeBox = await box(nodes.nth(index))
    expect(nodeBox.width).toBeCloseTo((gridBox.width - geometry.gap * (expectedColumns - 1)) / expectedColumns, 0)
  }
  const labelStyles = await cards.locator('[data-slot="card-x-content"] > div:first-child > span').evaluateAll(elements => elements.map(el => ({
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

test('short and long Node names retain identity actions and one shared card height', async ({ page }, testInfo) => {
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
  await page.getByLabel('Sort', { exact: true }).selectOption('name')
  const cards = page.locator('[data-slot="node-card"]')
  await expect(cards).toHaveCount(6)
  for (const dark of [false, true]) {
    await page.evaluate(value => document.documentElement.classList.toggle('dark', value), dark)
    await overviewGeometry(page)
    // The grid uses auto-rows-fr, so every card in every row shares the
    // tallest card's height — not just the two cards in one row. Six cards at
    // desktop span two rows, so a single unique height proves cross-row
    // equality, and a longer sanitized reason cannot stretch a subset.
    const cardHeights = await cards.evaluateAll(elements => elements.map(el => Math.round(el.getBoundingClientRect().height)))
    expect(cardHeights).toHaveLength(6)
    expect(new Set(cardHeights).size, 'all Node cards share one height across every row: ' + JSON.stringify(cardHeights)).toBe(1)
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
    await expect(page).toHaveURL(/\/$/)
    await expectNoHorizontalOverflow(page)
    await dialog.getByRole('button', { name: 'Close', exact: true }).click()
    await evidence(page, testInfo, dark ? 'node-natural-dark' : 'node-natural-light')
  }
})

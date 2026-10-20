import { mkdir, writeFile } from 'node:fs/promises'
import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test'
import { loginAs } from './helpers'

/**
 * Node Detail six-chart visual acceptance (this round).
 *
 * The six ECharts cards were ported to Emerald's grid, legend and palette in an
 * earlier round, and the spec that shipped with that round
 * (node-detail-six-charts.spec.ts) proves the configuration, the SVG geometry
 * and the data semantics. What it could not prove is what a reader actually
 * sees: that no axis label is painted outside its chart, that an axis title does
 * not sit on top of its own ticks, that a legend does not collide with the clock
 * labels, and that the plot box is still the size the Emerald margins promise
 * instead of silently shrinking to fit its own labels.
 *
 * This file is that acceptance. It runs a fixed fixture through the real
 * Server-fed page at the four widths the acceptance names plus the width the
 * running project itself fixes, in both themes, and reads the rendered text
 * boxes back out of the SVG. The fixture is display data only:
 * it is written into the metrics response of the throwaway e2e Server, it states
 * values the Server could really report, and no production code fills, smooths or
 * re-connects anything to draw it.
 */

const PUBLIC_NODE_NAME = 'Node A'
const PUBLIC_NODE_ID = '0195f2a1-0014-4014-8014-000000000014'

const nodeRoute = '**/api/public/v1/nodes/' + PUBLIC_NODE_ID
const metricsRoute = nodeRoute + '/metrics'

const GiB = 1024 * 1024 * 1024

const THEMES = ['light', 'dark'] as const
type Theme = (typeof THEMES)[number]

/**
 * The four widths the acceptance names. The Playwright projects already fix
 * 390 / 768 / 1440; 1920 is reached by resizing inside the test, so the same
 * assertions run at every width rather than only where a project happens to sit.
 */
const WIDTHS = [390, 768, 1440, 1920] as const

/**
 * The widths one run measures: the four above, plus the width the running
 * project itself fixes (docs/design/webui.md, "Responsive acceptance baseline":
 * 360x800, 390x844, 768x1024, 1280x800, 1440x900). Without this the phone-360
 * and desktop-1280 projects would resize themselves off their own width and
 * never measure what their own viewport shows.
 */
function measuredWidths(testInfo: TestInfo): number[] {
  const own = testInfo.project.use?.viewport?.width
  return [...new Set(own === undefined ? [...WIDTHS] : [...WIDTHS, own])].sort((left, right) => left - right)
}

/**
 * Emerald's declared chart geometry, quoted from the reference the port cites
 * (\`src/components/LoadChart.vue\`, commit c2c5e88; recorded in
 * docs/visual-migration/emerald/README.md sections 1 and 35):
 *
 *   chartMargin          { top: 30, right: 24, bottom: 32, left: 56 }
 *   chartMargin + legend { top: 30, right: 24, bottom: 52, left: 56 }
 *   axisLabel            fontSize 11, margin 12 on x and 6 on y
 *   legend               bottom 4, itemWidth/itemHeight 12, itemGap 20
 *   plot box             h-48 (192px)
 *
 * Emerald is not vendored - the reference repository is not a dependency of this
 * one - so the comparison at a given viewport width is against these recorded
 * constants, measured on PlatPulse's live chart. A drift on either side fails:
 * changing a margin here without changing the register there is a comparison
 * failure, which is the point of stating them here.
 */
const EMERALD = {
  left: 56,
  right: 24,
  top: 30,
  bottom: 32,
  bottomWithLegend: 52,
  fontSize: 11,
  xLabelMargin: 12,
  yLabelMargin: 6,
  legendBottom: 4,
  legendItem: 12,
  legendGap: 20,
} as const

/** Emerald's h-[132px] md:h-[168px] lg:h-[192px], keyed by CSS width. */
function expectedChartHeight(width: number): number {
  if (width >= 1024) return 192
  if (width >= 768) return 168
  return 132
}

/**
 * The fixture. One window, thirteen observations five seconds apart, every series
 * present in every one of them: nothing is filled, nothing is bridged and nothing
 * is smoothed, so all this data decides is which axis each chart has to scale
 * itself against. The four hard cases the acceptance names are all in here:
 *
 *   - Host network  upload climbing to 1.4 GiB/s and download to 2.6 GiB/s, so
 *                   the axis title carries a long unit: "Speed (GiB/s)"
 *   - Peers         around 1,200 peers in 726 inbound / 514 outbound
 *                   observations, so a four-digit tick has to stay in the gutter
 *   - Block         interval climbing to 58,400 ms, so the axis is read in
 *                   seconds while the tooltip keeps milliseconds
 *   - Process CPU   131% at its peak, above one core
 */
const FIXTURE = {
  processCpuPercent: [42, 58, 71, 96, 118, 131, 124, 109, 88, 66, 51, 45, 40],
  processMemoryPercent: [61, 62, 62, 63, 63, 64, 64, 65, 65, 66, 66, 67, 67],
  networkRxBytesPerSec: [0.4, 0.6, 0.9, 1.4, 2.1, 2.6, 2.4, 1.9, 1.3, 0.9, 0.6, 0.5, 0.4].map((value) => value * GiB),
  networkTxBytesPerSec: [0.2, 0.3, 0.5, 0.8, 1.1, 1.4, 1.3, 1.0, 0.7, 0.5, 0.3, 0.25, 0.2].map((value) => value * GiB),
  peerInboundCount: [1180, 1195, 1210, 1225, 1240, 1255, 1268, 1249, 1230, 1215, 1200, 1190, 1180],
  peerOutboundCount: [860, 872, 884, 896, 908, 920, 932, 921, 910, 899, 888, 874, 862],
  blockIntervalMs: [12000, 13500, 15200, 17800, 21500, 26400, 34800, 46200, 58400, 43800, 27600, 19200, 14100],
  transactionCount: [980, 1120, 1340, 1620, 1980, 2410, 2890, 3320, 3660, 3180, 2540, 1710, 1180],
} as const

const FIXTURE_METRICS = Object.keys(FIXTURE)
const OBSERVATIONS = 13
const SAMPLE_SECONDS = 5

/** What each card must show, decided by the fixture above. */
const CARDS = [
  { heading: 'Process CPU', axisName: 'CPU (%)', legend: [] as string[], series: 1 },
  { heading: 'Process memory', axisName: 'Memory (%)', legend: [], series: 1 },
  { heading: 'Host network', axisName: 'Speed (GiB/s)', legend: ['Download', 'Upload'], series: 2 },
  { heading: 'Peer connections', axisName: 'Connections', legend: ['Inbound', 'Outbound'], series: 2 },
  { heading: 'Block interval', axisName: 'Duration (s)', legend: [], series: 1 },
  { heading: 'Transactions / block', axisName: 'Txs', legend: [], series: 1 },
]

/** The window the fixture was written into, captured from the real answer. */
let windowFrom = 0
let windowTo = 0

async function installFixedFixture(page: Page) {
  await page.route(metricsRoute, async (route) => {
    const response = await route.fetch()
    const body = (await response.json()) as Record<string, unknown>
    const from = Date.parse(String(body.from))
    const to = Date.parse(String(body.to))
    windowFrom = from
    windowTo = to
    const at = (seconds: number) => new Date(from + seconds * 1000).toISOString()
    for (const metric of FIXTURE_METRICS) {
      body[metric] = (FIXTURE as unknown as Record<string, number[]>)[metric].map((value, index) => ({
        sampledAt: at(index * SAMPLE_SECONDS),
        value,
      }))
    }
    // The coverage ledger states what the observations above really are: the whole
    // window observed, no silence recorded, nothing expired.
    const series = Array.isArray(body.series) ? (body.series as Record<string, unknown>[]) : []
    body.series = series.map((entry) => ({
      ...entry,
      observed: true,
      neverObservedProven: true,
      observationCount: OBSERVATIONS,
      firstObservedAt: at(0),
      lastObservedAt: at((OBSERVATIONS - 1) * SAMPLE_SECONDS),
      cadenceSeconds: SAMPLE_SECONDS,
      gapThresholdSeconds: SAMPLE_SECONDS * 3,
      coveredSeconds: (to - from) / 1000,
      unobservedTailSeconds: 0,
      gaps: [],
    }))
    await route.fulfill({ response, json: body })
  })
  // The card headers read the Node row, not the window: the network directions and
  // the peer count beside the titles have to be the same hard cases.
  await page.route(nodeRoute, async (route) => {
    const response = await route.fetch()
    const body = (await response.json()) as Record<string, unknown>
    body.processCpuPercent = 131
    body.processMemoryPercent = 67
    body.hostNetworkRxBytesPerSec = 2.6 * GiB
    body.hostNetworkTxBytesPerSec = 1.4 * GiB
    body.peers = { ...(body.peers as Record<string, unknown>), peerCount: 1240, inboundCount: 726, outboundCount: 514 }
    await route.fulfill({ response, json: body })
  })
}

function metricsCard(page: Page, heading: string): Locator {
  return page.locator('[data-slot="node-metric-card"]')
    .filter({ has: page.getByRole('heading', { level: 3, name: heading, exact: true }) })
}

async function openFixtureDeck(page: Page) {
  await page.goto('/nodes/' + PUBLIC_NODE_ID)
  await expect(page.getByRole('heading', { level: 1, name: PUBLIC_NODE_NAME })).toBeVisible({ timeout: 15_000 })
  await expect(page.locator('[data-slot="node-metric-card"]')).toHaveCount(CARDS.length)
  await expect(page.locator('[data-slot="node-metric-chart"] svg')).toHaveCount(CARDS.length)
  for (const card of CARDS) {
    // The plot draws the fixture, not the real window: thirteen observations per
    // direction, which is also what makes the axis labels below predictable.
    await expect(metricsCard(page, card.heading).locator('[data-slot="node-metric-chart"]'))
      .toHaveAttribute('data-points', String(card.series * OBSERVATIONS))
  }
}

async function setTheme(page: Page, theme: Theme) {
  const button = page.locator('[data-slot="theme-toggle"]')
  const target = theme === 'light' ? 'Light' : 'Dark'
  for (let step = 0; step < 4; step += 1) {
    const label = (await button.getAttribute('aria-label')) ?? ''
    const current = /Theme: (Auto|Light|Dark)/.exec(label)?.[1] ?? 'Auto'
    if (current === target) break
    await button.click()
  }
  await expect(button).toHaveAttribute('aria-label', new RegExp('^Theme: ' + target))
}

type Box = { x: number; right: number; y: number; bottom: number }
type RenderedText = Box & { text: string; fontSize: number }

type ChartGeometry = {
  heading: string
  width: number
  height: number
  points: number
  texts: RenderedText[]
  /** The dashed split lines: Emerald's plot box, as the renderer painted it. */
  splits: Box[]
  /** The legend swatches: a filled 12px square per legend item. */
  swatches: Box[]
}

async function deckGeometry(page: Page): Promise<ChartGeometry[]> {
  return page.locator('[data-slot="node-metrics-section"]').evaluate((section) => {
    const relative = (rect: DOMRect, box: DOMRect): Box => ({
      x: rect.left - box.left,
      right: rect.right - box.left,
      y: rect.top - box.top,
      bottom: rect.bottom - box.top,
    })
    const cards = Array.from(section.querySelectorAll('[data-slot="node-metric-card"]'))
    return cards.map((card) => {
      const container = card.querySelector('[data-slot="node-metric-chart"]') as HTMLElement
      const box = container.getBoundingClientRect()
      const svg = container.querySelector('svg')
      const texts: RenderedText[] = svg === null ? [] : Array.from(svg.querySelectorAll('text')).map((element) => ({
        text: (element.textContent ?? '').trim(),
        fontSize: Number.parseFloat(window.getComputedStyle(element).fontSize),
        ...relative(element.getBoundingClientRect(), box),
      }))
      // The split lines are the only dashed paths: the curves and the axis line are
      // both solid. Bars are filled and strokeless, so they are not swatches; the
      // legend swatch is the only filled path that carries a stroke.
      const paths = svg === null ? [] : Array.from(svg.querySelectorAll('path'))
      const splits = paths
        .filter((element) => element.getAttribute('stroke-dasharray') === '4,2')
        .map((element) => relative(element.getBoundingClientRect(), box))
      const swatches = paths
        .filter((element) => {
          if (element.getAttribute('fill') === null || element.getAttribute('fill') === 'none') return false
          if (element.getAttribute('stroke') === null) return false
          const rect = element.getBoundingClientRect()
          return Math.round(rect.width) === 12 && Math.round(rect.height) === 12
        })
        .map((element) => relative(element.getBoundingClientRect(), box))
      return {
        heading: (card.querySelector('h3')?.textContent ?? '').trim(),
        width: box.width,
        height: box.height,
        points: Number(container.dataset.points ?? '0'),
        texts,
        splits,
        swatches,
      }
    })
  })
}

/**
 * The deck once the renderer has caught up with the viewport. `setViewportSize`
 * is observed asynchronously: the card shrinks to the new breakpoint's height in
 * the same frame, while ECharts is still painting the previous one, and a
 * measurement taken in that gap reports a 192px plot inside a 132px box - the
 * false "label outside its chart" this poll removes. It waits on the geometry
 * this suite asserts anyway, so a genuine layout defect still fails, at the
 * assertion rather than as a timeout.
 */
async function settledDeck(page: Page, width: number): Promise<ChartGeometry[]> {
  let charts: ChartGeometry[] = []
  await expect
    .poll(async () => {
      charts = await deckGeometry(page)
      if (charts.length !== CARDS.length) return false
      return charts.every((chart, index) => {
        const card = CARDS[index]
        if (Math.round(chart.height) !== expectedChartHeight(width)) return false
        if (chart.splits.length < 2) return false
        const bottom = chart.height - (card.legend.length > 0 ? EMERALD.bottomWithLegend : EMERALD.bottom)
        // The right edge is part of the condition because changing the viewport
        // width resizes the card before ECharts repaints: without it the poll
        // could return a chart still painted at the previous width.
        // The tick count is part of it too: ECharts' own resize repaints the grid
        // at the new size in the ResizeObserver callback, while the band count the
        // new plot height allows reaches the option only on React's next render.
        // In that gap the old band count is drawn into the shorter plot, so the
        // split lines outnumber the labels ECharts still has room to print.
        const lines = chart.splits.filter((line) => line.right - line.x > 1).length
        const ticks = chart.texts.filter((text) => text.text !== card.axisName && text.text !== '' && text.y < bottom).length
        return Math.abs(Math.min(...chart.splits.map((line) => line.x)) - EMERALD.left) <= 1
          && Math.abs(Math.max(...chart.splits.map((line) => line.bottom)) - bottom) <= 1
          && Math.abs(Math.max(...chart.splits.map((line) => line.right)) - (chart.width - EMERALD.right)) <= 1
          && lines === ticks
      })
    }, { timeout: 10_000, message: 'the six charts settle into their ' + expectedChartHeight(width) + 'px box, ' + width + 'px wide, at ' + width + 'px' })
    .toBe(true)
  return charts
}

/** A clock label as seconds from local midnight, so it can be compared with the window. */
function clockSeconds(label: string): number {
  const [hours, minutes, seconds] = label.split(':').map(Number)
  return hours * 3600 + minutes * 60 + seconds
}

function localSeconds(instant: number): number {
  return clockSeconds(new Date(instant).toTimeString().slice(0, 8))
}

/** A card heading as a file name: 'Host network' becomes 'host-network'. */
function slug(label: string): string {
  return label.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '')
}

/** The measured record of one card, for the comparison table in the migration docs. */
function measurement(chart: ChartGeometry) {
  const lines = chart.splits.filter((line) => line.right - line.x > 1).map((line) => line.y)
  const texts = chart.texts.map((text) => ({ text: text.text, x: text.x, right: text.right, y: text.y, bottom: text.bottom }))
  return {
    heading: chart.heading,
    width: chart.width,
    height: chart.height,
    plot: {
      left: Math.min(...chart.splits.map((line) => line.x)),
      right: Math.max(...chart.splits.map((line) => line.right)),
      top: Math.min(...chart.splits.map((line) => line.y)),
      bottom: Math.max(...chart.splits.map((line) => line.bottom)),
      splitLines: lines.length,
    },
    swatches: chart.swatches.length,
    texts,
  }
}

/** How far a clock label may sit past the window edge while staying honest. */
const CLOCK_SLACK_SECONDS = 30

/**
 * A one-pixel grid line is rasterised on a half pixel, so its measured box can sit
 * half a pixel either side of the coordinate it was drawn at. A whole pixel of slack
 * still catches the defect this round is about (a plot box that shrank by tens of
 * pixels to make room for a label).
 */
function expectPx(actual: number, expected: number, where: string): void {
  expect(Math.abs(actual - expected), where + ': measured ' + actual + ', expected ' + expected).toBeLessThanOrEqual(1)
}

function insideChart(text: RenderedText, chart: ChartGeometry, tolerance = 0.6): boolean {
  return text.x >= -tolerance
    && text.right <= chart.width + tolerance
    && text.y >= -tolerance
    && text.bottom <= chart.height + tolerance
}

test.describe('Node Detail six-chart visual acceptance', () => {
  test('keeps every axis label, unit and legend inside its chart at every accepted width', async ({ page }, testInfo) => {
    // Two themes times the accepted widths, each with six charts to measure and
    // a deck screenshot to attach.
    test.setTimeout(180_000)
    const widths = measuredWidths(testInfo)
    await loginAs(page)
    await installFixedFixture(page)
    await openFixtureDeck(page)

    for (const theme of THEMES) {
      await setTheme(page, theme)
      for (const width of widths) {
        await page.setViewportSize({ width, height: width >= 1024 ? 900 : 1024 })
        const charts = await settledDeck(page, width)
        expect(charts.map((chart) => chart.heading), 'six cards, in order, at ' + width).toEqual(CARDS.map((card) => card.heading))

        for (const chart of charts) {
          const expected = CARDS.find((card) => card.heading === chart.heading)
          if (expected === undefined) throw new Error('unknown card ' + chart.heading)
          const where = chart.heading + ' at ' + width + 'px, ' + theme
          const plotLeft = EMERALD.left
          const plotRight = chart.width - EMERALD.right
          const plotBottom = chart.height - (expected.legend.length > 0 ? EMERALD.bottomWithLegend : EMERALD.bottom)

          // 1. The card draws the fixture, in a box of the pinned height for this
          //    breakpoint (Emerald's h-48 from lg), whatever its labels have to say.
          expect(chart.points, where).toBe(expected.series * OBSERVATIONS)
          expect(Math.round(chart.height), where + ': pinned chart height').toBe(expectedChartHeight(width))
          expect(Math.round(chart.width), where + ': filled card width').toBeGreaterThan(200)

          // 2. No visible text is painted outside the chart it belongs to. This is
          //    the defect the round started from: a rate tick hung off the left edge.
          for (const text of chart.texts) {
            expect(insideChart(text, chart), where + ': "' + text.text + '" is outside its chart (' + JSON.stringify(text) + ')').toBe(true)
            expect(text.fontSize, where + ': "' + text.text + '" font size').toBe(EMERALD.fontSize)
          }

          // 3. The plot box is Emerald's, measured from the dashed split lines: the
          //    grid did not shrink to make room for a label, and nothing is painted
          //    past the shared gutter.
          expect(chart.splits.length, where + ': dashed split lines').toBeGreaterThan(1)
          expectPx(Math.min(...chart.splits.map((line) => line.x)), plotLeft, where + ': grid left')
          expectPx(Math.max(...chart.splits.map((line) => line.right)), plotRight, where + ': grid right')
          expectPx(Math.min(...chart.splits.map((line) => line.y)), EMERALD.top, where + ': grid top')
          expectPx(Math.max(...chart.splits.map((line) => line.bottom)), plotBottom, where + ': grid bottom')

          // 4. The axis title names the unit and is anchored inside the chart, above
          //    the ticks that belong to it - never centred over a gutter it is wider
          //    than, which is what clipped the rate axis.
          const name = chart.texts.find((text) => text.text === expected.axisName)
          expect(name, where + ': axis title "' + expected.axisName + '"').toBeDefined()
          expect(name!.x, where + ': axis title starts inside the chart').toBeGreaterThanOrEqual(0)
          expect(Math.round(name!.x), where + ': axis title inset').toBe(4)
          expect(name!.bottom, where + ': axis title clears the plot').toBeLessThanOrEqual(EMERALD.top)

          // 5. The ticks are bare numbers in the unit the title names - a tick that
          //    repeats the unit is the duplicate this round removed - they fit the
          //    gutter, they end on Emerald's 6px margin, and no two overlap.
          const ticks = chart.texts.filter((text) => text !== name && text.y < plotBottom && text.text !== '')
          expect(ticks.length, where + ': value ticks').toBeGreaterThan(1)
          const ordered = [...ticks].sort((a, b) => a.y - b.y)
          const splitLines = chart.splits.filter((line) => line.right - line.x > 1).map((line) => line.y)
          expect(splitLines.length, where + ': horizontal split lines').toBe(ticks.length)
          for (const tick of ordered) {
            expect(tick.text, where + ': tick "' + tick.text + '" is a bare number').toMatch(/^[0-9][0-9,]*(.[0-9]+)?$/)
            expect(tick.right, where + ': tick "' + tick.text + '" stays in the gutter').toBeLessThanOrEqual(plotLeft - EMERALD.yLabelMargin + 0.6)
            expect(tick.x, where + ': tick "' + tick.text + '" fits the shared gutter').toBeGreaterThanOrEqual(0)
            // One tick per line, centred on it: the number belongs to the line it
            // names. The lowest label therefore hangs half its height below the plot,
            // which is why the clock band below has to stay clear of it.
            const centre = (tick.y + tick.bottom) / 2
            const nearest = Math.min(...splitLines.map((line) => Math.abs(line - centre)))
            expect(nearest, where + ': tick "' + tick.text + '" sits on a split line (centre ' + centre + ', lines ' + JSON.stringify(splitLines) + ')').toBeLessThanOrEqual(1.5)
          }
          for (let index = 1; index < ordered.length; index += 1) {
            expect(ordered[index].y, where + ': ticks "' + ordered[index - 1].text + '" and "' + ordered[index].text + '" do not overlap')
              .toBeGreaterThanOrEqual(ordered[index - 1].bottom - 0.5)
          }
          // 5b. The title is a row of its own: it clears the top tick instead of
          //     being centred over it, which is how the old "Speed" label collided
          //     with the top of the rate column.
          expect(name!.bottom, where + ': axis title "' + expected.axisName + '" clears the top tick "' + ordered[0].text + '"')
            .toBeLessThanOrEqual(ordered[0].y - 1)

          // 6. The clock labels are the window's own: they sit under the axis inside
          //    the chart, they run from the window's start to its end, and they span
          //    the plot instead of huddling in one corner.
          const clocks = chart.texts.filter((text) => /^[0-9]{2}:[0-9]{2}:[0-9]{2}$/.test(text.text))
          expect(clocks.length, where + ': clock labels').toBeGreaterThanOrEqual(3)
          const byTime = [...clocks].sort((a, b) => a.x - b.x)
          const first = byTime[0].text
          const last = byTime[byTime.length - 1].text
          expect(clockSeconds(first) - localSeconds(windowFrom), where + ': first clock label "' + first + '" is the window start').toBeGreaterThanOrEqual(-1)
          expect(clockSeconds(first) - localSeconds(windowFrom), where + ': first clock label "' + first + '" is the window start').toBeLessThanOrEqual(CLOCK_SLACK_SECONDS)
          expect(localSeconds(windowTo) - clockSeconds(last), where + ': last clock label "' + last + '" is the window end').toBeGreaterThanOrEqual(-1)
          expect(localSeconds(windowTo) - clockSeconds(last), where + ': last clock label "' + last + '" is the window end').toBeLessThanOrEqual(CLOCK_SLACK_SECONDS)
          expect(byTime[byTime.length - 1].right - byTime[0].x, where + ': clock labels span the plot').toBeGreaterThan((plotRight - plotLeft) * 0.6)
          const lowestTick = ordered[ordered.length - 1]
          expect(Math.min(...clocks.map((clock) => clock.y)), where + ': the value labels and the clock band are separate rows')
            .toBeGreaterThanOrEqual(lowestTick.bottom - 0.5)
          for (const clock of clocks) {
            expect(clock.y, where + ': clock label "' + clock.text + '" sits under the axis').toBeGreaterThan(plotBottom)
            expect(clock.y - plotBottom, where + ': clock label "' + clock.text + '" keeps Emerald\'s margin').toBeLessThanOrEqual(EMERALD.xLabelMargin + 8)
          }

          // 6b. The axis runs from the window's first instant to its last. The
          //     drawn curve starts on the plot's left edge and ends on its right
          //     (so the axis is not padded, truncated or rounded to a nice time),
          //     and every clock label stands exactly where its own instant maps at
          //     the window's pixels per second.
          const pxPerSecond = (plotRight - plotLeft) / Math.max(1, localSeconds(windowTo) - localSeconds(windowFrom))
          for (const clock of clocks) {
            const centre = (clock.x + clock.right) / 2
            const expectedCentre = plotLeft + (clockSeconds(clock.text) - localSeconds(windowFrom)) * pxPerSecond
            expect(Math.abs(centre - expectedCentre), where + ': clock label "' + clock.text + '" stands at its own instant (centre '
              + centre + ', expected ' + expectedCentre + ')').toBeLessThanOrEqual(1.5)
          }
          const strokes = await metricsCard(page, chart.heading)
            .locator('[data-slot="node-metric-chart"] svg path')
            .evaluateAll((paths) => paths.map((path) => {
              const container = path.closest('[data-slot="node-metric-chart"]')
              if (container === null) return null
              const box = container.getBoundingClientRect()
              const rect = path.getBoundingClientRect()
              return {
                stroke: path.getAttribute('stroke'),
                left: rect.left - box.left,
                right: rect.right - box.left,
                width: rect.width,
              }
            }))
          // The drawn series: a stroked path, never a dashed split line (black) and
          // never a filled area or bar (no stroke). A bar chart has none, and its
          // category axis is checked through the labels above instead.
          const curves = (strokes ?? []).filter((item) => item !== null && item.stroke !== null && item.stroke !== 'none'
            && item.stroke !== 'rgb(0,0,0)' && item.width > 20)
          if (curves.length > 0) {
            const curveLeft = Math.min(...curves.map((curve) => curve!.left))
            const curveRight = Math.max(...curves.map((curve) => curve!.right))
            expect(Math.abs(curveLeft - plotLeft), where + ': the curve starts on the plot\'s left edge (measured ' + curveLeft + ')').toBeLessThanOrEqual(1.5)
            expect(Math.abs(curveRight - plotRight), where + ': the curve ends on the plot\'s right edge (measured ' + curveRight + ')').toBeLessThanOrEqual(1.5)
          }

          // 7. A legend, where a card has one, keeps Emerald's 12px swatch and 20px
          //    item gap, names its series in series order, and sits below the clock
          //    labels without touching them.
          if (expected.legend.length > 0) {
            const legendTexts = expected.legend.map((label) => {
              const found = chart.texts.find((text) => text.text === label)
              expect(found, where + ': legend "' + label + '"').toBeDefined()
              return found as RenderedText
            })
            expect(legendTexts.map((text) => text.text), where + ': legend order').toEqual([...expected.legend])
            const clockBottom = Math.max(...clocks.map((clock) => clock.bottom))
            for (const text of legendTexts) {
              expect(text.y, where + ': legend "' + text.text + '" clear of the clock labels').toBeGreaterThanOrEqual(clockBottom)
              expect(chart.height - text.bottom, where + ': legend "' + text.text + '" clears the card edge').toBeGreaterThanOrEqual(EMERALD.legendBottom)
              expect(chart.height - text.bottom, where + ': legend "' + text.text + '" sits on the bottom band').toBeLessThanOrEqual(EMERALD.legendBottom + 8)
            }
            const swatches = [...chart.swatches].sort((a, b) => a.x - b.x)
            expect(swatches.length, where + ': legend swatches').toBe(expected.legend.length)
            for (const swatch of swatches) {
              expect(Math.round(swatch.right - swatch.x), where + ': legend swatch width').toBe(EMERALD.legendItem)
              expect(Math.round(swatch.bottom - swatch.y), where + ': legend swatch height').toBe(EMERALD.legendItem)
            }
            expect(Math.round(swatches[1].x - legendTexts[0].right), where + ': legend item gap').toBe(EMERALD.legendGap)
          } else {
            // A card without a legend draws no swatch: the Plot labels below are the
            // only other text, so a stray one would be a legend by accident.
            expect(chart.swatches.length, where + ': no legend, no swatch').toBe(0)
          }
        }

        // 8. The deck is one grid: every card is the same size, every plot box starts
        //    on the same column, and the chart keeps the height its breakpoint pins.
        expect(new Set(charts.map((chart) => Math.round(chart.width))).size, width + ': one card width').toBe(1)
        expect(new Set(charts.map((chart) => Math.round(chart.height))).size, width + ': one chart height').toBe(1)
        expect(new Set(charts.map((chart) => Math.round(Math.min(...chart.splits.map((line) => line.x))))).size, width + ': one shared gutter').toBe(1)
        expect(new Set(charts.map((chart) => Math.round(Math.max(...chart.splits.map((line) => line.right))))).size, width + ': one shared right edge').toBe(1)

        // Evidence: the deck as it renders. Always attached to the run; written into
        // the migration record too when EMERALD_EVIDENCE is set.
        const shot = await page.locator('[data-slot="node-metrics-section"]').screenshot()
        await testInfo.attach('six-charts-' + theme + '-' + width + '.png', { body: shot, contentType: 'image/png' })
        const dir = '../docs/visual-migration/emerald/screenshots/chart-acceptance/' + testInfo.project.name
        const keepEvidence = process.env.EMERALD_EVIDENCE === '1'
        if (keepEvidence) {
          await mkdir(dir, { recursive: true })
          await writeFile(dir + '/six-charts-' + theme + '-' + width + '.png', shot)
        }
        // The two widths a reader compares against Emerald by hand: the phone and
        // the desktop. The chart region itself is attached to every run - the deck
        // screenshot is too small to judge a tick against its gutter - and written
        // into the migration record with its measured geometry when evidence is on,
        // so the comparison is against numbers this run produced rather than a
        // memory of what the chart used to look like.
        if (width === 390 || width === 1440) {
          for (const chart of charts) {
            const card = metricsCard(page, chart.heading).locator('[data-slot="node-metric-chart"]')
            const region = await card.screenshot()
            const file = 'chart-' + slug(chart.heading) + '-' + theme + '-' + width + '.png'
            await testInfo.attach(file, { body: region, contentType: 'image/png' })
            if (keepEvidence) await writeFile(dir + '/' + file, region)
          }
          if (keepEvidence) await writeFile(dir + '/geometry-' + theme + '-' + width + '.json', JSON.stringify(charts.map(measurement), null, 2))
        }
      }
    }
  })

  test('reads a long block interval in seconds on the axis and in milliseconds in the tooltip', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 })
    await loginAs(page)
    await installFixedFixture(page)
    await openFixtureDeck(page)

    const plot = metricsCard(page, 'Block interval').locator('[data-slot="node-metric-chart"]')
    // A real pointer gesture, the way the six-chart spec does it: centre the card
    // (the deck sits below the fold here, and a bare jump can land before the layout
    // settles), leave the plot, then cross into it in steps so ECharts sees movement.
    await plot.evaluate((node) => node.scrollIntoView({ block: 'center' }))
    const box = await plot.boundingBox()
    if (box === null) throw new Error('Block interval plot is not laid out')
    // The fixture's peak is the ninth of thirteen observations, two thirds across.
    const pointer = { x: box.x + box.width * 0.72, y: box.y + box.height * 0.45 }
    const hit = await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.closest('[data-slot="node-metric-chart"]') !== null, pointer)
    expect(hit, 'the pointer is over the Block interval plot, not over a sticky header').toBe(true)
    await page.mouse.move(2, 2)
    await page.mouse.move(pointer.x, pointer.y, { steps: 4 })
    const tooltip = plot.locator('.node-metric-chart-tooltip')
    await expect(tooltip).toBeVisible({ timeout: 5_000 })
    const text = (await tooltip.textContent()) ?? ''
    expect(text, 'the tooltip states the sample the Server retained, in milliseconds').toMatch(/[0-9]+(\.[0-9]+)? ms(\s|$)/)
    expect(text, 'the tooltip never rescales the sample into seconds').not.toMatch(/[0-9]+(\.[0-9]+)? s(\s|$)/)
    expect(text, 'the tooltip names the series').toContain('Block interval')
    // The axis above it says seconds, and says so once: in its title.
    await expect(plot.locator('svg text', { hasText: 'Duration (s)' })).toHaveCount(1)
  })
})

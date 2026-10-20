import { expect, test, type Locator, type Page } from '@playwright/test'
import { expectNoHorizontalOverflow, expectVisibleInteractiveTargets, loginAs } from './helpers'

/**
 * Issue #150 final Node Detail acceptance: the fixed four Playwright projects
 * multiplied by both themes and the normal / unknown / stale / single-CPU
 * failure / real-gap / never-observed scenarios exercise the six-chart
 * continuous reading page. The metrics response is fetched from the real
 * Server and only the targeted dimension is rewritten, so retained samples
 * stay real.
 */

const PUBLIC_NODE_NAME = 'Node A'
const PUBLIC_NODE_ID = '0195f2a1-0014-4014-8014-000000000014'
const CHART_HEADINGS = [
  'Process CPU',
  'Process memory',
  'Host network',
  'Peer connections',
  'Block interval',
  'Transactions / block',
] as const
const SCENARIOS = ['normal', 'unknown', 'stale', 'single-cpu-failure', 'gap', 'never-observed', 'expired'] as const
const THEMES = ['light', 'dark'] as const

type Scenario = (typeof SCENARIOS)[number]
type Theme = (typeof THEMES)[number]

const nodeRoute = '**/api/public/v1/nodes/' + PUBLIC_NODE_ID
const metricsRoute = nodeRoute + '/metrics'

/**
 * Centres a plot and returns its middle point, checking the point really belongs
 * to the plot: on the phones the sticky shell header would otherwise sit over
 * the point the pointer is sent to, and the crosshair would stay untriggered.
 */
async function centredPlotPoint(page: Page, plot: Locator, name: string) {
  await plot.evaluate((node) => node.scrollIntoView({ block: 'center' }))
  const box = await plot.boundingBox()
  if (!box) throw new Error('the ' + name + ' plot has no box')
  const point = { x: box.x + box.width / 2, y: box.y + box.height / 2 }
  const hit = await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.closest('[data-slot="node-metric-chart"]') !== null, point)
  expect(hit, name + ' plot under the pointer').toBe(true)
  return point
}

/**
 * The inline Validator activity badge is 11px inside the 12px meta row. With
 * some Linux font metrics (FreeSans/IPAGothic/FreeSerif/FreeMono, installed by
 * Playwright's `install --with-deps`) its data-tooltip wrapper used to add 1px
 * to the identity header, which shifted the summary, map and panels below it.
 * Pinning the font here keeps the role-geometry assertion red-capable off CI
 * instead of depending on whatever the runner's fontconfig picks.
 */
const FRAGILE_HEADER_FONTS = 'FreeSans, IPAGothic, FreeSerif, FreeMono, sans-serif'

async function forceFragileHeaderFont(page: Page) {
  await page.addInitScript((fonts: string) => {
    const apply = () => {
      const style = document.createElement('style')
      style.textContent = `:root{--font-sans:${fonts} !important}`
      document.documentElement.appendChild(style)
    }
    if (document.documentElement) apply()
    else document.addEventListener('DOMContentLoaded', apply)
  }, FRAGILE_HEADER_FONTS)
}

/**
 * No card writes the coverage verdict as small print under its title any more:
 * the sentence is written into the chart's own sr-only description, which every
 * plot points at through `aria-describedby`. Read the six sentences back by id,
 * each paired with the title of the card it belongs to, so a scenario can assert
 * what every individual card states rather than only how many said what.
 */
async function chartDescriptions(metrics: Locator) {
  return metrics.locator('[data-slot="node-metric-chart"]').evaluateAll((plots) =>
    plots.map((plot) => {
      const id = plot.getAttribute('aria-describedby')
      return {
        heading: plot.closest('[data-slot="node-metric-card"]')?.querySelector('h3')?.textContent?.trim() ?? '',
        text: id ? (document.getElementById(id)?.textContent ?? '') : '',
      }
    }),
  )
}

/**
 * Assert what each individual card states, by title, rather than how many cards
 * said what. A count alone cannot tell the cards apart: four cards saying
 * `No samples reported yet` is also what a deck would produce where one card
 * said it twice, another said nothing at all, and a fifth carried both sentences.
 * Every card must carry exactly one of the three absence verdicts, and the one
 * its own series has earned.
 */
type AbsenceVerdict = 'never-observed' | 'no-ledger' | 'dated'
type ChartHeading = (typeof CHART_HEADINGS)[number]

async function expectAbsenceVerdicts(
  metrics: Locator,
  verdicts: Record<ChartHeading, AbsenceVerdict>,
) {
  const descriptions = await chartDescriptions(metrics)
  expect(descriptions.map((entry) => entry.heading).sort(), JSON.stringify(descriptions)).toEqual([...CHART_HEADINGS].sort())
  for (const entry of descriptions) {
    const neverObserved = entry.text.includes('No samples reported yet')
    const unledgered = entry.text.includes('No retained observations')
    const dated = /Last observation [0-9]/.test(entry.text)
    // Exactly one verdict per card, never two and never none.
    expect(Number(neverObserved) + Number(unledgered) + Number(dated), JSON.stringify(entry)).toBe(1)
    const stated: AbsenceVerdict = neverObserved ? 'never-observed' : unledgered ? 'no-ledger' : 'dated'
    expect(stated, JSON.stringify(entry)).toBe(verdicts[entry.heading as ChartHeading])
  }
}

/** Cycle the production theme control to the requested explicit theme. */
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
  expect(await page.evaluate(() => document.documentElement.classList.contains('dark'))).toBe(theme === 'dark')
}

/** Rewrite only the failing dimension on top of the real Server responses. */
async function installScenario(page: Page, scenario: Scenario) {
  if (scenario === 'normal') return
  if (scenario === 'gap' || scenario === 'never-observed' || scenario === 'expired') {
    // Issue #225: the observation/coverage contract. The gap scenario rewrites
    // one series with a real silence inside the window and the coverage verdict
    // that measures it; the never-observed scenario keeps the node row real
    // while the series the Server keeps a ledger for report that nobody ever
    // observed them; the expired scenario keeps every array empty as well, but
    // those same ledgered series are dated from it, which is how the Server
    // answers a series whose raw samples retention removed instead of claiming
    // nobody observed it (crates/platpulse-server/src/http/public.rs,
    // public_node_metrics_dates_a_series_whose_samples_expired). The two block
    // series have no ledger to speak for them, so both scenarios leave them
    // `observed: false` with `neverObservedProven: false`: an expiry the Server
    // cannot date is an unknown, never a claim that nobody reported them.
    await page.route(metricsRoute, async (route) => {
      const response = await route.fetch()
      const body = (await response.json()) as Record<string, unknown>
      const entries = Array.isArray(body.series) ? (body.series as Record<string, unknown>[]) : []
      // Only a series stored under a metric name has a ledger behind it
      // (crates/platpulse-server/src/http/public.rs, public_metric_name and
      // metric_series_has_ledger), so only those can be proved never observed.
      // The two block series live in block_summaries, which has no ledger.
      const ledgered = (metric: string) => metric !== 'blockIntervalMs' && metric !== 'transactionCount'
      if (scenario === 'never-observed') {
        for (const key of Object.keys(body)) {
          if (Array.isArray(body[key]) && key !== 'series') body[key] = []
        }
        body.series = entries.map((entry) => ({
          ...entry,
          observed: false,
          neverObservedProven: ledgered(String(entry.metric)),
          observationCount: 0,
          firstObservedAt: null,
          lastObservedAt: null,
          cadenceSeconds: 0,
          gapThresholdSeconds: 0,
          coveredSeconds: 0,
          unobservedTailSeconds: 0,
          gaps: [],
        }))
      } else if (scenario === 'expired') {
        const to = Date.parse(String(body.to))
        for (const key of Object.keys(body)) {
          if (Array.isArray(body[key]) && key !== 'series') body[key] = []
        }
        const aged = (days: number) => new Date(to - days * 86_400_000).toISOString()
        body.series = entries.map((entry) => ledgered(String(entry.metric))
          ? {
              ...entry,
              observed: true,
              neverObservedProven: true,
              observationCount: 0,
              firstObservedAt: aged(41),
              lastObservedAt: aged(40),
              cadenceSeconds: 0,
              gapThresholdSeconds: 0,
              coveredSeconds: 0,
              unobservedTailSeconds: 40 * 86_400,
              gaps: [],
            }
          : {
              ...entry,
              observed: false,
              neverObservedProven: false,
              observationCount: 0,
              firstObservedAt: null,
              lastObservedAt: null,
              cadenceSeconds: 0,
              gapThresholdSeconds: 0,
              coveredSeconds: 0,
              unobservedTailSeconds: 0,
              gaps: [],
            })
      } else {
        const from = Date.parse(String(body.from))
        const to = Date.parse(String(body.to))
        const at = (seconds: number) => new Date(from + seconds * 1000).toISOString()
        // Two stretches of 5s observations 30s apart, then two more that settle
        // the rhythm again: the cadence is 5s, the threshold 15s, so the silence
        // is a gap the chart must break at, and the newest observation is 5s
        // before the window ends. These are the observations and the verdict the
        // Server really derives from them (crates/platpulse-server/src/http/public.rs,
        // public_metric_coverage_names_the_silence_of_a_recovered_series), not a
        // shape the Server could never answer.
        body.processCpuPercent = [5, 10, 15, 45, 50, 55].map((seconds) => ({ sampledAt: at(seconds), value: 20 }))
        body.series = entries.map((entry) => entry.metric === 'processCpuPercent'
          ? {
              ...entry,
              observed: true,
              observationCount: 6,
              firstObservedAt: at(5),
              lastObservedAt: at(55),
              cadenceSeconds: 5,
              gapThresholdSeconds: 15,
              coveredSeconds: 20,
              unobservedTailSeconds: (to - (from + 55_000)) / 1000,
              gaps: [{ from: at(15), to: at(45), seconds: 30, kind: 'collection_gap' }],
            }
          : entry)
      }
      await route.fulfill({ response, json: body })
    })
    return
  }
  await page.route(nodeRoute, async (route) => {
    const response = await route.fetch()
    const body = (await response.json()) as Record<string, unknown>
    if (scenario === 'unknown') {
      Object.assign(body, {
        health: 'unknown',
        healthReason: 'No observation recorded yet',
        freshness: 'unknown',
        processCpuPercent: null,
        processMemoryPercent: null,
        processUptimeMs: null,
        currentHead: null,
        lastReportAt: null,
        hostCpuPercent: null,
        hostMemoryPercent: null,
        hostStoragePercent: null,
        hostNetworkRxBytesPerSec: null,
        hostNetworkTxBytesPerSec: null,
      })
    } else if (scenario === 'stale') {
      Object.assign(body, { health: 'unhealthy', healthReason: 'Latest observation is stale', freshness: 'stale' })
    } else if (scenario === 'single-cpu-failure') {
      // The process component fails while the Public Projection retains the
      // last-good value; the card must mark that honestly.
      Object.assign(body, { processState: 'error' })
    }
    await route.fulfill({ response, json: body })
  })
  await page.route(metricsRoute, async (route) => {
    const response = await route.fetch()
    const body = (await response.json()) as Record<string, unknown>
    for (const key of Object.keys(body)) {
      if (Array.isArray(body[key]) && (scenario === 'unknown' || scenario === 'stale')) body[key] = []
    }
    if (scenario === 'single-cpu-failure') body.processCpuPercent = []
    await route.fulfill({ response, json: body })
  })
}

async function openNodeDetail(page: Page) {
  await page.goto('/')
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible({ timeout: 15_000 })
  await page.getByRole('link', { name: new RegExp(PUBLIC_NODE_NAME) }).first().click()
  await expect(page.getByRole('heading', { level: 1, name: PUBLIC_NODE_NAME })).toBeVisible({ timeout: 15_000 })
}

test.describe('Node Detail real latest-60-second six-chart closure (issue #150)', () => {
  test('normal, unknown, stale, and single-CPU failure in both themes', async ({ page }, testInfo) => {
    // Seven scenarios times both themes is fourteen steps, and each one reloads
    // Node Detail in its own theme and re-asserts the whole reading order, with
    // a full-page evidence screenshot at 1440. That put the test at 24-28s
    // against Playwright's default 30s budget, so a slower machine spent the
    // budget instead of reporting a failure: desktop-1440 hit exactly 30.0s
    // locally while desktop-1280 passed at 28.1s. The budget is declared here,
    // like the other multi-step acceptance matrices in this suite, so the test
    // fails on an assertion rather than on the clock.
    test.setTimeout(180_000)
    await loginAs(page)

    for (const scenario of SCENARIOS) {
      for (const theme of THEMES) {
        await test.step(scenario + ' / ' + theme, async () => {
          await page.unroute(nodeRoute).catch(() => undefined)
          await page.unroute(metricsRoute).catch(() => undefined)
          await installScenario(page, scenario)
          await openNodeDetail(page)
          await setTheme(page, theme)

          // Continuous reading order: identity -> key summary -> six charts ->
          // collapsible diagnostics, with no Details/Network tabs left.
          await expect(page.getByRole('tab')).toHaveCount(0)
          await expect(page.getByRole('heading', { level: 2, name: 'Latest 60 seconds' })).toBeVisible()
          const order = await page.evaluate(() => {
            const top = (selector: string) => {
              const element = document.querySelector(selector)
              return element ? element.getBoundingClientRect().top + window.scrollY : Number.NaN
            }
            return {
              identity: top('[data-slot="node-identity-main"]'),
              summary: top('[aria-label="Node key summary"]'),
              metrics: top('[data-slot="node-metrics-section"]'),
            }
          })
          expect(order.identity, JSON.stringify(order)).toBeLessThan(order.summary)
          expect(order.summary, JSON.stringify(order)).toBeLessThan(order.metrics)

          // The accepted A container (issue #151): an uncarded identity block,
          // six summary tiles, and three parallel observation panels that sit
          // on one row at lg and stack to one column below 1024px.
          await expect(page.locator('[data-slot="node-hero-card"]')).toHaveCount(0)
          await expect(page.getByLabel('Node key summary').locator('[data-slot="node-summary-tile"]')).toHaveCount(6)
          await expect(page.locator('[data-slot="node-info-group"]')).toHaveCount(3)
          const panelBoxes = await page.locator('[data-slot="node-info-group"]').evaluateAll((nodes) => nodes.map((node) => {
            const box = node.getBoundingClientRect()
            return { top: Math.round(box.top), left: Math.round(box.left) }
          }))
          if ((page.viewportSize()?.width ?? 0) >= 1024) {
            expect(new Set(panelBoxes.map((box) => box.top)).size, JSON.stringify(panelBoxes)).toBe(1)
          } else {
            expect(new Set(panelBoxes.map((box) => box.left)).size, JSON.stringify(panelBoxes)).toBe(1)
          }

          const metrics = page.locator('[data-slot="node-metrics-section"]')
          await expect(metrics.getByRole('img', { name: /line chart over the last 60 seconds/ })).toHaveCount(4)
          await expect(metrics.getByRole('img', { name: /bar chart over the last 60 seconds/ })).toHaveCount(2)
          expect(await metrics.locator('[data-slot="node-metric-card"] h3').allTextContents()).toEqual([...CHART_HEADINGS])
          // The plot is the ECharts container the card mounts, not a hand-drawn
          // SVG, so the accepted heights are asserted on that container: the
          // upstream 192px from lg, 168px from md, 132px on the phones.
          const viewportWidth = page.viewportSize()?.width ?? 0
          const expectedPlotHeight = viewportWidth >= 1024 ? 192 : viewportWidth >= 768 ? 168 : 132
          const plots = metrics.locator('[data-slot="node-metric-chart"]')
          await expect(plots).toHaveCount(6)
          expect(await plots.evaluateAll((nodes) => nodes.map((node) => node.getAttribute('data-kind')))).toEqual(['line', 'line', 'line', 'line', 'bar', 'bar'])
          for (const plot of await plots.all()) await expect(plot).toHaveCSS('height', expectedPlotHeight + 'px')

          // One header recipe for the six cards: the title carries the weight at
          // 16px and the current value stays at its right in a 12px reading, so no
          // value shouts louder than the chart's own name. The gutter the axis
          // labels sit in is shared too, which is what keeps the six plots start
          // and end on the same two columns.
          const headerRecipe = await metrics.locator('[data-slot="node-metric-card"]').evaluateAll((cards) => cards.map((card) => {
            const title = card.querySelector('h3')
            const value = card.querySelector('[data-slot="node-metric-header"] [data-slot="node-metric-value"]')
            const secondary = card.querySelector('[data-slot="node-metric-value-secondary"]')
            return {
              titleSize: title ? getComputedStyle(title).fontSize : 'missing',
              titleWeight: title ? getComputedStyle(title).fontWeight : 'missing',
              valueSize: value ? getComputedStyle(value).fontSize : 'missing',
              secondarySize: secondary ? getComputedStyle(secondary).fontSize : 'none',
              valueTop: value ? Math.round(value.getBoundingClientRect().top) : 0,
              headerTop: Math.round((card.querySelector('[data-slot="node-metric-header"]')?.getBoundingClientRect().top) ?? 0),
            }
          }))
          expect(headerRecipe, JSON.stringify(headerRecipe)).toHaveLength(6)
          for (const recipe of headerRecipe) {
            expect(recipe.titleSize, JSON.stringify(headerRecipe)).toBe('16px')
            expect(Number(recipe.titleWeight), JSON.stringify(headerRecipe)).toBeGreaterThanOrEqual(600)
            expect(recipe.valueSize, JSON.stringify(headerRecipe)).toBe('12px')
            // The value reads from the header's own top edge, never from a footer.
            expect(recipe.valueTop - recipe.headerTop, JSON.stringify(headerRecipe)).toBeLessThan(24)
          }
          // The network card's second direction is a current reading as well, so it
          // is set at 12px rather than shrunk to a caption: one card carries it.
          const secondaryReading = headerRecipe.filter((recipe) => recipe.secondarySize !== 'none')
          expect(secondaryReading, JSON.stringify(headerRecipe)).toHaveLength(1)
          expect(secondaryReading[0]?.secondarySize, JSON.stringify(headerRecipe)).toBe('12px')
          const plotColumns = await plots.evaluateAll((nodes) => nodes.map((node) => {
            const box = node.getBoundingClientRect()
            return { left: Math.round(box.left), right: Math.round(box.right) }
          }))
          expect(new Set(plotColumns.map((column) => column.left)).size, JSON.stringify(plotColumns)).toBeLessThanOrEqual(3)
          expect(new Set(plotColumns.map((column) => column.right)).size, JSON.stringify(plotColumns)).toBeLessThanOrEqual(3)
          // The window is named in clock time on the axis itself; the relative
          // "60s … 0s" caption that used to sit under each plot is gone, not
          // duplicated beside it. A scenario with no samples mounts no chart at
          // all — that is the empty state's own contract, asserted below — so the
          // axis text is read from the plots that do exist, and a mounted chart
          // that forgot its clock labels still fails here.
          const axisTexts = await metrics.locator('[data-slot="node-metric-chart"] svg').evaluateAll((svgs) => svgs.map((svg) => svg.textContent ?? ''))
          for (const axisText of axisTexts) expect(axisText, JSON.stringify(axisTexts)).toMatch(/\d{2}:\d{2}:\d{2}/)
          await expect(metrics.getByText('60s', { exact: true })).toHaveCount(0)
          await expect(metrics.getByText('0s', { exact: true })).toHaveCount(0)

          if (scenario === 'normal') {
            await expect(metrics.locator('[data-slot="node-metric-chart-empty"]')).toHaveCount(0)
            // Process CPU + process memory + two direction pairs = six runs,
            // each drawn as its own series so no silence can be joined.
            expect(await metrics.locator('[data-slot="node-metric-chart"][data-kind="line"]')
              .evaluateAll((nodes) => nodes.reduce((total, node) => total + Number(node.getAttribute('data-runs') ?? 0), 0))).toBe(6)
            // A plot that only claims to be drawn is not drawn: every card with
            // samples must hold a live ECharts SVG with real curve geometry, and
            // none of the six may report a failed load. The data-runs/data-points
            // attributes above are inputs; this is the picture.
            await expect(metrics.locator("[data-chart-error]")).toHaveCount(0)
            const drawn = await plots.evaluateAll((nodes) => nodes.map((node) => ({
              points: Number(node.getAttribute('data-points') ?? 0),
              svg: node.querySelectorAll('svg').length,
              shapes: Array.from(node.querySelectorAll("svg path")).filter((path) => (path.getAttribute("d") ?? "").startsWith("M")).length,
            })))
            expect(drawn.filter((plot) => plot.points === 0 || (plot.svg === 1 && plot.shapes > 0))).toHaveLength(6)
            expect(drawn.some((plot) => plot.points > 0)).toBe(true)
            // The paint recipe is read here, before any pointer reaches a plot:
            // hovering a series lightens it (ECharts lifts the colour under the
            // emphasis state), so the resting paint is the honest reading.
            const cardFor = (name: string) => metrics.getByRole('article').filter({ has: page.getByRole('heading', { level: 3, name }) })
            // Process memory matches Emerald's RAM curve: the same coral the CPU uses,
            // sharing the 0.25 -> 0.02 area gradient with Process CPU.
            const gradientStops = (name: string) => cardFor(name)
              .locator('[data-slot="node-metric-chart"] svg linearGradient stop')
              .evaluateAll((stops) => stops.map((stop) => ({
                color: stop.getAttribute('stop-color') ?? '',
                opacity: stop.getAttribute('stop-opacity') ?? '',
              })))
            const cpuStops = await gradientStops('Process CPU')
            const memoryStops = await gradientStops('Process memory')
            expect(cpuStops.map((stop) => stop.opacity), JSON.stringify(cpuStops)).toEqual(['0.25', '0.02'])
            expect(memoryStops.map((stop) => stop.opacity), JSON.stringify(memoryStops)).toEqual(['0.25', '0.02'])
            // Both curves are painted in one colour, and that colour is the coral:
            // its red channel dominates while green and blue stay equal, which the
            // teal the memory curve used to take (green above blue) never does. The
            // channel values are read rather than spelled out because the renderer
            // parses the declared colour before it writes the stop.
            expect(memoryStops.map((stop) => stop.color), JSON.stringify(memoryStops)).toEqual(cpuStops.map((stop) => stop.color))
            for (const stop of [...cpuStops, ...memoryStops]) {
              const channels = (stop.color.match(/[\d.]+/g) ?? []).map(Number)
              expect(channels, JSON.stringify(stop)).toHaveLength(3)
              expect(channels[1], JSON.stringify(stop)).toBe(channels[2])
              expect(channels[0] - channels[1], JSON.stringify(stop)).toBeGreaterThan(60)
            }
            // The orange bars keep their colour and give up their weight, so a fast
            // cadence reads as a rhythm rather than as a slab of orange.
            const barFills = await cardFor('Block interval')
              .locator('[data-slot="node-metric-chart"] svg path[fill="#FFB347"]')
              .evaluateAll((bars) => bars.map((bar) => bar.getAttribute('fill-opacity')))
            expect(barFills.length, 'Block interval bars').toBeGreaterThan(0)
            expect(new Set(barFills), JSON.stringify(barFills)).toEqual(new Set(['0.75']))
            // One unit per rate axis: the ceiling the real peak produced names the
            // unit once, in the axis title, and every label under it is a bare
            // number — so MiB/s never sits beside KiB/s and no unit is repeated
            // down the gutter until it is clipped.
            const rateCard = cardFor('Host network').locator('[data-slot="node-metric-chart"]')
            const rateLabels = await rateCard
              .locator('svg text[text-anchor="end"]')
              .evaluateAll((labels) => labels.map((label) => (label.textContent ?? '').trim()))
            expect(rateLabels.length, 'rate axis labels').toBeGreaterThan(1)
            expect(rateLabels.every((label) => /^\d+(\.\d+)?$/.test(label)), JSON.stringify(rateLabels)).toBe(true)
            const rateTitle = (await rateCard.locator('svg text[text-anchor="start"]').first().textContent())?.trim() ?? ''
            expect(rateTitle, JSON.stringify(rateLabels)).toMatch(/^Speed \([KMGT]?i?B\/s\)$/)
            // The crosshair tooltip states each sample with its own instant; a
            // percentage is not pre-rounded onto the card's one-decimal reading.
            const cpuPlot = metrics.getByRole("article").filter({ has: page.getByRole("heading", { level: 3, name: "Process CPU" }) }).locator('[data-slot="node-metric-chart"]')
            // A real pointer gesture: centre the plot, leave it, then cross into
            // it. A bare jump can resolve before the deck has stopped moving, and
            // the crosshair then stays untriggered.
            const cpuPoint = await centredPlotPoint(page, cpuPlot, 'Process CPU')
            await page.mouse.move(2, 2)
            await page.mouse.move(cpuPoint.x, cpuPoint.y, { steps: 4 })
            // Scoped to this card: every mounted chart owns a tooltip element, and
            // only the hovered one is shown.
            const tooltip = cpuPlot.locator(".node-metric-chart-tooltip")
            await expect(tooltip).toBeVisible()
            await expect(tooltip).toContainText(/\d{2}:\d{2}:\d{2}/)
            await expect(tooltip).toContainText("Process CPU")
            await expect(tooltip).toContainText(/%/)
            // Issue #249: the Host network tooltip reads its samples on the same
            // binary ladder as the card header and the `Speed (…)` axis title —
            // a number below 1024 in B/s, KiB/s, MiB/s, GiB/s or TiB/s, so a
            // sample under 1 KiB stays in B/s — never as a bare byte count such
            // as `151618 B/s`, and never in a unit above the one the axis
            // title chose from the window's peak.
            const ratePoint = await centredPlotPoint(page, rateCard, 'Host network')
            await page.mouse.move(2, 2)
            await page.mouse.move(ratePoint.x, ratePoint.y, { steps: 4 })
            const rateTooltip = rateCard.locator('.node-metric-chart-tooltip')
            await expect(rateTooltip).toBeVisible()
            await expect(rateTooltip).toContainText(/\d{2}:\d{2}:\d{2}/)
            const rateText = (await rateTooltip.textContent()) ?? ''
            const rateUnits = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
            const axisUnit = rateUnits.indexOf(/^Speed \(([KMGT]?i?B)\/s\)$/.exec(rateTitle)?.[1] ?? '')
            expect(axisUnit, rateTitle).toBeGreaterThanOrEqual(0)
            const readings = [...rateText.matchAll(/(\d+(?:\.\d+)?) ([A-Za-z]+)\/s/g)]
            expect(readings.length, rateText).toBeGreaterThan(0)
            for (const [reading, number, unit] of readings) {
              expect(rateUnits, reading).toContain(unit)
              expect(Number(number), reading).toBeLessThan(1024)
              expect(rateUnits.indexOf(unit), reading + ' under ' + rateTitle).toBeLessThanOrEqual(axisUnit)
            }
            await page.mouse.move(2, 2)
            // The retained-sample explanation opposite the section label is gone.
            await expect(page.getByText(/Real retained samples/)).toHaveCount(0)
            // Multi-series cards embed native ECharts legends matching Emerald specifications,
            // centered at the bottom of the canvas with roundRect icons and no separate DOM legend slot.
            for (const [heading, labels] of [
              ['Host network', ['Download', 'Upload']],
              ['Peer connections', ['Inbound', 'Outbound']],
            ] as const) {
              const card = metrics.getByRole('article').filter({ has: page.getByRole('heading', { level: 3, name: heading }) })
              await expect(card.locator('[data-slot="node-metric-legend"]')).toHaveCount(0)
              const chartSvg = card.locator('[data-slot="node-metric-chart"] svg')
              for (const label of labels) {
                await expect(chartSvg.locator('text').filter({ hasText: label }).first()).toBeVisible()
              }
            }
            // The seeded Node's newest block observation is 20s old against a
            // 2s cadence, so the two bar cards name the unobserved tail instead
            // of drawing bars up to now. The line cards measure the same tail
            // against their own 35s cadence, so their verdict depends on how
            // long this run has been going and is asserted as a shape only.
            const blockCard = metrics.getByRole('article').filter({ has: page.getByRole('heading', { level: 3, name: 'Block interval' }) })
            // No card writes the verdict as small print under its title any more
            // — asserted once for every scenario below the scenario chain — so the
            // sentence is read here from the chart's accessible description.
            await expect(blockCard.locator('[data-slot="node-metric-chart"]')).toHaveAccessibleDescription(/No samples for the last /)
          }
          if (scenario === 'unknown') {
            await expect(page.getByLabel('Node key summary').getByText('Unknown').first()).toBeVisible()
            await expect(metrics.locator('[data-slot="node-metric-chart-empty"]')).toHaveCount(6)
            await expect(metrics.locator('[data-slot="node-metric-value"]').first()).toHaveText('Unknown')
          }
          if (scenario === 'stale') {
            await expect(page.getByText('Latest observation is stale')).toBeVisible()
            await expect(metrics.locator('[data-slot="node-metric-chart-empty"]')).toHaveCount(6)
          }
          if (scenario === 'single-cpu-failure') {
            const cpuCard = metrics.getByRole('article').filter({ has: page.getByRole('heading', { level: 3, name: 'Process CPU' }) })
            await expect(cpuCard.locator('[data-slot="node-metric-chart-empty"]')).toHaveCount(1)
            await expect(cpuCard.locator('[data-slot="node-metric-chart"]')).toHaveAttribute('data-runs', '0')
            await expect(cpuCard.locator('[data-slot="node-metric-value"]')).toHaveText(/%$/)
            await expect(cpuCard.getByText(/last-good value retained/)).toBeVisible()
            const hostCard = metrics.getByRole('article').filter({ has: page.getByRole('heading', { level: 3, name: 'Host network' }) })
            await expect(hostCard.locator('[data-slot="node-metric-chart"]')).toHaveAttribute('data-runs', '2')
            await expect(hostCard.locator('[data-slot="node-metric-chart-empty"]')).toHaveCount(0)
          }
          if (scenario === 'gap') {
            // A silence is not a line: the curve breaks between the two
            // stretches and neither side is extended into the unobserved tail,
            // so the right edge of the window stays blank.
            const cpuCard = metrics.getByRole('article').filter({ has: page.getByRole('heading', { level: 3, name: 'Process CPU' }) })
            await expect(cpuCard.locator('[data-slot="node-metric-chart-empty"]')).toHaveCount(0)
            const cpuPlot = cpuCard.locator('[data-slot="node-metric-chart"]')
            await expect(cpuPlot).toHaveAttribute('data-runs', '2')
            // Every one of the six observations is handed to the renderer, none
            // across the silence and none into the unobserved tail. That each
            // run lands exactly on its own sample instants, and that no vertex
            // drops to the baseline inside the hole, is proven on the real
            // renderer in src/components/nodeMetricChart.test.ts.
            await expect(cpuPlot).toHaveAttribute('data-points', '6')
            await expect(cpuPlot).toHaveAccessibleDescription(/1 gap in this window/)
          }
          if (scenario === 'never-observed') {
            // Never observed is its own statement, never a flat line at zero.
            await expect(metrics.locator('[data-slot="node-metric-chart-empty"]')).toHaveCount(6)
            await expect(metrics.locator('[data-slot="node-metric-chart"][data-points="0"]')).toHaveCount(6)
            await expect(metrics.locator('[data-slot="node-metric-chart"][data-runs="0"]')).toHaveCount(6)
            // Four cards hold ledgered series, where an absence is proof that
            // nobody ever reported them. The two block cards have no ledger, so
            // their absence is an unknown the Server states as such — asserted
            // card by card, so no card can drop the sentence or carry both.
            await expectAbsenceVerdicts(metrics, {
              'Process CPU': 'never-observed',
              'Process memory': 'never-observed',
              'Host network': 'never-observed',
              'Peer connections': 'never-observed',
              'Block interval': 'no-ledger',
              'Transactions / block': 'no-ledger',
            })
          }
          if (scenario === 'expired') {
            // Samples retention removed are not evidence that nobody ever
            // observed the series: the ledger still dates it, so the card names
            // the age of the last observation and the chart stays empty.
            await expect(metrics.locator('[data-slot="node-metric-chart-empty"]')).toHaveCount(6)
            await expect(metrics.locator('[data-slot="node-metric-chart"][data-points="0"]')).toHaveCount(6)
            await expect(metrics.locator('[data-slot="node-metric-chart"][data-runs="0"]')).toHaveCount(6)
            // Four cards hold ledgered series: their last observation is dated
            // 40 days back and the window holds none of them. The two block
            // cards have no ledger, so their expiry is not dated at all and is
            // answered as an unknown rather than as never observed.
            await expectAbsenceVerdicts(metrics, {
              'Process CPU': 'dated',
              'Process memory': 'dated',
              'Host network': 'dated',
              'Peer connections': 'dated',
              'Block interval': 'no-ledger',
              'Transactions / block': 'no-ledger',
            })
          }

          // The coverage verdict is not visible small print on any of the six
          // cards any more, in any scenario. It is still a sentence: it rides the
          // chart's accessible description, which the never-observed and expired
          // scenarios above read back one card at a time.
          await expect(metrics.locator('[data-slot="node-metric-coverage"]')).toHaveCount(0)

          // The remaining disclosure is keyboard-operable in both directions, and
          // the retired Peer diagnostics region no longer renders.
          await expect(page.getByText('Peer diagnostics')).toHaveCount(0)
          const diagnosticsDisclosure = page.locator('details[data-slot="disclosure"]', { hasText: 'Validator diagnostics' })
          const diagnosticsSummary = diagnosticsDisclosure.locator('summary').first()
          await diagnosticsSummary.focus()
          await page.keyboard.press('Enter')
          await expect(diagnosticsDisclosure).toHaveAttribute('open', '')
          await diagnosticsSummary.focus()
          await page.keyboard.press('Enter')
          await expect(diagnosticsDisclosure).not.toHaveAttribute('open', '')

          // Accessibility, touch targets, and no page-level horizontal scroll.
          await expectVisibleInteractiveTargets(page)
          await expectNoHorizontalOverflow(page)
          await expect(page.locator('[data-slot="background-decoration-grid"]')).toHaveCount(1)

          // The fixed-position background is only correct from the top of the
          // page, so scroll there before the full-page evidence screenshot.
          if (scenario === 'normal' && theme === 'light') {
            await page.evaluate(() => window.scrollTo(0, 0))
            await page.screenshot({ path: testInfo.outputPath('node-detail-six-charts.png'), fullPage: true })
          }

          // Read-only cards retain their translucent surface on pointer hover.
          const chartCard = metrics.locator('[data-slot="node-metric-card"]').first()
          await page.mouse.move(2, 2)
          const restingShadow = await chartCard.evaluate((card) => getComputedStyle(card).boxShadow)
          const restingBackground = await chartCard.evaluate((card) => getComputedStyle(card).backgroundColor)
          await chartCard.hover()
          await page.waitForTimeout(220)
          const hovered = await chartCard.evaluate((card) => ({
            shadow: getComputedStyle(card).boxShadow,
            transform: getComputedStyle(card).transform,
          }))
          expect(
            hovered.transform === 'none' || hovered.transform === 'matrix(1, 0, 0, 1, 0, 0)',
            'the six-chart deck never moves on hover',
          ).toBe(true)
          expect(hovered.shadow, 'chart cards stay shadow-free').toBe(restingShadow)
          await expect(chartCard).toHaveCSS('box-shadow', 'none')
          await expect(chartCard).toHaveCSS('background-color', restingBackground)
          for (const card of await page.locator('[data-slot="node-summary-tile"], [data-slot="node-info-group"], [data-slot="disclosure"], [data-slot="linked-validator"]').all()) {
            await page.mouse.move(2, 2)
            const background = await card.evaluate((element) => getComputedStyle(element).backgroundColor)
            await card.hover()
            await expect(card).toHaveCSS('background-color', background)
          }

          // The observation panels and the diagnostic disclosures are the
          // other card classes on this page; they stay static in both themes.
          const observationPanel = page.locator('[data-slot="node-info-group"]').first()
          await page.mouse.move(2, 2)
          const panelResting = await observationPanel.evaluate((card) => getComputedStyle(card).boxShadow)
          await observationPanel.hover()
          await page.waitForTimeout(220)
          expect(await observationPanel.evaluate((card) => getComputedStyle(card).boxShadow), 'observation panels stay shadow-free').toBe(panelResting)
          await expect(observationPanel).toHaveCSS('box-shadow', 'none')
          await expect(observationPanel).toHaveCSS('transform', 'none')


          // Desktop 1280 lays the six charts out three columns by two rows.
          if (viewportWidth >= 1280) {
            const rows = await metrics.locator('[data-slot="node-metric-card"]').evaluateAll((cards) => cards.map((card) => {
              const box = card.getBoundingClientRect()
              return { top: Math.round(box.top), left: Math.round(box.left) }
            }))
            const tops = [...new Set(rows.map((row) => row.top))]
            expect(tops, JSON.stringify(rows)).toHaveLength(2)
            expect(rows.filter((row) => row.top === tops[0])).toHaveLength(3)
            expect(rows.filter((row) => row.top === tops[1])).toHaveLength(3)
          }
        })
      }
    }
  })

  test('compares Head only with current Server evidence and keeps absolute chain heights', async ({ page }) => {
    let mode: 'current' | 'unconfirmed' | 'low-confidence' = 'current'
    await page.route(nodeRoute, async route => {
      const response = await route.fetch()
      const body = await response.json()
      await route.fulfill({ response, json: { ...body,
        freshness: new Date().toISOString(), health: mode === 'unconfirmed' ? 'unknown' : 'healthy',
        rpcState: 'ok', currentHead: 100, networkReferenceHead: 98,
        networkReferenceConfidence: mode === 'low-confidence' ? 'low' : 'high',
        consensus: { ...body.consensus, state: 'ok', freshness: 'current',
          highestQcBlock: 102, highestLockBlock: 101, highestCommitBlock: 100 },
      } })
    })
    await loginAs(page)
    await openNodeDetail(page)
    const chain = page.getByRole('region', { name: 'Node chain state' })
    const head = page.getByLabel('Node key summary').locator('[data-slot="node-summary-tile"]').filter({ has: page.getByText('Head', { exact: true }) })
    const qc = chain.locator('[data-slot="metric-row"]').filter({ has: page.getByText('QC Head', { exact: true }) })
    await expect(head).toContainText('2 blocks ahead')
    await expect(qc).toContainText('102')
    mode = 'unconfirmed'
    await page.reload()
    await expect(head).not.toContainText('2 blocks ahead')
    await expect(qc).toContainText('102')
    mode = 'low-confidence'
    await page.reload()
    await expect(head).not.toContainText('2 blocks ahead')
    await expect(qc).toContainText('102')
    await expect(chain).not.toContainText('At network head')
    await expectNoHorizontalOverflow(page)
  })

  test('role changes preserve the shared header, KPI, map, panels and chart geometry', async ({ page }) => {
    await forceFragileHeaderFont(page)
    let role: 'validator' | 'not_validator' | 'unknown' = 'validator'
    await page.route(nodeRoute, async route => {
      const response = await route.fetch()
      const body = await response.json()
      await route.fulfill({ response, json: {
        ...body, health: 'healthy', healthReason: '', freshness: new Date().toISOString(),
        rpcState: 'ok', syncState: 'ok', processState: 'ok', lastReportAt: new Date().toISOString(),
        currentHead: 100, networkReferenceHead: 100, networkReferenceConfidence: 'high',
        validator: {
          ...body.validator, validatorId: 'layout-validator', validatorNodeId: '0x' + 'a'.repeat(128),
          nodeId: PUBLIC_NODE_ID, state: 'fresh', freshness: 'current', source: 'platscan',
          activity: role === 'validator' ? 'active' : 'observing', activityState: 'current',
          currentValidatorStatus: role, currentValidatorStatusState: role === 'unknown' ? 'unknown' : 'current',
          blockCount: 34509, rewardAmount: '320404.1586', blockRateState: 'ok', blockRate: '100.026',
          counterState: 'normal', rank: 151, rankState: 'ranked', rankFreshness: 'current',
        },
      } })
    })
    await page.route(metricsRoute, async route => {
      const response = await route.fetch()
      const body = await response.json()
      await route.fulfill({ response, json: { ...body,
        processCpuPercent: [{ sampledAt: body.from, value: 0.3 }, { sampledAt: body.to, value: 0.4 }],
        processMemoryPercent: [{ sampledAt: body.from, value: 2 }, { sampledAt: body.to, value: 2.1 }],
      } })
    })
    await loginAs(page)
    let baseline: Awaited<ReturnType<typeof geometry>> | undefined
    async function geometry() {
      return page.evaluate(() => {
        const rect = (selector: string) => {
          const element = document.querySelector(selector)
          if (!element) throw new Error('Missing shared layout element: ' + selector)
          const box = element.getBoundingClientRect()
          return { top: Math.round(box.top + scrollY), left: Math.round(box.left), width: Math.round(box.width), height: Math.round(box.height) }
        }
        return {
          summary: rect('[aria-label="Node key summary"]'),
          map: rect('[data-slot="node-map"]'),
          panels: Array.from(document.querySelectorAll('[data-slot="node-info-group"]')).map(element => {
            const box = element.getBoundingClientRect()
            return { top: Math.round(box.top + scrollY), left: Math.round(box.left), width: Math.round(box.width), height: Math.round(box.height) }
          }),
          charts: Array.from(document.querySelectorAll('[data-slot="node-metric-card"]')).map(element => {
            const box = element.getBoundingClientRect()
            return { width: Math.round(box.width), height: Math.round(box.height) }
          }),
        }
      })
    }
    for (const next of ['validator', 'not_validator', 'unknown', 'validator'] as const) {
      role = next
      await openNodeDetail(page)
      const header = page.locator('header[aria-labelledby="node-detail-title"]')
      await expect(header.getByRole('button', { name: 'Copy full Node ID' })).toBeVisible()
      await expect(header.getByText(PUBLIC_NODE_ID, { exact: true })).toBeVisible()
      await expect(header.getByText('Observer', { exact: true })).toHaveCount(role === 'not_validator' ? 1 : 0)
      await expect(header.getByText('Validator', { exact: true })).toHaveCount(role === 'validator' ? 1 : 0)
      const performance = page.getByRole('region', { name: 'Validator performance' })
      await expect(performance).toHaveCount(role === 'validator' ? 1 : 0)
      await expect(page.getByText('Validator diagnostics', { exact: true })).toHaveCount(role === 'validator' ? 1 : 0)
      if (role === 'validator') {
        await expect(header.getByText('Active', { exact: true })).toBeVisible()
        await expect(performance.locator('header').getByText('Active', { exact: true })).toHaveCount(0)
      }
      const summary = page.getByRole('group', { name: 'Node key summary' })
      expect(await summary.locator('[data-slot="node-summary-tile"] [data-slot="card-x-content"] > div:first-child > span:first-child').allTextContents()).toEqual(['Head', 'Sync collection', 'Peers', 'Uptime', 'Block interval', 'Transactions / block'])
      for (const name of ['Node chain state', 'PlatON process', 'Shared Host resources']) {
        await expect(page.getByRole('region', { name })).toBeVisible()
      }
      const process = page.getByRole('region', { name: 'PlatON process' })
      await expect(process.getByText('Collection', { exact: true })).toBeVisible()
      await expect(process.getByText('Successful', { exact: true })).toBeVisible()
      await expect(process.getByText('Current', { exact: true })).toHaveCount(0)
      await expect(process.getByText('Running', { exact: true })).toHaveCount(0)
      const metrics = page.locator('[data-slot="node-metrics-section"]')
      expect(await metrics.locator('[data-slot="node-metric-card"] h3').allTextContents()).toEqual([...CHART_HEADINGS])
      for (const [heading, axisTitle] of [['Process CPU', 'CPU (%)'], ['Process memory', 'Memory (%)']] as const) {
        const card = metrics.getByRole('article').filter({ has: page.getByRole('heading', { name: heading, exact: true }) })
        const chart = card.locator('[data-slot="node-metric-chart"]')
        await expect(chart).toHaveAttribute('data-runs', '1')
        // The ceiling still comes from the window's own peak (5 for these
        // samples), but it is printed as a bare tick under a title that states
        // the unit, not as a second '%' the header already carries. Both are
        // read through retrying locator assertions: sampling the SVG once can
        // run before ECharts has painted the axis on a slower project.
        await expect(chart.locator('svg text[text-anchor="end"]'), heading).toHaveText(['0', '1', '2', '3', '4', '5'])
        await expect(chart.locator('svg text[text-anchor="start"]').first(), heading).toHaveText(axisTitle)
      }
      const current = await geometry()
      if (baseline) expect(current, role + ' keeps the same shared layout geometry').toEqual(baseline)
      else baseline = current
      await expectNoHorizontalOverflow(page)
    }
  })

  test('reduced motion removes the disclosure and chart transitions', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' })
    await loginAs(page)
    await openNodeDetail(page)
    // Chromium reports a removed transition as a near-zero duration
    // ("1e-05s") rather than the literal "0s".
    const transition = await page.locator('[data-slot="disclosure-marker"]').first().evaluate((element) => getComputedStyle(element).transitionDuration)
    expect(parseFloat(transition)).toBeLessThan(0.001)
    const chartTransition = await page.locator('[data-slot="node-metric-card"]').first().evaluate((card) => getComputedStyle(card).transitionDuration)
    expect(parseFloat(chartTransition)).toBeLessThan(0.001)
    await expectNoHorizontalOverflow(page)
  })

  test('the breadcrumb keeps public Node Detail navigation intact', async ({ page }) => {
    await loginAs(page)
    await openNodeDetail(page)
    // The deleted Network overview route is not linked any more: the breadcrumb
    // names its destination and returns Home.
    const breadcrumb = page.locator('[data-slot="node-detail-breadcrumb"] a')
    await expect(breadcrumb).toHaveText(/Back to Home/)
    await breadcrumb.click()
    await expect(page).toHaveURL(/\/$/)
    await expect(page.getByRole('region', { name: 'Home' })).toBeVisible({ timeout: 15_000 })
    await page.goBack()
    await expect(page.getByRole('heading', { level: 1, name: PUBLIC_NODE_NAME })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByRole('heading', { level: 2, name: 'Latest 60 seconds' })).toBeVisible()
  })
  test('the Node Peer Country View fills the first row across the fixed viewports', async ({ page }, testInfo) => {
    // Home's states, word for word, on the Node scope: the same uncarded map
    // over this Node's current Peer records, never a Node deployment map.
    const scenarios = [
      {
        mode: 'current',
        geo: {
          state: 'current', scope: 'complete',
          countries: [{ countryCode: 'SE', count: 1, staleCount: 0, centroidLat: 59.33, centroidLon: 18.06 }],
          knownCountryCount: 1, unknownCountryCount: 2,
          unknownWithPublicIpCount: 0, unknownWithoutRemoteIpCount: 2,
          availablePeerCount: 3,
        },
        notice: null as string | null,
        counters: '3 Peer records in scope for Node A, 2 with no retained country attribution: 2 without a usable public remote IP',
      },
      {
        mode: 'stale',
        geo: {
          state: 'stale', scope: 'complete',
          countries: [{ countryCode: 'SE', count: 1, staleCount: 1, centroidLat: 59.33, centroidLon: 18.06 }],
          knownCountryCount: 1, unknownCountryCount: 2,
          unknownWithPublicIpCount: 0, unknownWithoutRemoteIpCount: 2,
          availablePeerCount: 3,
        },
        notice: 'Map data stale',
        counters: '3 Peer records in scope for Node A, 2 with no retained country attribution: 2 without a usable public remote IP',
      },
      {
        mode: 'all-unknown',
        geo: {
          state: 'current', scope: 'complete', countries: [],
          knownCountryCount: 0, unknownCountryCount: 3,
          unknownWithPublicIpCount: 3, unknownWithoutRemoteIpCount: 0,
          availablePeerCount: 3,
        },
        notice: 'No locations to show',
        counters: '3 Peer records in scope for Node A, 3 with no retained country attribution: 3 without a retained country result',
      },
      {
        mode: 'unobserved',
        geo: {
          state: 'current', scope: 'unobserved', countries: [],
          knownCountryCount: null, unknownCountryCount: null,
          unknownWithPublicIpCount: null, unknownWithoutRemoteIpCount: null,
          availablePeerCount: null,
        },
        notice: 'No observations yet',
        counters: null,
      },
      {
        mode: 'disabled',
        geo: {
          state: 'disabled', scope: 'unavailable', countries: null,
          knownCountryCount: null, unknownCountryCount: null,
          unknownWithPublicIpCount: null, unknownWithoutRemoteIpCount: null,
          availablePeerCount: null,
        },
        notice: 'Peer countries · Disabled by server',
        counters: null,
      },
    ]
    let scenario = scenarios[0]
    await page.route(nodeRoute, async (route) => {
      const response = await route.fetch()
      const body = (await response.json()) as Record<string, unknown>
      // Pin the Peer Count tile and the map to the same figure so the rendered
      // agreement is deterministic. The Server-side invariant (both derive from
      // COUNT(*) of current_node_peers) is pinned by the Rust projection test.
      body.peers = { ...(body.peers as Record<string, unknown>), peerCount: 3, inboundCount: 2, outboundCount: 1 }
      body.geo = scenario.geo
      await route.fulfill({ response, json: body })
    })
    await loginAs(page)

    for (const step of scenarios) {
      scenario = step
      await openNodeDetail(page)
      const map = page.getByRole('region', { name: 'Node Peer countries' })
      await expect(map).toBeVisible({ timeout: 15_000 })
      // `data-state` is the Server's Geo database state; the Node scope is
      // `data-scope` (complete or unobserved, never partial).
      await expect(map).toHaveAttribute('data-state', step.geo.state)
      await expect(map).toHaveAttribute('data-scope', step.geo.scope)
      // A Node scope is not Home's Network selection, so the Home data hook
      // is absent rather than repurposed.
      expect(await map.getAttribute('data-network-filter')).toBeNull()
      if (step.notice) await expect(map.getByRole('status')).toContainText(step.notice)

      const layout = await page.evaluate(() => {
        const row = document.querySelector('[data-slot="node-overview"]') as HTMLElement
        const summary = row.querySelector('[aria-label="Node key summary"]') as HTMLElement
        const slot = row.querySelector('[data-slot="node-map"]') as HTMLElement
        const rect = (element: HTMLElement) => {
          const box = element.getBoundingClientRect()
          return { top: box.top + window.scrollY, left: box.left, width: box.width, height: box.height }
        }
        return {
          summary: rect(summary),
          map: rect(slot),
          mapBackground: getComputedStyle(slot).backgroundColor,
          mapCardish: /(^|\s)bg-/.test(slot.className),
        }
      })
      const viewportWidth = page.viewportSize()?.width ?? 0
      expect(layout.map.height, step.mode + ' keeps the map track').toBeGreaterThan(0)
      if (viewportWidth >= 1024) {
        // From lg the tiles and the wider map share one row, tiles left.
        expect(layout.map.left, step.mode).toBeGreaterThan(layout.summary.left)
        expect(layout.map.width / layout.summary.width, step.mode + ' uses 5:6 tracks').toBeCloseTo(6 / 5, 1)
        // Home parity: the map band sets the row height and the content-height
        // KPI grid sits on the band's floor (items-end), so the two columns
        // share a bottom edge while the taller map may start above the tiles.
        expect(layout.map.top, step.mode + ' keeps the map at or above the KPI grid').toBeLessThanOrEqual(layout.summary.top + 1)
        expect(
          Math.abs((layout.map.top + layout.map.height) - (layout.summary.top + layout.summary.height)),
          step.mode + ' bottom-aligns the KPI grid with the map band',
        ).toBeLessThanOrEqual(1)
        // Home parity: proportional 2:1 below xl, the fixed 22rem band from xl.
        if (viewportWidth >= 1280) {
          expect(Math.round(layout.map.height), step.mode + ' keeps the fixed 22rem band').toBe(352)
        } else {
          expect(Math.abs(layout.map.height - layout.map.width / 2), step.mode + ' keeps the 2:1 track').toBeLessThanOrEqual(1)
        }
      } else {
        // Below lg the map leads the band and the six tiles follow it, the
        // same visual order Home uses; the DOM order stays tiles-then-map.
        expect(layout.map.top + layout.map.height, step.mode).toBeLessThanOrEqual(layout.summary.top + 1)
        expect(Math.abs(layout.map.left - layout.summary.left), step.mode).toBeLessThanOrEqual(1)
      }
      // Uncarded: the map introduces neither a fourth card surface nor a
      // fourth reading tier (design §11.1).
      expect(layout.mapBackground, step.mode).toBe('rgba(0, 0, 0, 0)')
      expect(layout.mapCardish, step.mode).toBe(false)

      if (step.counters) {
        await expect(map.getByRole('note')).toHaveAttribute('aria-label', step.counters)
      }
      if (step.mode === 'current' || step.mode === 'stale') {
        await expect(map.locator('[data-slot="geo-country-list"]')).toContainText('Sweden')
      }
      if (step.mode === 'current') {
        // The map denominator and the Peer Count tile are one Server figure.
        await expect(page.locator('[data-slot="node-summary-tile"]').filter({ hasText: 'Peers' }).locator('strong')).toHaveText('3')
      }
      await expectNoHorizontalOverflow(page)

      // Evidence: the first row at this fixed viewport in both themes, so the
      // map track, the uncarded surface and the tile/ map rhythm can be
      // checked against the approved composition.
      await page.evaluate(() => window.scrollTo(0, 0))
      await setTheme(page, 'light')
      await page.screenshot({ path: testInfo.outputPath('node-peer-country-view-' + step.mode + '-light.png'), fullPage: true })
      await setTheme(page, 'dark')
      await page.screenshot({ path: testInfo.outputPath('node-peer-country-view-' + step.mode + '-dark.png'), fullPage: true })
    }
  })
  test('an unusable basemap keeps the Node map track and says so', async ({ page }) => {
    // The committed world geometry is a static asset; a Server that cannot
    // serve it must degrade inside the map slot, not take the page down.
    await page.route('**/assets/geo/world-countries-echarts-www-v1.json', (route) => route.abort())
    await page.route(nodeRoute, async (route) => {
      const response = await route.fetch()
      const body = (await response.json()) as Record<string, unknown>
      body.geo = {
        state: 'current', scope: 'complete',
        countries: [{ countryCode: 'SE', count: 1, staleCount: 0, centroidLat: 59.33, centroidLon: 18.06 }],
        knownCountryCount: 1, unknownCountryCount: 0,
        unknownWithPublicIpCount: 0, unknownWithoutRemoteIpCount: 0,
        availablePeerCount: 1,
      }
      await route.fulfill({ response, json: body })
    })
    await loginAs(page)
    await openNodeDetail(page)
    const map = page.getByRole('region', { name: 'Node Peer countries' })
    await expect(map.getByRole('status')).toContainText('Map unavailable', { timeout: 15_000 })
    // The failed basemap keeps the track: the slot never collapses and no
    // canvas is created in place of the world.
    const track = page.locator('[data-slot="node-map"]')
    expect((await track.boundingBox())?.height ?? 0).toBeGreaterThan(0)
    await expect(map.locator('[data-slot="geo-chart"]')).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })
})

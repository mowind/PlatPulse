import { expect, test, type Page } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

/**
 * Issue #247 (ADR 0012, docs/design/webui.md §15.38): the Node Detail Metric
 * Window. The existing six-chart matrix keeps running on Live
 * (node-detail-six-charts.spec.ts); this file adds one `normal` scenario for
 * each longer window in both themes, plus a bucket `gap` and an `Open bucket`
 * scenario for 24h, across the five fixed projects. The buckets are the ones
 * the harness writes the way ingestion does (e2e/metric_buckets.py), read back
 * through the real Server; only the gap scenario rewrites one series.
 */

const PUBLIC_NODE_NAME = 'Node A'
const PUBLIC_NODE_ID = '0195f2a1-0014-4014-8014-000000000014'
const THEMES = ['light', 'dark'] as const
type Theme = (typeof THEMES)[number]

const WINDOWS = [
  { window: '1h', title: 'Latest hour', span: 'the last hour', axis: /\d{2}:\d{2}:\d{2}/, bucketed: false },
  { window: '6h', title: 'Latest 6 hours', span: 'the last 6 hours', axis: /\d{2}:\d{2}/, bucketed: true },
  { window: '24h', title: 'Latest 24 hours', span: 'the last 24 hours', axis: /\d{2}:\d{2}/, bucketed: true },
  { window: '7d', title: 'Latest 7 days', span: 'the last 7 days', axis: /\d{2}-\d{2} \d{2}:\d{2}/, bucketed: true },
] as const

const metricsOf = (window: string) => (url: URL) =>
  url.pathname === '/api/public/v1/nodes/' + PUBLIC_NODE_ID + '/metrics' && url.searchParams.get('window') === window

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

/** Open Node Detail from Home, then choose the window with the selector. */
async function openWindow(page: Page, window: string) {
  await page.goto('/')
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible({ timeout: 15_000 })
  await page.getByRole('link', { name: new RegExp(PUBLIC_NODE_NAME) }).first().click()
  await expect(page.getByRole('heading', { level: 1, name: PUBLIC_NODE_NAME })).toBeVisible({ timeout: 15_000 })
  await expect(page.getByRole('heading', { level: 2, name: 'Latest 60 seconds' })).toBeVisible()
  await page.getByRole('radio', { name: window, exact: true }).click()
  await expect(page).toHaveURL(new RegExp('[?&]window=' + window + '(&|$)'))
}

/**
 * Hover a plot with a real pointer gesture, at a fraction of its grid width or,
 * with `'end'`, one pixel inside the grid's right edge, where the axis tooltip
 * snaps to the newest point.
 */
async function hoverPlot(page: Page, heading: string, fraction: number | 'end') {
  const plot = page.locator('[data-slot="node-metrics-section"]').getByRole('article')
    .filter({ has: page.getByRole('heading', { level: 3, name: heading }) })
    .locator('[data-slot="node-metric-chart"]')
  // The chart mounts once its lazy engine has loaded; hovering before that
  // would hit an empty container.
  await expect(plot).toHaveAttribute('data-runs', /^[1-9]/)
  await expect(plot.locator('svg path').first()).toBeAttached()
  await plot.evaluate((node) => node.scrollIntoView({ block: 'center' }))
  const box = await plot.boundingBox()
  if (!box) throw new Error(heading + ' plot has no box')
  // The grid keeps 56px on the left and 24px on the right of the plot.
  const x = fraction === 'end' ? box.x + box.width - 24 - 2 : box.x + 56 + (box.width - 56 - 24) * fraction
  const y = box.y + box.height / 2
  await page.mouse.move(2, 2)
  await page.mouse.move(x, y, { steps: 4 })
  return plot.locator('.node-metric-chart-tooltip')
}

test.describe('Node Detail Metric Window (issue #247)', () => {
  test('each longer window in both themes', async ({ page }) => {
    test.setTimeout(240_000)
    await loginAs(page)
    for (const spec of WINDOWS) {
      for (const theme of THEMES) {
        await test.step(spec.window + ' / ' + theme, async () => {
          await openWindow(page, spec.window)
          await setTheme(page, theme)
          const metrics = page.locator('[data-slot="node-metrics-section"]')
          await expect(metrics.getByRole('heading', { level: 2, name: spec.title })).toBeVisible()
          const selector = metrics.getByRole('radiogroup', { name: 'Metric window' })
          await expect(selector.getByRole('radio', { checked: true })).toHaveText(spec.window)

          // Four sample charts at the window's own span, the two block cards Live only.
          const lines = metrics.getByRole('img', { name: new RegExp('line chart over ' + spec.span + '$') })
          await expect(lines).toHaveCount(4)
          await expect(metrics.getByRole('img', { name: /bar chart/ })).toHaveCount(0)
          await expect(metrics.locator('[data-slot="node-metric-chart-live-only"]')).toHaveCount(2)
          await expect(metrics.getByText('Block charts show the Live window only')).toHaveCount(2)
          for (const heading of ['Block interval', 'Transactions / block']) {
            const card = metrics.getByRole('article').filter({ has: page.getByRole('heading', { level: 3, name: heading }) })
            await expect(card.locator('[data-slot="node-metric-value"]')).not.toHaveText('')
          }
          await expect(metrics.locator('[data-chart-error]')).toHaveCount(0)
          await expect(metrics.locator('[data-slot="node-metric-chart-empty"]')).toHaveCount(0)
          const drawn = await metrics.locator('[data-slot="node-metric-chart"]').evaluateAll((nodes) => nodes.map((node) => ({
            points: Number(node.getAttribute('data-points') ?? 0),
            shapes: Array.from(node.querySelectorAll('svg path')).filter((path) => (path.getAttribute('d') ?? '').startsWith('M')).length,
            axis: node.querySelector('svg')?.textContent ?? '',
          })))
          expect(drawn, JSON.stringify(drawn)).toHaveLength(4)
          for (const plot of drawn) {
            expect(plot.points, JSON.stringify(drawn)).toBeGreaterThan(0)
            expect(plot.shapes, JSON.stringify(drawn)).toBeGreaterThan(0)
            expect(plot.axis, JSON.stringify(drawn)).toMatch(spec.axis)
            if (spec.window === '6h' || spec.window === '24h') {
              // HH:mm, never seconds (a date is added only across midnight).
              expect(plot.axis, JSON.stringify(drawn)).not.toMatch(/\d{2}:\d{2}:\d{2}/)
            }
          }
          // Plot heights are the deck's own.
          const viewportWidth = page.viewportSize()?.width ?? 0
          const expectedPlotHeight = viewportWidth >= 1024 ? 192 : viewportWidth >= 768 ? 168 : 132
          for (const plot of await metrics.locator('[data-slot="node-metric-chart"]').all()) {
            await expect(plot).toHaveCSS('height', expectedPlotHeight + 'px')
          }
          for (const notice of await metrics.locator('[data-slot="node-metric-chart-live-only"]').all()) {
            await expect(notice).toHaveCSS('height', expectedPlotHeight + 'px')
          }

          // The selector: one row of 44px segments, beside the title from md and
          // on its own row under it on phones, never scrolling sideways.
          const geometry = await metrics.evaluate((section) => {
            const title = section.querySelector('h2')?.getBoundingClientRect()
            const group = section.querySelector('[role="radiogroup"]')?.getBoundingClientRect()
            const radios = Array.from(section.querySelectorAll('[role="radio"]')).map((radio) => {
              const box = radio.getBoundingClientRect()
              return { top: Math.round(box.top), height: box.height, width: box.width }
            })
            return { title: title && { top: title.top, bottom: title.bottom }, group: group && { top: group.top, bottom: group.bottom, width: group.width }, radios }
          })
          expect(geometry.radios, JSON.stringify(geometry)).toHaveLength(5)
          expect(new Set(geometry.radios.map((radio) => radio.top)).size, JSON.stringify(geometry)).toBe(1)
          for (const radio of geometry.radios) {
            expect(radio.height, JSON.stringify(geometry)).toBeGreaterThanOrEqual(44)
            expect(radio.width, JSON.stringify(geometry)).toBeGreaterThanOrEqual(44)
          }
          if (!geometry.title || !geometry.group) throw new Error('missing header geometry')
          if (viewportWidth >= 768) {
            expect(geometry.group.top, JSON.stringify(geometry)).toBeLessThan(geometry.title.bottom)
          } else {
            expect(geometry.group.top, JSON.stringify(geometry)).toBeGreaterThanOrEqual(geometry.title.bottom - 1)
          }
          await expectNoHorizontalOverflow(page)

          // The tooltip states the point's own instant; a bucket states its
          // interval, its extremes and its sample count.
          const tooltip = await hoverPlot(page, 'Process CPU', 0.5)
          await expect(tooltip).toBeVisible()
          await expect(tooltip).toContainText('Process CPU')
          if (spec.bucketed) {
            await expect(tooltip).toContainText(' – ')
            await expect(tooltip).toContainText('(max)')
            await expect(tooltip).toContainText(/min [\d.]+% · max [\d.]+% · \d+ samples/)
          } else {
            await expect(tooltip).toContainText(/\d{2}:\d{2}:\d{2}/)
            await expect(tooltip).not.toContainText('samples')
          }

          // Refresh restores the window from the URL.
          await page.reload()
          await expect(page.getByRole('heading', { level: 2, name: spec.title })).toBeVisible({ timeout: 15_000 })
          await expect(page.getByRole('radio', { name: spec.window, exact: true })).toHaveAttribute('aria-checked', 'true')
        })
      }
    }
  })

  test('24h: a missing bucket is a gap, and the newest bucket is open', async ({ page }) => {
    test.setTimeout(120_000)
    await loginAs(page)
    for (const theme of THEMES) {
      await test.step('gap / ' + theme, async () => {
        // Drop the middle third of the Process CPU buckets and state the gap
        // the Server's long-history rule names for exactly that hole.
        await page.route(metricsOf('24h'), async (route) => {
          const response = await route.fetch()
          const body = (await response.json()) as Record<string, unknown>
          const points = body.processCpuPercent as Array<{ sampledAt: string; bucket: { lastObservedAt: string; firstObservedAt: string } }>
          const from = Math.floor(points.length / 3)
          const to = Math.floor((points.length * 2) / 3)
          const before = points[from - 1]
          const after = points[to]
          body.processCpuPercent = [...points.slice(0, from), ...points.slice(to)]
          const series = body.series as Array<Record<string, unknown>>
          body.series = series.map((entry) => entry.metric === 'processCpuPercent' && before && after
            ? {
                ...entry,
                gaps: [{
                  from: before.bucket.lastObservedAt,
                  to: after.bucket.firstObservedAt,
                  seconds: Math.round((Date.parse(after.bucket.firstObservedAt) - Date.parse(before.bucket.lastObservedAt)) / 1000),
                  kind: 'collection_gap',
                }],
              }
            : entry)
          await route.fulfill({ response, json: body })
        })
        await openWindow(page, '24h')
        await setTheme(page, theme)
        const metrics = page.locator('[data-slot="node-metrics-section"]')
        const cpu = metrics.getByRole('img', { name: 'Process CPU line chart over the last 24 hours' })
        // Two runs either side of the hole: nothing joined, nothing zero-filled.
        await expect(cpu).toHaveAttribute('data-runs', '2')
        await expect(cpu).toHaveAccessibleDescription(/1 gap in this window/)
        const memory = metrics.getByRole('img', { name: 'Process memory line chart over the last 24 hours' })
        await expect(memory).toHaveAttribute('data-runs', '1')
        await expectNoHorizontalOverflow(page)
        await page.unroute(metricsOf('24h'))
      })
      await test.step('open bucket / ' + theme, async () => {
        // The Server itself marks the newest 5-minute bucket open. The fixture
        // refresher writes the current bucket every 25s, so just after a bucket
        // edge the newest one may still be the closed previous bucket; wait for
        // the real answer to carry an open one.
        type Point = { sampledAt: string; bucket?: { open: boolean; firstObservedAt: string; lastObservedAt: string } }
        const newestOpen = async () => {
          const response = await page.request.get('/api/public/v1/nodes/' + PUBLIC_NODE_ID + '/metrics?window=24h')
          const body = (await response.json()) as { processCpuPercent?: Point[] }
          return body.processCpuPercent?.at(-1)?.bucket?.open === true
        }
        await expect.poll(newestOpen, { timeout: 45_000, intervals: [2_000] }).toBe(true)
        // A 5-minute bucket is about one pixel wide on a phone's 24h axis, so
        // the pointer cannot pick it out of its neighbours. Keep two of the
        // Server's own buckets - one from the window's first quarter and the
        // open newest one - so each is the only point under the pointer.
        await page.route(metricsOf('24h'), async (route) => {
          const response = await route.fetch()
          const body = (await response.json()) as Record<string, unknown>
          const points = body.processCpuPercent as Point[]
          const older = points[Math.floor(points.length / 4)]
          const newest = points.at(-1)
          body.processCpuPercent = [older, newest].filter((point) => point !== undefined)
          const series = body.series as Array<Record<string, unknown>>
          // The buckets dropped between them are a silence, stated as the gap the
          // Server would name, so the two are never joined into one line.
          const gap = older?.bucket && newest?.bucket
            ? [{
                from: older.bucket.lastObservedAt,
                to: newest.bucket.firstObservedAt,
                seconds: Math.round((Date.parse(newest.bucket.firstObservedAt) - Date.parse(older.bucket.lastObservedAt)) / 1000),
                kind: 'collection_gap',
              }]
            : []
          body.series = series.map((entry) => entry.metric === 'processCpuPercent' ? { ...entry, gaps: gap } : entry)
          await route.fulfill({ response, json: body })
        })
        await openWindow(page, '24h')
        await setTheme(page, theme)
        await expect(page.getByRole('img', { name: 'Process CPU line chart over the last 24 hours' })).toHaveAttribute('data-runs', '2')
        const tooltip = await hoverPlot(page, 'Process CPU', 'end')
        await expect(tooltip).toBeVisible()
        await expect(tooltip).toContainText('Open bucket')
        await expect(tooltip).toContainText(/\d+ samples?/)
        // An older bucket is closed.
        const older = await hoverPlot(page, 'Process CPU', 0.25)
        await expect(older).toBeVisible()
        await expect(older).not.toContainText('Open bucket')
        await page.unroute(metricsOf('24h'))
      })
    }
  })
})

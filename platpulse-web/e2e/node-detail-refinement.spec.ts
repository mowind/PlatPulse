import { expect, test, type Locator, type Page } from '@playwright/test'
import { expectNoHorizontalOverflow, expectVisibleInteractiveTargets, loginAs } from './helpers'

/**
 * Node Detail closing refinement: the live chart interaction keeps a reader's
 * legend choice, a failed history refresh keeps the previous window and says so
 * without passing it off as the current minute, and the static definitions that
 * used to sit as small print open on demand by touch and keyboard.
 *
 * The realtime stream is replaced by a controlled EventSource, so each refresh
 * is one deliberate invalidation of the Node rather than whatever the seeded
 * Server happens to publish.
 */

const PUBLIC_NODE_NAME = 'Node A'
const PUBLIC_NODE_ID = '0195f2a1-0014-4014-8014-000000000014'
const nodeRoute = '**/api/public/v1/nodes/' + PUBLIC_NODE_ID
const metricsRoute = nodeRoute + '/metrics'
const UPLOAD = '#a78bfa'
const DOWNLOAD = '#60a5fa'
const INBOUND = '#ff6b6b'
const OUTBOUND = '#4ecdc4'

async function installControlledRealtime(page: Page) {
  await page.addInitScript(() => {
    type Handler = (event: { data?: string }) => void
    class ControlledEventSource {
      static instances: ControlledEventSource[] = []
      handlers = new Map<string, Handler[]>()
      onopen: (() => void) | null = null
      onerror: (() => void) | null = null
      constructor() {
        ControlledEventSource.instances.push(this)
        queueMicrotask(() => this.onopen?.())
      }
      addEventListener(type: string, handler: Handler) {
        this.handlers.set(type, [...(this.handlers.get(type) ?? []), handler])
      }
      removeEventListener(type: string, handler: Handler) {
        this.handlers.set(type, (this.handlers.get(type) ?? []).filter((candidate) => candidate !== handler))
      }
      close() {}
      emit(type: string, data?: string) {
        for (const handler of this.handlers.get(type) ?? []) handler({ data })
      }
    }
    Object.defineProperty(window, 'EventSource', { configurable: true, value: ControlledEventSource })
    Object.defineProperty(window, '__emitRealtime', { configurable: true, value: (type: string, data?: string) => {
      for (const source of ControlledEventSource.instances) source.emit(type, data)
    } })
  })
}

let revision = 10

/** One authoritative refresh of this Node: Node, block history and metrics. */
async function refreshNode(page: Page) {
  revision += 1
  await page.evaluate((payload) => {
    const emit = (window as typeof window & { __emitRealtime: (type: string, data?: string) => void }).__emitRealtime
    emit('invalidation', JSON.stringify(payload))
  }, { resource: 'node', resourceId: PUBLIC_NODE_ID, revision, eventId: revision })
}

async function setTheme(page: Page, theme: 'light' | 'dark') {
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

function chartIn(page: Page, heading: string): Locator {
  return page.locator('[data-slot="node-metric-card"]')
    .filter({ has: page.getByRole('heading', { level: 3, name: heading, exact: true }) })
    .locator('[data-slot="node-metric-chart"]')
}

/** Drawn curves of one colour: stroked, unfilled paths (legend swatches are filled). */
async function curveCount(chart: Locator, color: string): Promise<number> {
  return chart.evaluate((element, wanted) => Array.from(element.querySelectorAll('svg path')).filter((path) => {
    const stroke = (path.getAttribute('stroke') ?? '').toLowerCase()
    const fill = path.getAttribute('fill')
    return stroke === wanted && (fill === null || fill === 'none' || fill === 'transparent')
  }).length, color)
}

async function clickLegend(chart: Locator, label: string) {
  await chart.evaluate((element) => element.scrollIntoView({ block: 'center' }))
  const text = chart.locator('svg text', { hasText: new RegExp('^' + label + '$') })
  await expect(text).toHaveCount(1)
  // ECharts may repaint the SVG between lookups (a live refresh or resize),
  // detaching the element just found, so read the box until it is stable.
  let box: { x: number, y: number, width: number, height: number } | null = null
  await expect.poll(async () => (box = await text.boundingBox({ timeout: 1_000 }).catch(() => null)) !== null).toBe(true)
  await chart.page().mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2)
  // Leave the legend for an empty corner of the same chart: hovering an item
  // emphasises its series in a lighter stroke until the pointer moves off it.
  const corner = (await chart.boundingBox())!
  await chart.page().mouse.move(corner.x + 2, corner.y + 2, { steps: 3 })
}

async function openNodeDetail(page: Page) {
  await page.goto('/nodes/' + PUBLIC_NODE_ID)
  await expect(page.getByRole('heading', { level: 1, name: PUBLIC_NODE_NAME })).toBeVisible({ timeout: 15_000 })
  await expect(page.locator('[data-slot="node-metric-chart"] svg')).toHaveCount(6)
}

test.describe('Node Detail closing refinement', () => {
  test('keeps hidden legend directions hidden through refreshes, resizes and theme changes', async ({ page }) => {
    await installControlledRealtime(page)
    let metricsCalls = 0
    await page.route(metricsRoute, async (route) => {
      metricsCalls += 1
      await route.continue()
    })
    await loginAs(page)
    await openNodeDetail(page)
    const network = chartIn(page, 'Host network')
    const peers = chartIn(page, 'Peer connections')
    await network.scrollIntoViewIfNeeded()
    await expect.poll(() => curveCount(network, UPLOAD)).toBeGreaterThan(0)
    await expect.poll(() => curveCount(peers, OUTBOUND)).toBeGreaterThan(0)

    await clickLegend(network, 'Upload')
    await clickLegend(peers, 'Outbound')
    const expectChoiceKept = async (when: string) => {
      await expect.poll(() => curveCount(network, UPLOAD), when + ': Upload stays hidden').toBe(0)
      await expect.poll(() => curveCount(network, DOWNLOAD), when + ': Download stays drawn').toBeGreaterThan(0)
      await expect.poll(() => curveCount(peers, OUTBOUND), when + ': Outbound stays hidden').toBe(0)
      await expect.poll(() => curveCount(peers, INBOUND), when + ': Inbound stays drawn').toBeGreaterThan(0)
    }
    await expectChoiceKept('after the click')

    for (let round = 1; round <= 3; round += 1) {
      const before = metricsCalls
      await refreshNode(page)
      await expect.poll(() => metricsCalls, 'refresh ' + round + ' refetched the metric window').toBeGreaterThan(before)
      await page.waitForTimeout(300)
      await expectChoiceKept('refresh ' + round)
    }

    const viewport = page.viewportSize()!
    await page.setViewportSize({ width: viewport.width >= 1024 ? 900 : viewport.width + 120, height: viewport.height })
    await page.waitForTimeout(300)
    await expectChoiceKept('resize')
    await page.setViewportSize(viewport)
    await page.waitForTimeout(300)
    await expectChoiceKept('resize back')

    await setTheme(page, 'dark')
    await expectChoiceKept('dark theme')
    await setTheme(page, 'light')
    await expectChoiceKept('light theme')

    // The legend still answers: showing Upload again draws it on the next frame.
    await clickLegend(network, 'Upload')
    await expect.poll(() => curveCount(network, UPLOAD)).toBeGreaterThan(0)
    await refreshNode(page)
    await page.waitForTimeout(300)
    await expect.poll(() => curveCount(network, UPLOAD)).toBeGreaterThan(0)
    await expect.poll(() => curveCount(peers, OUTBOUND), 'the other chart keeps its own choice').toBe(0)
  })

  test('keeps the previous window after a failed history refresh and clears the notice on recovery', async ({ page }) => {
    await installControlledRealtime(page)
    let failMetrics = false
    await page.route(metricsRoute, async (route) => {
      if (!failMetrics) return route.continue()
      await route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ error: { code: 'unavailable', message: 'history failed' } }) })
    })
    await loginAs(page)
    await openNodeDetail(page)
    const section = page.locator('[data-slot="node-metrics-section"]')
    const notice = section.locator('[data-slot="node-metrics-refresh-error"]')
    await expect(section.getByRole('heading', { level: 2, name: 'Latest 60 seconds', exact: true })).toBeVisible()
    await expect(notice).toHaveCount(0)
    const network = chartIn(page, 'Host network')
    const pointsBefore = await network.getAttribute('data-points')

    failMetrics = true
    await refreshNode(page)
    await expect(notice).toHaveText('History update failed · Showing previous window')
    await expect(section.getByRole('heading', { level: 2, name: 'Retained 60-second window', exact: true })).toBeVisible()
    await expect(section.getByRole('heading', { level: 2, name: 'Latest 60 seconds' })).toHaveCount(0)
    // The retained curves are the same samples, still drawn, and named as retained.
    await expect(network).toHaveAttribute('data-points', pointsBefore ?? '')
    await expect(page.getByRole('img', { name: /chart over a retained 60-second window/ })).toHaveCount(6)
    const windowButton = section.getByRole('button', { name: 'Retained window times' })
    await windowButton.click()
    await expect(page.getByRole('dialog', { name: 'Retained window times' })).toContainText(/From .+ UTC to .+ UTC/)
    await page.keyboard.press('Escape')
    await expect(page.getByRole('dialog', { name: 'Retained window times' })).toHaveCount(0)
    await expect(windowButton).toBeFocused()
    await expectNoHorizontalOverflow(page)

    failMetrics = false
    await refreshNode(page)
    await expect(notice).toHaveCount(0)
    await expect(section.getByRole('heading', { level: 2, name: 'Latest 60 seconds', exact: true })).toBeVisible()
    await expect(page.getByRole('img', { name: /chart over the last 60 seconds/ })).toHaveCount(6)
  })

  test('keeps a first history failure distinct from a retained window', async ({ page }) => {
    await page.route(metricsRoute, (route) => route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ error: { code: 'unavailable', message: 'history failed' } }) }))
    await loginAs(page)
    await page.goto('/nodes/' + PUBLIC_NODE_ID)
    await expect(page.getByRole('heading', { level: 1, name: PUBLIC_NODE_NAME })).toBeVisible({ timeout: 15_000 })
    const section = page.locator('[data-slot="node-metrics-section"]')
    await expect(section.locator('[data-slot="node-metric-card"]').filter({ hasText: 'Metric history unavailable' })).toHaveCount(6)
    await expect(section.locator('[data-slot="node-metrics-refresh-error"]')).toHaveCount(0)
    await expect(section.getByRole('heading', { level: 2, name: 'Latest 60 seconds', exact: true })).toBeVisible()
  })

  test('opens static definitions by touch or keyboard and keeps the labels short', async ({ page, isMobile }, testInfo) => {
    const touch = Boolean(testInfo.project.use.hasTouch || isMobile)
    await loginAs(page)
    await openNodeDetail(page)
    // The standing small print is gone; the labels carry the meaning.
    for (const gone of ['Data collection state; not chain sync', 'Last collection result; not a live process status',
      'Collected once per Agent; shared by every Node it monitors', 'Directory size against its filesystem capacity, not whole-Host disk usage']) {
      await expect(page.getByText(gone, { exact: true })).toHaveCount(0)
    }
    const syncTile = page.locator('[data-slot="node-summary-tile"]').filter({ hasText: 'Sync collection' })
    await expect(syncTile).toHaveCount(1)
    await expect(page.getByText('Data directory', { exact: true })).toBeVisible()

    const hints: Array<[string, RegExp]> = [
      ['About Sync collection', /not a chain-sync verdict/i],
      ['About Collection', /not a live Running or Stopped/],
      ['About Data usage', /not whole-Host disk usage/],
      ['About Host resources', /once per Agent/],
    ]
    for (const [name, text] of hints) {
      const button = page.getByRole('button', { name, exact: true })
      await button.scrollIntoViewIfNeeded()
      if (touch) await button.tap()
      else { await button.focus(); await page.keyboard.press('Enter') }
      const dialog = page.getByRole('dialog', { name, exact: true })
      await expect(dialog).toBeVisible()
      await expect(dialog).toContainText(text)
      await expect(button).toHaveAttribute('aria-expanded', 'true')
      const box = (await dialog.boundingBox())!
      const viewport = page.viewportSize()!
      expect(box.x, name + ' stays inside the viewport').toBeGreaterThanOrEqual(0)
      expect(box.x + box.width).toBeLessThanOrEqual(viewport.width + 1)
      if (touch) {
        // A tap outside dismisses it.
        await page.getByRole('heading', { level: 1, name: PUBLIC_NODE_NAME }).tap()
      } else {
        await page.keyboard.press('Escape')
        await expect(button).toBeFocused()
      }
      await expect(dialog).toHaveCount(0)
      await expect(button).toHaveAttribute('aria-expanded', 'false')
    }
    await expectVisibleInteractiveTargets(page)
    await expectNoHorizontalOverflow(page)
  })

  test('captures the refined Node Detail in both themes', async ({ page }, testInfo) => {
    await loginAs(page)
    for (const theme of ['light', 'dark'] as const) {
      await openNodeDetail(page)
      await setTheme(page, theme)
      const widths = testInfo.project.name === 'desktop-1440' ? [1440, 1920] : [page.viewportSize()!.width]
      for (const width of widths) {
        await page.setViewportSize({ width, height: width >= 1024 ? 900 : page.viewportSize()!.height })
        const summary = page.locator('summary').filter({ hasText: 'Validator diagnostics' })
        const disclosure = summary.locator('..')
        if ((await disclosure.getAttribute('open')) === null) await summary.click()
        await expect(disclosure.locator('[data-slot="validator-diagnostics-group"]')).toHaveCount(3)
        await page.waitForTimeout(250)
        await expectNoHorizontalOverflow(page)
        const shots: Array<[string, Locator]> = [
          ['node-detail', page.locator('main')],
          ['validator-diagnostics', disclosure],
          ['validator-performance', page.getByRole('region', { name: 'Validator performance' })],
        ]
        for (const [name, locator] of shots) {
          const path = testInfo.outputPath(name + '-' + theme + '-' + width + '.png')
          await locator.screenshot({ path })
          await testInfo.attach(name + '-' + theme + '-' + width + '.png', { path, contentType: 'image/png' })
        }
      }
    }
  })
})

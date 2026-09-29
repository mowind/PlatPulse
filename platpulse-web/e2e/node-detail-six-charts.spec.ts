import { expect, test, type Page } from '@playwright/test'
import { expectNoHorizontalOverflow, expectVisibleInteractiveTargets, loginAs } from './helpers'

/**
 * Issue #150 final Node Detail acceptance: the fixed four Playwright projects
 * multiplied by both themes and the normal / unknown / stale / single-CPU
 * failure scenarios exercise the six-chart continuous reading page. The
 * metrics response is fetched from the real Server and only the targeted
 * dimension is rewritten, so retained samples stay real.
 */

const PUBLIC_NODE_NAME = 'Node A'
const PUBLIC_NODE_ID = '0195f2a1-0014-4014-8014-000000000014'
const CHART_HEADINGS = [
  'Process CPU',
  'Process memory',
  'Host network',
  'Peer connections',
  'Block interval',
  'Transactions per block',
] as const
const SCENARIOS = ['normal', 'unknown', 'stale', 'single-cpu-failure'] as const
const THEMES = ['light', 'dark'] as const

type Scenario = (typeof SCENARIOS)[number]
type Theme = (typeof THEMES)[number]

const nodeRoute = '**/api/public/v1/nodes/' + PUBLIC_NODE_ID
const metricsRoute = nodeRoute + '/metrics'

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
            // The Validator diagnostics are one of the same shared disclosures but
            // they sit inside the Linked Validator card, so the reading-order
            // assertion names the page-level one it means.
            const disclosureTop = (title: string) => {
              const found = Array.from(document.querySelectorAll('details[data-slot="disclosure"]'))
                .find(element => element.querySelector('summary')?.textContent?.includes(title))
              return found ? found.getBoundingClientRect().top + window.scrollY : Number.NaN
            }
            return {
              identity: top('[data-slot="node-identity-main"]'),
              summary: top('[aria-label="Node key summary"]'),
              metrics: top('[data-slot="node-metrics-section"]'),
              technical: disclosureTop('Identifiers and technical details'),
            }
          })
          expect(order.identity, JSON.stringify(order)).toBeLessThan(order.summary)
          expect(order.summary, JSON.stringify(order)).toBeLessThan(order.metrics)
          expect(order.metrics, JSON.stringify(order)).toBeLessThan(order.technical)

          // The accepted A container (issue #151): an uncarded identity block,
          // four summary tiles, and three parallel observation panels that sit
          // on one row at lg and stack to one column below 1024px.
          await expect(page.locator('[data-slot="node-hero-card"]')).toHaveCount(0)
          await expect(page.getByLabel('Node key summary').locator('[data-slot="node-summary-tile"]')).toHaveCount(4)
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
          const expectedPlotHeight = (page.viewportSize()?.width ?? 0) >= 1024 ? 100 : 116
          for (const svg of await metrics.locator('svg[role="img"]').all()) {
            await expect(svg).toHaveCSS('height', expectedPlotHeight + 'px')
            const plotRow = await svg.evaluate((element) => getComputedStyle(element.parentElement!).gridTemplateRows.split(' ')[0])
            expect(plotRow).toBe(expectedPlotHeight + 'px')
          }

          if (scenario === 'normal') {
            await expect(metrics.locator('[data-slot="node-metric-chart-empty"]')).toHaveCount(0)
            // Process CPU + process memory + two direction pairs = six lines.
            await expect(metrics.locator('[data-slot="node-metric-chart-line"]')).toHaveCount(6)
            // The retained-sample explanation opposite the section label is gone.
            await expect(page.getByText(/Real retained samples/)).toHaveCount(0)
            // Curve legends sit under their plots, never in the header, so the
            // header keeps title-left / current-value-right on every card.
            for (const [heading, legend] of [
              ['Host network', 'Host network chart legend'],
              ['Peer connections', 'Peer connections chart legend'],
            ] as const) {
              const card = metrics.getByRole('article').filter({ has: page.getByRole('heading', { level: 3, name: heading }) })
              await expect(card.locator('[data-slot="node-metric-header"] [data-slot="node-metric-legend"]')).toHaveCount(0)
              await expect(card.locator('[data-slot="node-metric-legend"] [aria-label="' + legend + '"]')).toHaveCount(1)
            }
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
            await expect(cpuCard.locator('[data-slot="node-metric-chart-line"]')).toHaveCount(0)
            await expect(cpuCard.locator('[data-slot="node-metric-value"]')).toHaveText(/%$/)
            await expect(cpuCard.getByText(/last-good value retained/)).toBeVisible()
            const hostCard = metrics.getByRole('article').filter({ has: page.getByRole('heading', { level: 3, name: 'Host network' }) })
            await expect(hostCard.locator('[data-slot="node-metric-chart-line"]')).toHaveCount(2)
            await expect(hostCard.locator('[data-slot="node-metric-chart-empty"]')).toHaveCount(0)
          }

          // Disclosure is keyboard-operable in both directions, and the retired
          // Peer diagnostics region no longer renders.
          await expect(page.getByText('Peer diagnostics')).toHaveCount(0)
          const technicalDisclosure = page.locator('details[data-slot="disclosure"]', { hasText: 'Identifiers and technical details' })
          const technicalSummary = technicalDisclosure.locator('summary')
          await technicalSummary.focus()
          await page.keyboard.press('Enter')
          await expect(technicalDisclosure).toHaveAttribute('open', '')
          await technicalSummary.focus()
          await page.keyboard.press('Enter')
          await expect(technicalDisclosure).not.toHaveAttribute('open', '')

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

          // Low-frequency technical details open by keyboard and stay in the
          // Public Projection.
          const technicalDisclosure = page.locator('details[data-slot="disclosure"]', { hasText: 'Identifiers and technical details' })
          await technicalDisclosure.locator('summary').focus()
          await page.keyboard.press('Enter')
          await expect(technicalDisclosure).toHaveAttribute('open', '')
          await expect(technicalDisclosure.getByText('Reference confidence')).toBeVisible()
          await page.keyboard.press('Enter')
          await expect(technicalDisclosure).not.toHaveAttribute('open', '')

          // Desktop 1280 lays the six charts out three columns by two rows.
          const viewportWidth = page.viewportSize()?.width ?? 0
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

  test('uses timestamp-shaped Public freshness and Server evidence for independent height deltas', async ({ page }) => {
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
    const headDelta = page.locator('[data-slot="head-delta"]')
    // Each height names its own reference, and an exact match is a state rather
    // than a zero delta.
    const heightOffset = (name: string) => page.getByRole('group', { name: name + ' height' }).locator('[data-slot="height-offset"]')
    await expect(headDelta).toHaveText('+2 from Observed Network Head')
    await expect(heightOffset('QC')).toHaveText('+2 from Node Head')
    await expect(heightOffset('Locked')).toHaveText('+1 from Node Head')
    await expect(heightOffset('Committed')).toHaveText('At Node Head')
    mode = 'unconfirmed'
    await page.reload()
    await expect(headDelta).toContainText('—')
    await expect(headDelta).toContainText('Current Node Head not confirmed by Server')
    await expect(page.getByRole('group', { name: 'QC height' })).toContainText('102')
    await expect(heightOffset('QC')).toHaveText('—')
    mode = 'low-confidence'
    await page.reload()
    await expect(headDelta).toContainText('Observed Network Head confidence low')
    await expect(heightOffset('QC')).toHaveText('+2 from Node Head')
    await expectNoHorizontalOverflow(page)
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
    // is named "All Networks" and returns Home.
    const breadcrumb = page.locator('[data-slot="node-detail-breadcrumb"] a')
    await expect(breadcrumb).toHaveText(/All Networks/)
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
        counters: '3 Peer records in scope for Node A, 2 with no retained country attribution',
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
        counters: '3 Peer records in scope for Node A, 2 with no retained country attribution',
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
        counters: '3 Peer records in scope for Node A, 3 with no retained country attribution',
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
        expect(layout.map.width, step.mode).toBeGreaterThan(layout.summary.width)
      } else {
        // Below lg the map sits above the tiles, both full width.
        expect(layout.map.top, step.mode).toBeLessThan(layout.summary.top)
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

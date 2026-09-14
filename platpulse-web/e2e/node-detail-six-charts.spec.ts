import { expect, test, type Page } from '@playwright/test'
import { expectNoHorizontalOverflow, expectVisibleInteractiveTargets, loginAs, expectComputedColor } from './helpers'

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
            return {
              identity: top('[data-slot="node-identity-main"]'),
              summary: top('[aria-label="Node key summary"]'),
              metrics: top('[data-slot="node-metrics-section"]'),
              peer: top('details[data-slot="node-disclosure"]'),
            }
          })
          expect(order.identity, JSON.stringify(order)).toBeLessThan(order.summary)
          expect(order.summary, JSON.stringify(order)).toBeLessThan(order.metrics)
          expect(order.metrics, JSON.stringify(order)).toBeLessThan(order.peer)

          // The accepted A container (issue #151): an uncarded identity block,
          // four summary tiles, and three parallel observation panels that sit
          // on one row at desktop width and stack to one column at <=48rem.
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

          if (scenario === 'normal') {
            await expect(metrics.locator('[data-slot="node-metric-chart-empty"]')).toHaveCount(0)
            // Process CPU + process memory + two direction pairs = six lines.
            await expect(metrics.locator('[data-slot="node-metric-chart-line"]')).toHaveCount(6)
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

          // Disclosure is keyboard-operable in both directions.
          const peerDisclosure = page.locator('details[data-slot="node-disclosure"]', { hasText: 'Peer diagnostics' })
          const peerSummary = peerDisclosure.locator('summary')
          await peerSummary.focus()
          await page.keyboard.press('Enter')
          await expect(peerDisclosure).toHaveAttribute('open', '')
          await page.keyboard.press('Enter')
          await expect(peerDisclosure).not.toHaveAttribute('open', '')

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

          // Emerald detail cards become opaque without a glow or lift.
          const hoverCapable = await page.evaluate(
            () => matchMedia('(hover: hover) and (pointer: fine)').matches,
          )
          const chartCard = metrics.locator('[data-slot="node-metric-card"]').first()
          await page.mouse.move(2, 2)
          const restingShadow = await chartCard.evaluate((card) => getComputedStyle(card).boxShadow)
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
          if (hoverCapable) {
            await expectComputedColor(chartCard, 'background-color', theme === 'light'
              ? 'rgb(255, 255, 255)' : 'oklch(0.141 0.005 285.823)')
          }

          // The observation panels and the diagnostic disclosures are the
          // other card classes on this page; they react in both themes too.
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
          const technicalDisclosure = page.locator('details[data-slot="node-disclosure"]', { hasText: 'Identifiers and technical details' })
          await technicalDisclosure.locator('summary').focus()
          await page.keyboard.press('Enter')
          await expect(technicalDisclosure).toHaveAttribute('open', '')
          await expect(technicalDisclosure.getByText('Reference confidence')).toBeVisible()
          await page.keyboard.press('Enter')
          await expect(technicalDisclosure).not.toHaveAttribute('open', '')

          // Desktop 1280 lays the six charts out three columns by two rows.
          const viewportWidth = page.viewportSize()?.width ?? 0
          if (viewportWidth >= 1024) {
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

  test('reduced motion removes the disclosure and chart transitions', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' })
    await loginAs(page)
    await openNodeDetail(page)
    // Chromium reports a removed transition as a near-zero duration
    // ("1e-05s") rather than the literal "0s".
    const transition = await page.locator('[data-slot="node-disclosure-marker"]').first().evaluate((element) => getComputedStyle(element).transitionDuration)
    expect(parseFloat(transition)).toBeLessThan(0.001)
    const chartTransition = await page.locator('[data-slot="node-metric-card"]').first().evaluate((card) => getComputedStyle(card).transitionDuration)
    expect(parseFloat(chartTransition)).toBeLessThan(0.001)
    await expectNoHorizontalOverflow(page)
  })

  test('the breadcrumb keeps public Node Detail navigation intact', async ({ page }) => {
    await loginAs(page)
    await openNodeDetail(page)
    await page.locator('[data-slot="node-detail-breadcrumb"] a').click()
    await expect(page).toHaveURL(/\/networks\//)
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible({ timeout: 15_000 })
    await page.goBack()
    await expect(page.getByRole('heading', { level: 1, name: PUBLIC_NODE_NAME })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByRole('heading', { level: 2, name: 'Latest 60 seconds' })).toBeVisible()
  })
})

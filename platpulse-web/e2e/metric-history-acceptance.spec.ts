import { expect, test, type Page } from '@playwright/test'
import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

import type { AdminNodeMetricHistoryResponse } from '../src/api/generated'
import {
  VIEWPORTS,
  expectLocalTableScroll,
  expectResolvedTheme,
  focusByKeyboard,
  gotoAuthenticated,
  loginToDisposableServer,
} from './admin-flow'
import {
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
} from './helpers'
import { startDisposableServer, type DisposableServer } from './server-harness'

/**
 * Raw 24 hour Node metric history acceptance (issue #213, design §11.4).
 *
 * Each test owns one throwaway Server, port, and temporary SQLite database and
 * drives the production WebUI build against real Server HTTP: no mock, no
 * test-only seam, and no jsdom. Every observation is stored by submitting a
 * real Agent Report, so the history the Owner reads is the history the Server
 * actually wrote, and the silence in the middle of the window is a real pause
 * the capacity protection recorded — never a fabricated zero or a line drawn
 * across a stretch nobody observed.
 */

/**
 * The largest floor the Server accepts: the thresholds are stored in SQLite, so
 * the ceiling is `i64::MAX`. It stays a string because it sits one past
 * `Number.MAX_SAFE_INTEGER`, and as a JavaScript number it would round up to a
 * floor the Server rejects.
 */
const MAX_PERSISTED_BYTES = '9223372036854775807'

/** A floor every real filesystem clears, so protection stays released. */
const CLEARED_FLOOR = 1

/** The canonical fixture supplies the healthy Node process probe the minimal
 * one disables; without it the report carries no Node metric history at all.
 * The report itself is the minimal fixture, so the Node it declares is the
 * minimal one's: the two fixtures describe different Nodes. */
const CANONICAL_FIXTURE = '../crates/platpulse-core/tests/fixtures/report_v1_canonical.json'
const REPORT_FIXTURE = '../crates/platpulse-core/tests/fixtures/report_v1_minimal.json'

const MINUTE_SECONDS = 60
const HOUR_SECONDS = 3600

function enableNodeProcess(report: Record<string, unknown>) {
  const canonical = JSON.parse(readFileSync(CANONICAL_FIXTURE, 'utf8')) as {
    nodes: { process: unknown }[]
  }
  const nodes = report.nodes as Record<string, unknown>[]
  nodes[0].process = canonical.nodes[0].process
}

/** Canonical second-precision instant, seconds before now, matching the
 * Server's own timestamp format. */
function canonicalAgo(seconds: number): string {
  const instant = Math.floor(Date.now() / 1000) * 1000 - seconds * 1000
  return new Date(instant).toISOString().replace(/\.\d{3}Z$/, 'Z')
}

/**
 * One genuinely new Report whose Node process observation is stamped at a real
 * instant relative to now. The fixture's own timestamps are fixed calendar
 * values, and an observation stamped there would land outside the one day raw
 * window; the report id and sequence move so the Server stores a new
 * observation instead of answering an exact replay from its receipt.
 */
function reportObservedAt(observed: string, sequence: number) {
  return (report: Record<string, unknown>) => {
    enableNodeProcess(report)
    const nodes = report.nodes as Record<string, unknown>[]
    const component = nodes[0].process as Record<string, unknown>
    component.attempted_at = observed
    component.latest_observed_at = observed
    report.report_sequence = sequence
    report.report_id = '0195f2a1-0091-4091-8091-0000000000' + String(sequence).padStart(2, '0')
  }
}

function fixtureNodeId(): string {
  const report = JSON.parse(readFileSync(REPORT_FIXTURE, 'utf8')) as {
    nodes: { node_id: string }[]
  }
  return report.nodes[0].node_id
}

/**
 * Declare a new low-space floor in the Server's configuration file. The
 * capacity policy is read at startup, so the Server is restarted on the same
 * state directory: a floor no real filesystem clears adopts protection, and a
 * cleared floor releases it. This is the Operator action the issue describes
 * and it never pokes the database behind the running Server's back.
 */
function declareFloor(stateDir: string, floorBytes: number | string) {
  const path = join(stateDir, 'server.toml')
  const lines = readFileSync(path, 'utf8').split('\n')
  const start = lines.indexOf('[capacity]')
  expect(start, 'the harness declares a [capacity] section').toBeGreaterThan(-1)
  let end = start + 1
  while (end < lines.length && !lines[end].startsWith('[')) end += 1
  lines.splice(
    start,
    end - start,
    '[capacity]',
    'enabled = true',
    'pause_below_bytes = ' + floorBytes,
    'resume_above_bytes = ' + floorBytes,
    'sample_interval_seconds = 5',
  )
  writeFileSync(path, lines.join('\n'))
}

/** Read the Owner-only raw history for one series over real HTTP. */
async function readHistory(
  server: DisposableServer,
  nodeId: string,
  metric: string,
  range = '',
): Promise<AdminNodeMetricHistoryResponse> {
  const response = await server.adminGet(
    '/api/admin/v1/nodes/' + nodeId + '/metric-history?metric=' + metric + range,
  )
  expect(response.status).toBe(200)
  return response.body as AdminNodeMetricHistoryResponse
}

/**
 * The plot must never draw a line across a silence the Server reported: every
 * drawn stretch stays entirely on one side of the band. The observations are
 * split at the reported silence before they are folded into columns, and each
 * column sits where its own observations are, so a stretch only has to stay
 * outside the band. The tolerance absorbs the two decimals a drawn path is
 * written with, nothing more.
 */
async function expectNoLineCrossesTheGap(page: Page) {
  const crossings = await page.evaluate(() => {
    const band = document.querySelector('[data-slot="metric-history-gap-band"]')
    if (!band) return -1
    const start = Number(band.getAttribute("x"))
    const width = Number(band.getAttribute("width"))
    const tolerance = 0.005
    const lines = Array.from(document.querySelectorAll('[data-slot="metric-history-line"]'))
    return lines.filter((line) => {
      const path = line.getAttribute("d") ?? ""
      const xs = Array.from(path.matchAll(/[ML] ([0-9.]+) /g)).map((match) => Number(match[1]))
      if (xs.length === 0) return false
      const min = Math.min(...xs)
      const max = Math.max(...xs)
      return max > start + tolerance && min < start + width - tolerance
    }).length
  })
  expect(crossings, "no drawn line may cross the reported silence").toBe(0)
}
test.describe('Raw Node metric history (issue #213)', () => {
  test('reads a stored 24 hour series, reports the protection pause in the middle of it, and never fills the silence in', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the process-heavy flow runs once')
    test.setTimeout(600_000)
    const server = await startDisposableServer({
      capacity: { pauseBelowBytes: CLEARED_FLOOR, resumeAboveBytes: CLEARED_FLOOR, sampleIntervalSeconds: 5 },
    })
    try {
      const nodeId = fixtureNodeId()
      const context = await browser.newContext()
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        const agent = await server.enrollAgent()

        // One stored observation before the pause. Every instant is captured
        // once and reused, so an assertion never races the clock past a second.
        const observed = {
          beforePause: canonicalAgo(3 * HOUR_SECONDS),
          afterPause: canonicalAgo(20 * MINUTE_SECONDS),
          newest: canonicalAgo(10 * MINUTE_SECONDS),
        }
        await server.submitReport(agent.agentId, agent.credential, reportObservedAt(observed.beforePause, 1))

        // The Operator declares a floor this deployment cannot clear. The two
        // Reports submitted under protection are still accepted and still
        // commit their core projection: only optional history is paused, and
        // every skipped observation is recorded as a pause.
        declareFloor(server.stateDir, MAX_PERSISTED_BYTES)
        await server.restart()
        await server.submitReport(agent.agentId, agent.credential, reportObservedAt(canonicalAgo(2 * HOUR_SECONDS), 2))
        await server.submitReport(agent.agentId, agent.credential, reportObservedAt(canonicalAgo(HOUR_SECONDS), 3))

        // The floor clears, and the same series is stored again.
        declareFloor(server.stateDir, CLEARED_FLOOR)
        await server.restart()
        await server.submitReport(agent.agentId, agent.credential, reportObservedAt(observed.afterPause, 4))
        await server.submitReport(agent.agentId, agent.credential, reportObservedAt(observed.newest, 5))

        // The raw window the Server answers with is the one the policy
        // declares, and the series carries only what was really stored: the
        // three observations, and the pause in the middle of them.
        const history = await readHistory(server, nodeId, 'process_cpu_percent')
        expect(history.grain).toBe('raw')
        expect(history.aggregateSupported).toBe(false)
        expect(history.rawRetentionDays).toBe(1)
        expect(history.windowSeconds).toBe(24 * HOUR_SECONDS)
        expect(history.availability).toBeNull()
        expect(history.truncated).toBe(false)
        expect(history.items.map((item) => item.observedAt)).toEqual([
          observed.beforePause,
          observed.afterPause,
          observed.newest,
        ])
        expect(history.series.observed).toBe(true)
        expect(history.series.observationCount).toBe(3)
        expect(history.series.replayedCount).toBe(0)
        expect(history.series.correctedCount).toBe(0)
        expect(history.series.sampledCount).toBe(3)
        // Coverage is the stretch two stored observations prove — the ten
        // minutes between the two after the pause — never the paused hours.
        expect(history.series.coverageSeconds).toBe(10 * MINUTE_SECONDS)
        expect(history.gaps).toHaveLength(1)
        const pause = history.gaps[0]
        expect(pause.kind).toBe('protection_pause')
        expect(pause.skippedCount).toBe(2)
        expect(pause.reason).toBe('low-space protection paused sample collection')
        expect(pause.from).toBe(observed.beforePause)

        // The timing evidence of a sample belongs to that sample: observed and
        // received are reported apart, and the delay is measured, not assumed.
        for (const item of history.items) {
          expect(item.delaySeconds ?? -1).toBeGreaterThanOrEqual(0)
          expect(item.clockSuspect).toBe(false)
        }

        // A series this Node never reported is named as absent, an unusable
        // series token and range are refused, and an unknown Node never leaks.
        const neverReported = await readHistory(server, nodeId, 'data_directory_percent')
        expect(neverReported.series.observed).toBe(false)
        expect(neverReported.items).toHaveLength(0)
        expect(neverReported.series.observationCount).toBe(0)

        const invalidMetric = await server.adminGet(
          '/api/admin/v1/nodes/' + nodeId + '/metric-history?metric=carrier_pigeons',
        )
        expect(invalidMetric.status).toBe(400)
        expect((invalidMetric.body as { error: { code: string } }).error.code).toBe('invalid_metric')
        const invalidRange = await server.adminGet(
          '/api/admin/v1/nodes/' + nodeId + '/metric-history?metric=process_cpu_percent&from=yesterday',
        )
        expect(invalidRange.status).toBe(400)
        expect((invalidRange.body as { error: { code: string } }).error.code).toBe('invalid_history_range')
        const unknownNode = await server.adminGet(
          '/api/admin/v1/nodes/0195f2a1-00ff-40ff-80ff-0000000000ff/metric-history?metric=process_cpu_percent',
        )
        expect(unknownNode.status).toBe(404)

        // The production WebUI shows the same answer on the Node detail page.
        await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
        await expect(page.getByRole('heading', { level: 2, name: 'Metric history' })).toBeVisible()
        // The samples table names the timing evidence it carries, and a delay
        // never replaces the observation it belongs to.
        await expect(page.getByRole('columnheader', { name: 'Observed' })).toBeVisible()
        await expect(page.getByRole('columnheader', { name: 'Received' })).toBeVisible()
        await expect(page.getByText('3 stored observation(s) since the first one')).toBeVisible()
        await expect(page.getByText(/0 replay\(s\), 0 correction\(s\)/)).toBeVisible()
        await expect(page.getByText('10 minutes of 24 hours', { exact: false })).toBeVisible()

        // The pause is drawn as an undrawn band, and the line is broken at it:
        // nothing is plotted inside the silence. The observation before the
        // pause sits alone in its column, so it is drawn as a point with a
        // min/max whisker instead of a segment reaching across the silence, and
        // no drawn line crosses the band.
        const chart = page.locator('[data-slot=\'metric-history-chart\']')
        await expect(chart).toBeVisible()
        await expect(page.locator('[data-slot=\'metric-history-gap-band\']')).toHaveCount(1)
        expect(
          await page.locator('[data-slot=\'metric-history-whisker\']').count(),
        ).toBeGreaterThan(0)
        await expectNoLineCrossesTheGap(page)
        const gaps = page.locator('[data-slot=\'metric-history-gaps\']')
        await expect(gaps.getByText('Protection pause', { exact: true })).toBeVisible()
        await expect(gaps.getByText(/2 observation\(s\) skipped/)).toBeVisible()
        await expect(gaps).toContainText('low-space protection paused sample collection')
        await expect(page.locator('[data-slot=\'metric-history-sample\']')).toHaveCount(3)

        // Narrowing the window drops the pause out of the answered range: the
        // two samples after it remain, and no silence is reported for the
        // stretch the request no longer covers.
        await page.getByRole('group', { name: 'History range' }).getByRole('button', { name: '1 hour' }).click()
        await expect(page.locator('[data-slot=\'metric-history-sample\']')).toHaveCount(2)
        await expect(page.locator('[data-slot=\'metric-history-gaps\']')).toHaveCount(0)

        // A series the Node never reported is named as absent: no chart and no
        // zero line.
        await page.getByLabel('Metric series').selectOption('data_directory_percent')
        await expect(
          page.getByText(/This Node never reported Data directory/),
        ).toBeVisible()
        await expect(page.locator('[data-slot=\'metric-history-chart\']')).toHaveCount(0)
        await expect(page.locator('[data-slot=\'metric-history-samples\']')).toHaveCount(0)
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('the metric history surface holds across the fixed viewport and theme matrix while a collection gap is displayed', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix runs once against one disposable Server')
    test.setTimeout(600_000)
    const server = await startDisposableServer({
      capacity: { pauseBelowBytes: CLEARED_FLOOR, resumeAboveBytes: CLEARED_FLOOR, sampleIntervalSeconds: 5 },
    })
    try {
      const nodeId = fixtureNodeId()
      const agent = await server.enrollAgent()
      // Two minutes of real cadence, then a long stretch nobody observed: the
      // Server derives the gap from the series' own cadence, so the matrix does
      // not need a fabricated flag to display one.
      const first = canonicalAgo(23 * HOUR_SECONDS)
      const second = canonicalAgo(23 * HOUR_SECONDS - 2 * MINUTE_SECONDS)
      const third = canonicalAgo(23 * HOUR_SECONDS - 4 * MINUTE_SECONDS)
      await server.submitReport(agent.agentId, agent.credential, reportObservedAt(first, 1))
      await server.submitReport(agent.agentId, agent.credential, reportObservedAt(second, 2))
      await server.submitReport(agent.agentId, agent.credential, reportObservedAt(third, 3))
      await server.submitReport(agent.agentId, agent.credential, reportObservedAt(canonicalAgo(10 * MINUTE_SECONDS), 4))
      await server.submitReport(agent.agentId, agent.credential, reportObservedAt(canonicalAgo(5 * MINUTE_SECONDS), 5))

      const history = await readHistory(server, nodeId, 'process_cpu_percent')
      expect(history.items).toHaveLength(5)
      expect(history.gaps).toHaveLength(1)
      expect(history.gaps[0].kind).toBe('collection_gap')
      expect(history.gaps[0].reason).toBe('no observation was received in this stretch')

      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        for (const viewport of VIEWPORTS) {
          for (const colorScheme of ['light', 'dark'] as const) {
            await page.setViewportSize(viewport)
            await page.emulateMedia({ colorScheme })
            await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
            await expect(page.getByRole('heading', { level: 2, name: 'Metric history' })).toBeVisible()
            await expectResolvedTheme(page, colorScheme)
            await expect(page.locator('[data-slot=\'metric-history-chart\']')).toBeVisible()
            await expect(page.locator('[data-slot=\'metric-history-gap-band\']')).toHaveCount(1)
            await expectNoLineCrossesTheGap(page)
            await expect(
              page
                .locator('[data-slot=\'metric-history-gaps\']')
                .getByText('Collection gap', { exact: true }),
            ).toBeVisible()
            // The newest observations scroll inside their own region, never the
            // document, and the gap identity never widens the page.
            await expectLocalTableScroll(page, 'metric-history-samples')
            await expectNoHorizontalOverflow(page)
            if (viewport.width <= 768) {
              // The range presets and the series selector are the interactive
              // targets this panel owns: they stay touch sized and reachable by
              // keyboard while the gap is on screen.
              await expectVisibleInteractiveTargets(page)
              const series = await focusByKeyboard(page, page.getByLabel('Metric series'))
              await expect(series).toBeFocused()
              await expectFocusedElementHasVisibleFocus(page)
            }
          }
        }
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })
})
